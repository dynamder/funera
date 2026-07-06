use chrono::{DateTime, Utc};
use funera_core::chat::message::{
    FuneraMessage, Message, MsgVariant, Role, TextMessage, ToolResponseMessage,
};
use uuid::Uuid;

#[test]
fn role_display() {
    assert_eq!(Role::User.to_string(), "user");
    assert_eq!(Role::Assistant.to_string(), "assistant");
    assert_eq!(Role::System.to_string(), "system");
    assert_eq!(Role::Tool.to_string(), "tool");
}

#[test]
fn role_serde_roundtrip() {
    let roles = [Role::User, Role::Assistant, Role::System, Role::Tool];
    for role in &roles {
        let json = serde_json::to_string(role).unwrap();
        let deserialized: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(*role, deserialized);
    }
}

#[test]
fn text_message_new() {
    let msg = FuneraMessage::new(
        Role::User,
        MsgVariant::Text(TextMessage {
            text: "hello".into(),
        }),
    );
    assert_eq!(*msg.role(), Role::User);
    assert_eq!(msg.to_prompt_content(), "hello");
    assert!(matches!(msg.msg_variant(), MsgVariant::Text(_)));
}

#[test]
fn text_message_format_json() {
    let msg = FuneraMessage::new(
        Role::Assistant,
        MsgVariant::Text(TextMessage { text: "hi".into() }),
    );
    let json = msg.format_json();
    assert_eq!(json["role"], "assistant");
    assert_eq!(json["content"], "hi");
}

#[test]
fn text_message_is_unique_id() {
    let msg1 = FuneraMessage::new(
        Role::User,
        MsgVariant::Text(TextMessage { text: "a".into() }),
    );
    let msg2 = FuneraMessage::new(
        Role::User,
        MsgVariant::Text(TextMessage { text: "a".into() }),
    );
    assert_ne!(msg1.id(), msg2.id());
}

#[test]
fn text_message_timestamp_is_recent() {
    let before = Utc::now();
    let msg = FuneraMessage::new(
        Role::User,
        MsgVariant::Text(TextMessage { text: "x".into() }),
    );
    let after = Utc::now();
    assert!(*msg.timestamp() >= before);
    assert!(*msg.timestamp() <= after);
}

#[test]
fn tool_response_message_format_json() {
    let tool_call_id = Uuid::new_v4();
    let msg = FuneraMessage::new(
        Role::Tool,
        MsgVariant::ToolResponse(ToolResponseMessage {
            tool_call_id,
            result: "25°C".into(),
        }),
    );
    let json = msg.format_json();
    assert_eq!(json["role"], "tool");
    assert_eq!(json["tool_call_id"], tool_call_id.to_string());
    assert_eq!(
        json["content"],
        format!(
            "Tool Response -> tool_call_id: {}, result: 25°C",
            tool_call_id
        )
    );
}

#[test]
fn tool_response_message_impl_message() {
    let id = Uuid::new_v4();
    let resp = ToolResponseMessage {
        tool_call_id: id,
        result: "42".into(),
    };
    let content = resp.to_prompt_content();
    assert!(content.contains(&id.to_string()));
    assert!(content.contains("42"));
}

#[test]
fn text_message_impl_message() {
    let text = TextMessage {
        text: "test content".into(),
    };
    assert_eq!(text.to_prompt_content(), "test content");
}

#[test]
fn funera_message_serde_roundtrip() {
    let msg = FuneraMessage::new(
        Role::System,
        MsgVariant::Text(TextMessage {
            text: "sys msg".into(),
        }),
    );
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: FuneraMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg.id(), deserialized.id());
    assert_eq!(*msg.role(), *deserialized.role());
    assert_eq!(msg.to_prompt_content(), deserialized.to_prompt_content());
}

#[test]
fn msg_variant_debug_and_clone() {
    let variant = MsgVariant::Text(TextMessage {
        text: "debug".into(),
    });
    let cloned = variant.clone();
    assert_eq!(format!("{:?}", variant), format!("{:?}", cloned));
}

#[test]
fn timestamp_is_utc() {
    let msg = FuneraMessage::new(
        Role::User,
        MsgVariant::Text(TextMessage { text: "t".into() }),
    );
    let ts: &DateTime<Utc> = msg.timestamp();
    let _ = ts.format("%+");
}
