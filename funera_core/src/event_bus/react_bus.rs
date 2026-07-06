use crate::{chat::message::FuneraMessage, re_act::tool::ToolCallError};
use serde_json::Value as JsonValue;
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    index: usize,
    call_id: String,
    name: String,
    args: JsonValue,
}

#[derive(Debug, Clone)]
pub struct ToolCallResponse {
    call_id: String,
    result: String,
}

#[derive(Debug, Clone)]
pub enum ReactEvent {
    TurnStart,
    TurnEnd,
    MessageQueued(FuneraMessage),
    ToolExecRequest(ToolCallRequest),
    ToolExecResponse(Result<ToolCallResponse, String>),
}

#[derive(Debug)]
pub struct ReactBus {
    react_tx: broadcast::Sender<ReactEvent>,
}

impl ReactBus {
    pub fn new() -> Self {
        let (react_tx, _) = broadcast::channel(30);
        Self { react_tx }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<ReactEvent> {
        self.react_tx.subscribe()
    }
    pub fn send(&self, event: ReactEvent) -> anyhow::Result<usize> {
        self.react_tx.send(event).map_err(|e| e.into())
    }
}
