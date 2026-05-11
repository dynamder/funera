use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::chat::message::FuneraMessage;
use serde_json::{Value as JsonValue, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuneraSession {
    id: Uuid,
    msgs: Arc<RwLock<Vec<FuneraMessage>>>,
}

impl FuneraSession {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            msgs: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn session_context(&self) -> JsonValue {
        self.msgs
            .read()
            .iter()
            .map(|msg| msg.format_request())
            .collect::<Vec<_>>()
            .into()
    }

    pub fn push_message(&self, msg: FuneraMessage) {
        self.msgs.write().push(msg);
    }
}
