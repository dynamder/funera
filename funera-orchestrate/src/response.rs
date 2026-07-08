use serde_json::Value as JsonValue;

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCallInfo>,
    pub iterations: usize,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub name: String,
    pub args: JsonValue,
    pub result: Result<String, String>,
}
