use std::sync::Arc;

use serde_json::Value as JsonValue;

use funera_core::chat::message::{
    MsgVariant, Role, TextMessage, ToolRequestMessage, ToolResponseMessage,
};
use funera_core::event_bus::env_state_bus::EnvStateEvent;
use funera_core::event_bus::react_bus::ReactEvent;
use funera_core::event_bus::token_bus::TokenEvent;
use funera_core::middleware::MiddlewareEvent;
use funera_core::re_act::tool::ToolType;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Text(String),
    Reasoning(String),
    ToolCallRequest {
        index: usize,
        call_id: Arc<str>,
        name: String,
        args: JsonValue,
    },
    ToolCallResult {
        call_id: Arc<str>,
        name: String,
        result: Result<String, String>,
    },
    TurnStart,
    TurnEnd {
        finish_reason: Option<String>,
    },
    Error(String),
    Done,
}

impl MiddlewareEvent for AgentEvent {
    type Error = String;

    fn assistant_text(content: String, _reasoning: Option<String>) -> Self {
        AgentEvent::Text(content)
    }

    fn tool_call_request(call_id: Arc<str>, name: String, args: JsonValue) -> Self {
        AgentEvent::ToolCallRequest {
            index: 0,
            call_id,
            name,
            args,
        }
    }

    fn tool_response(call_id: Arc<str>, name: String, result: Result<String, String>) -> Self {
        AgentEvent::ToolCallResult {
            call_id,
            name,
            result,
        }
    }

    fn turn_start() -> Self {
        AgentEvent::TurnStart
    }

    fn turn_end(finish_reason: Option<String>) -> Self {
        AgentEvent::TurnEnd { finish_reason }
    }

    fn done() -> Self {
        AgentEvent::Done
    }

    fn into_session_message(self) -> Option<(Role, MsgVariant)> {
        match self {
            AgentEvent::Text(text) => Some((
                Role::Assistant,
                MsgVariant::Text(TextMessage {
                    text: text.into(),
                    reasoning_content: None,
                }),
            )),
            AgentEvent::ToolCallRequest {
                call_id,
                name,
                args,
                ..
            } => Some((
                Role::Assistant,
                MsgVariant::ToolRequest(ToolRequestMessage {
                    tool_call_id: call_id,
                    tool_type: ToolType::Function,
                    function_name: name.into(),
                    function_args: args,
                    reasoning_content: None,
                }),
            )),
            AgentEvent::ToolCallResult {
                call_id,
                name: _,
                result,
            } => Some((
                Role::Tool,
                MsgVariant::ToolResponse(ToolResponseMessage {
                    tool_call_id: call_id,
                    result: result.unwrap_or_default().into(),
                }),
            )),
            _ => None,
        }
    }
}

/// Wraps a raw underlying event from the core event buses.
///
/// Returned by [`Agent::subscribe_raw_events`](crate::Agent::subscribe_raw_events).
#[derive(Debug, Clone)]
pub enum RawAgentEvent {
    Token(TokenEvent),
    React(ReactEvent),
    EnvState(EnvStateEvent),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_text() {
        let e = AgentEvent::Text("hello".into());
        assert!(matches!(e, AgentEvent::Text(t) if t == "hello"));
    }

    #[test]
    fn agent_event_tool_call_start() {
        let e = AgentEvent::ToolCallRequest {
            index: 0,
            call_id: "call_abc".into(),
            name: "test".into(),
            args: serde_json::json!({"x": 1}),
        };
        assert!(matches!(e, AgentEvent::ToolCallRequest { name, .. } if name == "test"));
    }

    #[test]
    fn agent_event_tool_call_result_ok() {
        let e = AgentEvent::ToolCallResult {
            call_id: "call_abc".into(),
            name: "test".into(),
            result: Ok("done".into()),
        };
        assert!(matches!(e, AgentEvent::ToolCallResult { .. }));
    }

    #[test]
    fn agent_event_tool_call_result_err() {
        let e = AgentEvent::ToolCallResult {
            call_id: "call_abc".into(),
            name: "test".into(),
            result: Err("fail".into()),
        };
        assert!(matches!(
            e,
            AgentEvent::ToolCallResult { result: Err(_), .. }
        ));
    }

    #[test]
    fn agent_event_clone() {
        let e = AgentEvent::Text("hi".into());
        let cloned = e.clone();
        assert!(matches!(cloned, AgentEvent::Text(t) if t == "hi"));
    }

    #[test]
    fn agent_event_turn_end() {
        let e = AgentEvent::TurnEnd {
            finish_reason: Some("Stop".into()),
        };
        assert!(matches!(e, AgentEvent::TurnEnd { finish_reason: Some(ref r) } if r == "Stop"));
    }

    #[test]
    fn middleware_event_into_session() {
        let event: AgentEvent = MiddlewareEvent::assistant_text("hi".into(), None);
        let (role, variant) = event.into_session_message().unwrap();
        assert_eq!(role.to_string(), "assistant");
        assert!(matches!(variant, MsgVariant::Text(_)));
    }

    #[test]
    fn raw_token_wraps_text() {
        let raw = RawAgentEvent::Token(TokenEvent::Text("hello".into()));
        assert!(matches!(raw, RawAgentEvent::Token(TokenEvent::Text(t)) if t == "hello"));
    }

    #[test]
    fn raw_react_wraps_turn_start() {
        let raw = RawAgentEvent::React(ReactEvent::TurnStart);
        assert!(matches!(raw, RawAgentEvent::React(ReactEvent::TurnStart)));
    }

    #[test]
    fn raw_env_state_wraps_session_start() {
        let raw = RawAgentEvent::EnvState(EnvStateEvent::SessionStart);
        assert!(matches!(
            raw,
            RawAgentEvent::EnvState(EnvStateEvent::SessionStart)
        ));
    }

    #[test]
    fn raw_clone() {
        let raw = RawAgentEvent::Token(TokenEvent::Text("x".into()));
        let cloned = raw.clone();
        assert!(matches!(cloned, RawAgentEvent::Token(TokenEvent::Text(t)) if t == "x"));
    }

    #[test]
    fn raw_debug() {
        let raw = RawAgentEvent::Token(TokenEvent::Text("x".into()));
        let _ = format!("{raw:?}");
    }
}
