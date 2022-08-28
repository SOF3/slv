use futures::channel::mpsc;
use futures::{Sink, SinkExt, Stream, StreamExt as _};
use slv_proto::{client, server};

use crate::index;

pub async fn handle(
    stream: impl Stream<Item = client::Message> + Unpin,
    sink: impl Sink<server::Message, Error = mpsc::SendError> + Unpin,
    index: &index::Store,
) {
    if let Err(err) = handle_fallible(stream, sink, index).await {
        log::error!("Error handling client session: {err}");
    }
}

async fn handle_fallible(
    mut stream: impl Stream<Item = client::Message> + Unpin,
    mut sink: impl Sink<server::Message, Error = mpsc::SendError> + Unpin,
    index: &index::Store,
) -> Result<(), Error> {
    while let Some(message) = stream.next().await {
        match message {
            client::Message::Handshake(_) => return Err(Error::MultiAuth),
            client::Message::ListKeys(_) => {
                let keys = index.list_indices();
                sink.send(server::Message::UpdateKeyList(keys)).await?;
            }
        }
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("Client should not send auth message multiple times")]
    MultiAuth,
    #[error("Socket adapter closed: {0}")]
    Send(#[from] mpsc::SendError),
}
