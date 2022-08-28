use std::sync::Arc;

use arcstr::ArcStr;
pub use rmp_serde::{decode, encode};

pub mod client {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    pub enum Message {
        Handshake(Handshake),
        ListKeys(ListKeys),
    }

    #[derive(Serialize, Deserialize)]
    pub struct Handshake {
        pub token: String,
    }

    #[derive(Serialize, Deserialize)]
    pub struct ListKeys {}
}

pub mod server {
    use serde::{Deserialize, Serialize};

    use crate::IndexMethod;

    #[derive(Serialize, Deserialize)]
    pub enum Message {
        HandshakeOk(HandshakeOk),
        UpdateKeyList(Vec<IndexMethod>),
        StatusFeed(StatusFeed),
    }

    #[derive(Serialize, Deserialize)]
    pub struct HandshakeOk {}

    #[derive(Serialize, Deserialize)]
    pub struct IndexList {
        indices: Vec<IndexMethod>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct StatusFeed {
        file_name:      String,
        lines_in_index: u64,
    }
}

#[derive(Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub struct IndexMethod {
    pub conditions: Arc<[FieldCondition]>,
}

impl IndexMethod {
    pub fn new(mut units: Vec<FieldCondition>) -> Self {
        units.sort();
        Self { conditions: Arc::from(units) }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize)]
pub enum FieldCondition {
    HasKey(ArcStr),
    KeyValue(ArcStr, ArcStr),
}

impl FieldCondition {
    pub fn key(&self) -> &str {
        match self {
            Self::HasKey(key) | Self::KeyValue(key, _) => key,
        }
    }
}

pub enum Entry {
    Json(JsonEntry),
    Raw(RawEntry),
}

pub struct JsonEntry(pub Vec<(ArcStr, arcstr::ArcStr)>);

pub struct RawEntry(pub Arc<[u8]>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageId(pub usize);
