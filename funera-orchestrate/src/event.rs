use std::sync::Arc;

use serde_json::Value as JsonValue;

use funera_core::event_bus::env_state_bus::EnvStateEvent;
use funera_core::event_bus::react_bus::ReactEvent;
use funera_core::event_bus::token_bus::TokenEvent;

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
    TurnEnd,
    Error(String),
    Done,
}

/// Wraps a raw underlying event from the core event buses.
///
/// Returned by [`Agent::subscribe_raw_events`](crate::Agent::subscribe_raw_events).
///
/// Unlike [`AgentEvent`] which is a curated/translated view, this enum
/// provides direct access to the original [`TokenEvent`], [`ReactEvent`],
/// and [`EnvStateEvent`] as emitted by `funera_core`.
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
    fn agent_event_token() {
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

    // ── RawAgentEvent ──────────────────────────────────────────────

    #[test]
    fn raw_token_wraps_text() {
        let raw = RawAgentEvent::Token(TokenEvent::Text("hello".into()));
        assert!(matches!(
            raw,
            RawAgentEvent::Token(TokenEvent::Text(t)) if t == "hello"
        ));
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
