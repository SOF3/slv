use std::future::Future;
use std::sync::Arc;

pub use options::Options;
use slv_input::index;
use tokio::sync::broadcast;

mod options;
mod session;
mod ws;

pub async fn new(
    options: Options,
    shutdown_tx: &broadcast::Sender<()>,
    index: Arc<index::Store>,
) -> Result<impl Future<Output = ()>, InitError> {
    let ws = ws::init(options, shutdown_tx.subscribe(), index).await?;

    Ok(ws)
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("{0}")]
    Conn(#[from] ws::InitError),
}
