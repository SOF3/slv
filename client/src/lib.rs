use std::sync::Arc;

use arc_swap::ArcSwap;
use futures::{Sink, Stream, StreamExt};
use slv_proto::IndexMethod;
use tokio::sync::broadcast;

pub struct State<Tx: Sink<slv_proto::client::Message> + Unpin> {
    tx:       Tx,
    key_list: ArcSwap<Vec<IndexMethod>>,
}

impl<Tx: Sink<slv_proto::client::Message> + Unpin> State<Tx> {
    pub fn new(tx: Tx) -> Self { Self { tx, key_list: ArcSwap::default() } }

    pub fn status_line(&self) -> String {
        let key_list = self.key_list.load();
        format!("{} keys", key_list.len())
    }
}

pub async fn worker<
    Tx: Sink<slv_proto::client::Message> + Unpin,
    Rx: Stream<Item = slv_proto::server::Message> + Unpin,
>(
    state: &State<Tx>,
    mut rx: Rx,
    mut shutdown: broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            message = rx.next() => {
                let message = match message {
                    Some(message) => message,
                    None => break,
                };

                handle_message(state, message).await;
            },
        }
    }
}

async fn handle_message<Tx: Sink<slv_proto::client::Message> + Unpin>(
    state: &State<Tx>,
    message: slv_proto::server::Message,
) {
    match message {
        slv_proto::server::Message::HandshakeOk(_) => {}
        slv_proto::server::Message::UpdateKeyList(list) => {
            let list = Arc::new(list);
            state.key_list.store(Arc::clone(&list));
        }
        slv_proto::server::Message::StatusFeed(_) => todo!(),
    }
}
