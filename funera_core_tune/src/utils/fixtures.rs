use funera_core::{
    chat::message::{FuneraMessage, MsgVariant, Role, TextMessage, ToolResponseMessage},
    re_act::tool::{ToolCallError, ToolRegistry},
};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::utils::mock_tool::MockTool;

pub fn text_message(role: Role, text: &str) -> FuneraMessage {
    FuneraMessage::new(role, MsgVariant::Text(TextMessage { text: text.into() }))
}

pub fn tool_response_message(tool_call_id: Uuid, result: &str) -> FuneraMessage {
    FuneraMessage::new(
        Role::Tool,
        MsgVariant::ToolResponse(ToolResponseMessage {
            tool_call_id,
            result: result.into(),
        }),
    )
}

pub fn create_registry_with_tools(names: &[&str]) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for name in names {
        let tool = MockTool::new(*name, default_schema(name));
        registry.add_tool(Box::new(tool));
    }
    registry
}

pub fn default_schema(name: &str) -> JsonValue {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": name,
            "description": format!("A mock tool named {}", name),
            "parameters": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }
    })
}

pub fn err_tool(name: &str, error: ToolCallError) -> MockTool {
    MockTool::new(name, default_schema(name)).with_result(Err(error))
}

pub fn sample_history_messages() -> Vec<JsonValue> {
    vec![
        serde_json::json!({
            "role": "user",
            "content": "Hello"
        }),
        serde_json::json!({
            "role": "assistant",
            "content": "Hi! How can I help you?"
        }),
    ]
}
