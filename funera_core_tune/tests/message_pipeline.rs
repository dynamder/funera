use funera_core::chat::message::{
    FuneraMessage, Message, MsgVariant, Role, TextMessage, ToolRequestMessage, ToolResponseMessage,
};
use funera_core::re_act::tool::ToolType;
use serde_json::{json, Value as JsonValue};
use uuid::Uuid;

// ── ToolRequestMessage tests ────────────────────────────────────

#[test]
fn tool_request_message_fields() {
    let call_id = Uuid::new_v4();
    let msg = FuneraMessage::new(
        Role::Assistant,
        MsgVariant::ToolRequest(ToolRequestMessage {
            tool_call_id: call_id,
            tool_type: ToolType::Function,
            function_name: "get_weather".into(),
            function_args: json!({"city": "NYC"}),
        }),
    );
    assert_eq!(msg.role().to_string(), "assistant");
    assert!(matches!(msg.msg_variant(), MsgVariant::ToolRequest(_)));
}

#[test]
fn tool_request_message_to_prompt_content() {
    let call_id = Uuid::new_v4();
    let msg = FuneraMessage::new(
        Role::Assistant,
        MsgVariant::ToolRequest(ToolRequestMessage {
            tool_call_id: call_id,
            tool_type: ToolType::Function,
            function_name: "search".into(),
            function_args: json!({"q": "rust"}),
        }),
    );
    let content = msg.to_prompt_content();
    assert!(content.contains("search"));
    assert!(content.contains("rust"));
    assert!(content.contains(&call_id.to_string()));
}

#[test]
fn tool_request_message_format_json() {
    let call_id = Uuid::new_v4();
    let msg = FuneraMessage::new(
        Role::Assistant,
        MsgVariant::ToolRequest(ToolRequestMessage {
            tool_call_id: call_id,
            tool_type: ToolType::Function,
            function_name: "calculate".into(),
            function_args: json!({"expr": "1+1"}),
        }),
    );
    let json = msg.format_json();
    assert_eq!(json["role"], "assistant");
    assert!(json["tool_calls"].is_array());
    assert_eq!(json["tool_calls"][0]["function"]["name"], "calculate");
    assert_eq!(
        json["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap(),
        "{\"expr\":\"1+1\"}",
    );
}

// ── System message tests ───────────────────────────────────────

#[test]
fn system_message_format_json() {
    let msg = FuneraMessage::new(
        Role::System,
        MsgVariant::Text(TextMessage {
            text: "You are a helpful assistant.".into(),
            reasoning_content: None,
        }),
    );
    let json = msg.format_json();
    assert_eq!(json["role"], "system");
    assert_eq!(json["content"], "You are a helpful assistant.");
}

#[test]
fn system_message_to_prompt_content() {
    let msg = FuneraMessage::new(
        Role::System,
        MsgVariant::Text(TextMessage {
            text: "Be concise.".into(),
            reasoning_content: None,
        }),
    );
    assert_eq!(msg.to_prompt_content(), "Be concise.");
}

// ── Multi-turn tool-chain history ─────────────────────────────

#[test]
fn multi_turn_tool_chain_formats_correctly() {
    let user_msg = FuneraMessage::new(
        Role::User,
        MsgVariant::Text(TextMessage {
            text: "What's the weather in NYC?".into(),
            reasoning_content: None,
        }),
    );

    let call_id = Uuid::new_v4();
    let assistant_tool_call = FuneraMessage::new(
        Role::Assistant,
        MsgVariant::ToolRequest(ToolRequestMessage {
            tool_call_id: call_id,
            tool_type: ToolType::Function,
            function_name: "get_weather".into(),
            function_args: json!({"city": "NYC"}),
        }),
    );

    let tool_response = FuneraMessage::new(
        Role::Tool,
        MsgVariant::ToolResponse(ToolResponseMessage {
            tool_call_id: call_id,
            result: "72°F, sunny".into(),
        }),
    );

    let assistant_response = FuneraMessage::new(
        Role::Assistant,
        MsgVariant::Text(TextMessage {
            text: "The weather in NYC is 72°F and sunny.".into(),
            reasoning_content: None,
        }),
    );

    let history: Vec<JsonValue> = vec![
        user_msg.format_json(),
        assistant_tool_call.format_json(),
        tool_response.format_json(),
        assistant_response.format_json(),
    ];

    assert_eq!(history.len(), 4);
    assert_eq!(history[0]["role"], "user");
    assert_eq!(history[0]["content"], "What's the weather in NYC?");
    assert_eq!(history[1]["role"], "assistant");
    assert!(history[1]["tool_calls"].is_array());
    assert_eq!(history[2]["role"], "tool");
    assert_eq!(history[2]["tool_call_id"], call_id.to_string());
    assert_eq!(history[3]["role"], "assistant");
    assert_eq!(
        history[3]["content"],
        "The weather in NYC is 72°F and sunny."
    );
}

// ── Full message serde roundtrip ───────────────────────────────

#[test]
fn tool_request_serde_roundtrip() {
    let call_id = Uuid::new_v4();
    let original = FuneraMessage::new(
        Role::Assistant,
        MsgVariant::ToolRequest(ToolRequestMessage {
            tool_call_id: call_id,
            tool_type: ToolType::Function,
            function_name: "test".into(),
            function_args: json!({"key": "value"}),
        }),
    );

    let json_str = serde_json::to_string(&original).unwrap();
    let deserialized: FuneraMessage = serde_json::from_str(&json_str).unwrap();

    assert_eq!(original.id(), deserialized.id());
    assert_eq!(original.role().to_string(), deserialized.role().to_string());
    assert!(matches!(
        deserialized.msg_variant(),
        MsgVariant::ToolRequest(_)
    ));
}

// ── Edge cases ─────────────────────────────────────────────────

#[test]
fn tool_request_with_empty_args() {
    let call_id = Uuid::new_v4();
    let msg = FuneraMessage::new(
        Role::Assistant,
        MsgVariant::ToolRequest(ToolRequestMessage {
            tool_call_id: call_id,
            tool_type: ToolType::Function,
            function_name: "noop".into(),
            function_args: json!({}),
        }),
    );
    let json = msg.format_json();
    assert_eq!(json["tool_calls"][0]["function"]["name"], "noop");
    assert_eq!(
        json["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap(),
        "{}",
    );
}

#[test]
fn tool_response_with_empty_result() {
    let call_id = Uuid::new_v4();
    let msg = FuneraMessage::new(
        Role::Tool,
        MsgVariant::ToolResponse(ToolResponseMessage {
            tool_call_id: call_id,
            result: "".into(),
        }),
    );
    let json = msg.format_json();
    assert_eq!(json["role"], "tool");
    assert_eq!(json["tool_call_id"], call_id.to_string());
}

#[test]
fn text_message_with_special_chars() {
    let msg = FuneraMessage::new(
        Role::User,
        MsgVariant::Text(TextMessage {
            text: "line1\nline2\twith\ttabs".into(),
            reasoning_content: None,
        }),
    );
    let json = msg.format_json();
    assert_eq!(json["content"], "line1\nline2\twith\ttabs");
}

// ── FuneraMessage msg_variant accessor ─────────────────────────

#[test]
fn funera_message_msg_variant_matches() {
    let text_msg = FuneraMessage::new(
        Role::User,
        MsgVariant::Text(TextMessage {
            text: "hi".into(),
            reasoning_content: None,
        }),
    );
    assert!(matches!(text_msg.msg_variant(), MsgVariant::Text(_)));

    let req_msg = FuneraMessage::new(
        Role::Assistant,
        MsgVariant::ToolRequest(ToolRequestMessage {
            tool_call_id: Uuid::new_v4(),
            tool_type: ToolType::Function,
            function_name: "fn".into(),
            function_args: json!({}),
        }),
    );
    assert!(matches!(req_msg.msg_variant(), MsgVariant::ToolRequest(_)));

    let resp_msg = FuneraMessage::new(
        Role::Tool,
        MsgVariant::ToolResponse(ToolResponseMessage {
            tool_call_id: Uuid::new_v4(),
            result: "done".into(),
        }),
    );
    assert!(matches!(
        resp_msg.msg_variant(),
        MsgVariant::ToolResponse(_)
    ));
}
