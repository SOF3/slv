use clap::Parser;
use tokio::signal;
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let options = Options::parse();

    let (shutdown_tx, _) = broadcast::channel(1);

    let (index, input) = slv_input::init(options.input, &shutdown_tx).await?;
    let ws = slv_server::new(options.server, &shutdown_tx, index).await?;

    let mut handles = Vec::new();
    handles.push(tokio::spawn(input));
    if options.enable_server {
        handles.push(tokio::spawn(ws));
    }

    if let Err(err) = signal::ctrl_c().await {
        log::error!("Cannot register ctrl-c handler: {err}");
    }

    drop(shutdown_tx);

    for handle in handles {
        if let Err(err) = handle.await {
            log::error!("Thread panicked: {err}");
        }
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Error starting input source: {0}")]
    InputError(#[from] slv_input::InitError),
    #[error("Error starting server: {0}")]
    ServerError(#[from] slv_server::InitError),
}

#[derive(Parser)]
#[clap(name = "slv", version, author, about)]
pub struct Options {
    #[clap(flatten)]
    pub input:         slv_input::Options,
    #[clap(long = "disable-http", action = clap::ArgAction::SetFalse)]
    pub enable_server: bool,
    #[clap(flatten)]
    pub server:        slv_server::Options,
}
