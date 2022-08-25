use std::future::Future;
use std::sync::Arc;

use tokio::sync::broadcast;

pub mod index;
mod source;

pub async fn init(
    options: Options,
    shutdown_tx: &broadcast::Sender<()>,
) -> Result<(Arc<index::Store>, impl Future<Output = ()>), InitError> {
    let store = Arc::new(index::Store::new(options.index));
    let input = source::init(
        options.source,
        {
            let store = Arc::clone(&store);
            move |message| store.push(message)
        },
        shutdown_tx.subscribe(),
    )
    .await?;

    Ok((store, input))
}

#[derive(clap::Parser)]
pub struct Options {
    #[clap(flatten)]
    index:  index::Options,
    #[clap(flatten)]
    source: source::Options,
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("{0}")]
    Source(#[from] source::InitError),
}
