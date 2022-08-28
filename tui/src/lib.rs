use std::future::Future;
use std::io::{self, Write};
use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyModifiers};
use crossterm::terminal::{self, disable_raw_mode, enable_raw_mode};
use crossterm::QueueableCommand as _;
use futures::channel::mpsc;
use futures::StreamExt;
use slv_input::index;
use tokio::sync::broadcast;
use tui::backend::{Backend, CrosstermBackend};
use tui::{layout, widgets, Terminal};

type State = slv_client::State<mpsc::UnboundedSender<slv_proto::client::Message>>;

pub async fn init(
    index: Arc<index::Store>,
    shutdown_tx: broadcast::Sender<()>,
    shutdown_rx: broadcast::Receiver<()>,
) -> Result<impl Future<Output = ()>, InitError> {
    let run = async move {
        if let Err(err) = start_tui(index, shutdown_tx, shutdown_rx).await {
            eprintln!("Error: {err}");
        }
    };
    Ok(run)
}

type TerminalCommandFn =
    Box<dyn FnOnce(&mut io::Stdout) -> Result<&mut io::Stdout, io::Error> + Send>;
struct ResetTerminalGuardBuilder {
    pairs: Vec<[TerminalCommandFn; 2]>,
}
impl ResetTerminalGuardBuilder {
    fn new() -> Self {
        Self {
            pairs: vec![[
                Box::new(|s| enable_raw_mode().map(|()| s)),
                Box::new(|s| disable_raw_mode().map(|()| s)),
            ]],
        }
    }

    fn with(
        mut self,
        enable: impl crossterm::Command + 'static + Send,
        disable: impl crossterm::Command + 'static + Send,
    ) -> Self {
        self.pairs.push([
            Box::new(|stdout| stdout.queue(enable)),
            Box::new(|stdout| stdout.queue(disable)),
        ]);
        self
    }

    fn build(self) -> Result<ResetTerminalGuard, io::Error> {
        // we deliberately want to run the drop hook of `guard` on error
        let mut guard = ResetTerminalGuard { shutdown: Vec::new(), stdout: io::stdout() };
        for [enable, disable] in self.pairs {
            enable(&mut guard.stdout)?;
            guard.shutdown.push(disable);
        }
        guard.stdout.flush()?;
        Ok(guard)
    }
}
struct ResetTerminalGuard {
    shutdown: Vec<TerminalCommandFn>,
    stdout:   io::Stdout,
}
impl Drop for ResetTerminalGuard {
    fn drop(&mut self) {
        _ = disable_raw_mode();
        for item in self.shutdown.drain(..) {
            _ = item(&mut self.stdout); // continue on error anyway
        }
        _ = self.stdout.flush();
    }
}

async fn start_tui(
    index: Arc<index::Store>,
    shutdown_tx: broadcast::Sender<()>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<(), RunError> {
    let mut guard = ResetTerminalGuardBuilder::new()
        .with(terminal::EnterAlternateScreen, terminal::LeaveAlternateScreen)
        .build()
        .map_err(RunError::Configure)?;
    let stdout = &mut guard.stdout;

    let (session_tx, client_rx) = mpsc::unbounded();
    let (client_tx, session_rx) = mpsc::unbounded();
    let state = Arc::new(slv_client::State::new(client_tx));

    tokio::spawn({
        async move {
            slv_input::session::handle(session_rx, session_tx, &index).await;
        }
    });

    tokio::spawn({
        let state = Arc::clone(&state);
        let shutdown = shutdown_rx.resubscribe();
        async move { slv_client::worker(&state, client_rx, shutdown).await }
    });

    let mut terminal =
        Terminal::new(CrosstermBackend::new(stdout)).map_err(RunError::StartTerminal)?;

    let mut term_events = crossterm::event::EventStream::new();

    loop {
        terminal.draw(|f| ui(f, &state)).map_err(RunError::Draw)?;

        tokio::select! {
            _ = shutdown_rx.recv() => break,
            event = term_events.next() => {
                match event {
                    None => break,
                    Some(Ok(event)) => handle_event(event, &shutdown_tx),
                    Some(Err(err)) => {
                        eprintln!("{err}");
                        _ = shutdown_tx.send(());
                    },
                }
            }
        }
    }

    Ok(())
}

fn ui(f: &mut tui::Frame<impl Backend>, state: &State) {
    let [main_chunk, status_chunk]: [_; 2] = layout::Layout::default()
        .direction(layout::Direction::Vertical)
        .margin(1)
        .constraints([layout::Constraint::Min(1), layout::Constraint::Max(1)])
        .split(f.size())
        .try_into()
        .expect("constraints.len()");

    f.render_widget(widgets::Paragraph::new("slv"), main_chunk);
    f.render_widget(widgets::Paragraph::new(state.status_line()), status_chunk)
}

fn handle_event(event: Event, shutdown_tx: &broadcast::Sender<()>) {
    match event {
        Event::Key(event) => {
            if event.modifiers.contains(KeyModifiers::CONTROL) && event.code == KeyCode::Char('c') {
                log::debug!("ctrl-c received from crossterm");
                _ = shutdown_tx.send(());
            }
        }
        Event::Paste(_) => {
            // do nothing, paste is most likely an accident since there is no input
        }
        _ => {}
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("Cannot start interactive terminal: {0}")]
    StartTerminal(io::Error),
    #[error("Cannot configure terminal: {0}")]
    Configure(io::Error),
    #[error("Cannot draw terminal: {0}")]
    Draw(io::Error),
}
