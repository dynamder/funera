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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_response_construct() {
        let resp = ChatResponse {
            content: "Hello".into(),
            tool_calls: vec![],
            iterations: 1,
            finish_reason: Some("stop".into()),
        };
        assert_eq!(resp.content, "Hello");
        assert_eq!(resp.iterations, 1);
        assert_eq!(resp.finish_reason, Some("stop".into()));
    }

    #[test]
    fn tool_call_info_ok() {
        let info = ToolCallInfo {
            name: "get_weather".into(),
            args: serde_json::json!({"city": "Tokyo"}),
            result: Ok("22°C".into()),
        };
        assert_eq!(info.name, "get_weather");
        assert!(info.result.is_ok());
    }

    #[test]
    fn tool_call_info_err() {
        let info = ToolCallInfo {
            name: "bad_tool".into(),
            args: serde_json::json!({}),
            result: Err("timeout".into()),
        };
        assert!(info.result.is_err());
    }
}
