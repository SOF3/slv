use std::future::Future;
use std::sync::Arc;

use tokio::sync::broadcast;

pub mod index;
pub mod session;
mod source;

pub async fn init(
    options: Options,
    shutdown: broadcast::Receiver<()>,
) -> Result<(Arc<index::Store>, impl Future<Output = ()>), InitError> {
    let store = Arc::new(index::Store::new(options.index));
    let input = source::init(
        options.source,
        {
            let store = Arc::clone(&store);
            move |message| store.push(message)
        },
        shutdown,
    )
    .await?;

    Ok((store, input))
}

#[derive(clap::Parser)]
pub struct Options {
    #[clap(flatten)]
    pub index:  index::Options,
    #[clap(flatten)]
    pub source: source::Options,
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("{0}")]
    Source(#[from] source::InitError),
}
