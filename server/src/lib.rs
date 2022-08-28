use std::future::Future;
use std::sync::Arc;

pub use options::Options;
use slv_input::index;
use tokio::sync::broadcast;

mod options;
mod ws;

pub async fn new(
    options: Options,
    shutdown: broadcast::Receiver<()>,
    index: Arc<index::Store>,
) -> Result<impl Future<Output = ()>, InitError> {
    let ws = ws::init(options, shutdown, index).await?;

    Ok(ws)
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("{0}")]
    Conn(#[from] ws::InitError),
}
