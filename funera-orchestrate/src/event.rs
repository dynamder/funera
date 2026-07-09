use std::sync::Arc;

use serde_json::Value as JsonValue;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Token(String),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_token() {
        let e = AgentEvent::Token("hello".into());
        assert!(matches!(e, AgentEvent::Token(t) if t == "hello"));
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
    fn agent_event_turn_boundaries() {
        assert!(matches!(AgentEvent::TurnStart, AgentEvent::TurnStart));
        assert!(matches!(AgentEvent::TurnEnd, AgentEvent::TurnEnd));
        assert!(matches!(AgentEvent::Done, AgentEvent::Done));
    }

    #[test]
    fn agent_event_clone() {
        let e = AgentEvent::Token("hi".into());
        let cloned = e.clone();
        assert!(matches!(cloned, AgentEvent::Token(t) if t == "hi"));
    }
}
