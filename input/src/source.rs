use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::{future, Future, Stream, StreamExt as _};
use inotify::{Inotify, WatchMask};
use slv_proto::{Entry, JsonEntry, RawEntry};
use tokio::io::{self, AsyncBufReadExt as _, AsyncSeekExt as _};
use tokio::sync::broadcast;
use tokio::{fs, time};

pub async fn init(
    options: Options,
    receiver: impl Fn(Entry),
    shutdown: broadcast::Receiver<()>,
) -> Result<impl Future<Output = ()>, InitError> {
    let input = if options.input.as_os_str() == "-" {
        Input::stream(io::BufReader::new(Box::pin(io::stdin())))
    } else if options.watch {
        let inotify = if options.inotify {
            match setup_inotify(&options.input) {
                Ok(inotify) => Some(inotify),
                Err(err) => {
                    log::warn!("Cannot enable inotify for input: {err}");
                    None
                }
            }
        } else {
            None
        };

        let notifier = match inotify {
            Some(inotify) => Notifier::inotify(inotify),
            None => Notifier::Timer { interval: options.watch_interval.into(), current: None },
        };

        Input::watch_file(options.input.clone(), notifier)
    } else {
        let file = fs::File::open(&options.input).await.map_err(InitError::OpenInput)?;
        Input::stream(io::BufReader::new(Box::pin(file)))
    };

    Ok(async move {
        watch_loop(input, receiver, shutdown).await;
    })
}

type InotifyStream = Pin<Box<dyn Stream<Item = io::Result<inotify::EventOwned>> + Send>>;

fn setup_inotify(path: &Path) -> io::Result<InotifyStream> {
    let mut inotify = Inotify::init()?;
    inotify.add_watch(path, WatchMask::MODIFY | WatchMask::MOVE | WatchMask::CREATE)?;
    Ok(Box::pin(inotify.event_stream([0; 4096])?))
}

enum Input {
    Stream {
        reader: io::BufReader<Pin<Box<dyn io::AsyncRead + Send>>>,
        buf:    Vec<u8>,
    },
    WatchFile {
        path:         PathBuf,
        notifier:     Notifier,
        previous_len: u64,
        file:         Option<io::BufReader<fs::File>>,
        buf:          Vec<u8>,
    },
}

impl Input {
    fn stream(reader: impl io::AsyncRead + Send + 'static) -> Self {
        let reader = io::BufReader::new(Box::pin(reader) as Pin<Box<dyn io::AsyncRead + Send>>);
        Self::Stream { reader, buf: Vec::new() }
    }

    fn watch_file(path: PathBuf, notifier: Notifier) -> Self {
        Self::WatchFile { path, notifier, previous_len: 0, file: None, buf: Vec::new() }
    }

    /// Reads the next line, cancel-safe
    async fn next_line(&mut self) -> io::Result<Entry> {
        let message = match self {
            Self::Stream { reader, buf } => {
                let len = reader.read_until(b'\n', buf).await?;
                if len == 0 {
                    future::pending::<Infallible>().await;
                }

                let message = parse_entry(&buf[..]);
                buf.clear();
                message
            }
            Self::WatchFile { notifier, previous_len, path, file, buf } => loop {
                let file = match file {
                    Some(file) => {
                        let metadata = fs::metadata(path.as_path()).await?;
                        if metadata.len() != *previous_len {
                            // file truncated, seek from start
                            file.seek(io::SeekFrom::Start(0)).await?;
                        }
                        *previous_len = metadata.len();
                        file
                    }
                    None => file.insert(io::BufReader::new(fs::File::open(path.as_path()).await?)),
                };

                let read = file.read_until(b'\n', buf).await?;
                if read == 0 {
                    // EOF, go to re-read loop
                    notifier.wait().await?;
                    continue;
                }

                let message = parse_entry(&buf[..]);
                buf.clear();
                break message;
            },
        };
        Ok(message)
    }
}

enum Notifier {
    Inotify { inotify: InotifyStream },
    Timer { interval: Duration, current: Option<time::Instant> },
}

impl Notifier {
    fn inotify(inotify: InotifyStream) -> Self { Self::Inotify { inotify } }

    async fn wait(&mut self) -> io::Result<()> {
        match self {
            Self::Inotify { inotify } => {
                let result = inotify.next().await.expect("InotifyStream never closes");
                result?; // inotify error
            }
            Self::Timer { interval, current } => {
                let until = current.get_or_insert_with(|| time::Instant::now() + *interval);
                time::sleep_until(*until).await;
            }
        }
        Ok(())
    }
}

fn parse_entry(bytes: &[u8]) -> Entry {
    match serde_json::from_slice::<BTreeMap<_, _>>(bytes) {
        Ok(fields) => Entry::Json(JsonEntry({
            let fields: Vec<_> = fields.into_iter().collect();
            assert!(fields.windows(2).all(|pair| pair[0].0 < pair[1].0)); // BTreeMap iterates in order
            fields
        })),
        Err(_) => {
            let stripped = bytes.strip_suffix(b"\r\n").unwrap_or(bytes);
            Entry::Raw(RawEntry(Arc::from(stripped)))
        }
    }
}

async fn watch_loop(
    mut input: Input,
    mut receiver: impl FnMut(Entry),
    mut shutdown: broadcast::Receiver<()>,
) {
    loop {
        let message = tokio::select! {
            _ = shutdown.recv() => break,
            message = input.next_line() => message,
        };

        let message = match message {
            Ok(message) => message,
            Err(err) => {
                log::error!("Cannot poll message: {err}");
                continue;
            }
        };

        receiver(message);
    }
}

#[derive(clap::Parser)]
pub struct Options {
    /// Path to log file, or `-` to read from stdin.
    #[clap(value_parser, default_value = "-")]
    pub input: PathBuf,

    /// Watch file for updates. No effect if input is stdin.
    #[clap(long = "no-watch", action = clap::ArgAction::SetFalse)]
    pub watch:          bool,
    /// Use inotify to watch file. No effect if `--no-watch`.
    #[clap(long = "no-inotify", action = clap::ArgAction::SetFalse)]
    pub inotify:        bool,
    /// The interval to try to read new data from a file, if inotify is unavailable.
    #[clap(long, value_parser)]
    pub watch_interval: humantime::Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("Failed to open input file: {0}")]
    OpenInput(io::Error),
    #[error("Failed to set up inotify for input file: {0}")]
    Inotify(io::Error),
}
