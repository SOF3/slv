use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::Arc;
use std::{env, fs, io};

use clap::Parser;
use tokio::signal;
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(err) = run().await {
        eprintln!("{err}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

async fn run() -> Result<(), Error> {
    let options = Options::parse();

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    let mut inits: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = Vec::new();

    let implicit_noninteractive = options.interactive && atty::isnt(atty::Stream::Stdout);
    let read_from_stdin = options.input.source.input.as_os_str() == "-";

    let (index, input) = slv_input::init(options.input, shutdown_rx.resubscribe()).await?;
    inits.push(Box::pin(input));

    if options.interactive && !implicit_noninteractive {
        if let Ok(path) = env::var("RUST_LOG_FILE") {
            let pipe = fs::File::create(path).map_err(Error::LogFileOpen)?;
            simplelog::WriteLogger::init(
                log::LevelFilter::Debug,
                simplelog::Config::default(),
                pipe,
            )
            .expect("only run once");
        }

        inits.push(Box::pin(
            slv_tui::init(Arc::clone(&index), shutdown_tx.clone(), shutdown_rx.resubscribe())
                .await?,
        ));
    } else {
        env_logger::init();
        if implicit_noninteractive {
            log::info!("Interactive mode disabled automatically because stdout is not a tty");
        }
    }

    if options.enable_server {
        let ws = slv_server::new(options.server, shutdown_rx.resubscribe(), index).await?;
        inits.push(Box::pin(ws));
    }

    let handles: Vec<_> = inits.into_iter().map(tokio::spawn).collect();

    tokio::spawn(async move {
        if let Err(err) = signal::ctrl_c().await {
            log::error!("Cannot register ctrl-c handler: {err}");
        }
        log::debug!("Received ctrl-c from tokio signal");

        _ = shutdown_tx.send(()); // error does not matter
    });

    for handle in handles {
        if let Err(err) = handle.await {
            log::error!("Thread panicked: {err}");
        }
    }

    if read_from_stdin && atty::is(atty::Stream::Stdin) {
        eprintln!("slv is reading from stdin. Press Enter to exit.");
    }

    Ok(())
}

#[derive(Parser)]
#[clap(name = "slv", version, author, about)]
pub struct Options {
    #[clap(flatten)]
    pub input: slv_input::Options,

    /// Do not start a websocket server.
    #[clap(long = "disable-http", action = clap::ArgAction::SetFalse)]
    pub enable_server: bool,
    #[clap(flatten)]
    pub server:        slv_server::Options,

    /// Do not start interactive UI.
    ///
    /// Interactive mode is automatically disabled if the standard output is not a terminal.
    #[clap(
        long = "non-interactive",
        action = clap::ArgAction::SetFalse,
    )]
    pub interactive: bool,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Cannot open target log file for slv")]
    LogFileOpen(io::Error),
    #[error("Error starting input source: {0}")]
    Input(#[from] slv_input::InitError),
    #[error("Error starting server: {0}")]
    Server(#[from] slv_server::InitError),
    #[error("{0}")]
    Tui(#[from] slv_tui::InitError),
}
