use crate::chat::message::FuneraMessage;
use serde_json::Value as JsonValue;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub index: usize,
    pub call_id: String,
    pub name: String,
    pub args: JsonValue,
}

#[derive(Debug, Clone)]
pub struct ToolCallResponse {
    pub call_id: String,
    pub result: String,
}

#[derive(Debug, Clone)]
pub enum ReactEvent {
    TurnStart,
    TurnEnd,
    MessageQueued(FuneraMessage),
    ToolExecRequest(ToolCallRequest),
    ToolExecResponse(Result<ToolCallResponse, String>),
}

#[derive(Debug, Clone)]
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
    pub fn sender(&self) -> broadcast::Sender<ReactEvent> {
        self.react_tx.clone()
    }
    pub fn send(&self, event: ReactEvent) -> anyhow::Result<usize> {
        self.react_tx.send(event).map_err(|e| e.into())
    }
}
