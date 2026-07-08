use serde_json::Value as JsonValue;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Token(String),
    ToolCallStart {
        index: usize,
        call_id: Uuid,
        name: String,
        args: JsonValue,
    },
    ToolCallResult {
        call_id: Uuid,
        name: String,
        result: Result<String, String>,
    },
    TurnStart,
    TurnEnd,
    Error(String),
    Done,
}
