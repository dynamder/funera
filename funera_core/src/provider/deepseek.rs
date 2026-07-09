use async_openai::{
    config::OpenAIConfig,
    error::OpenAIError,
    types::{
        chat::{ChatCompletionMessageToolCallChunk, FinishReason, Role},
        stream::StreamResponse,
    },
    Client,
};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::event_bus::token_bus::TokenEvent;
use crate::provider::{build_standard_request_json, ChatProvider, StreamChunkExt};

#[derive(Debug, Deserialize)]
pub struct Delta {
    pub content: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    pub role: Option<Role>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChatCompletionMessageToolCallChunk>>,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    pub id: String,
    pub choices: Vec<Choice>,
    pub created: u32,
    pub model: String,
    #[serde(default)]
    pub system_fingerprint: Option<String>,
    pub object: String,
}

impl StreamChunkExt for StreamChunk {
    fn extract_events(&self) -> Vec<TokenEvent> {
        let mut events = Vec::new();
        for choice in &self.choices {
            if let Some(finish_reason) = choice.finish_reason {
                events.push(TokenEvent::Finish(finish_reason));
            }
            if let Some(ref reasoning) = choice.delta.reasoning_content {
                if !reasoning.is_empty() {
                    events.push(TokenEvent::Reasoning(reasoning.clone()));
                }
            }
            match (choice.delta.content.as_deref(), choice.delta.tool_calls.as_ref()) {
                (Some(text), Some(tool_calls)) => {
                    if !text.is_empty() {
                        events.push(TokenEvent::Text(text.to_string()));
                    }
                    for tc in tool_calls {
                        events.push(TokenEvent::ToolDelta {
                            index: tc.index as usize,
                            call_id: tc.id.clone().unwrap_or_default(),
                            name: tc.function.clone().and_then(|f| f.name),
                            args_chunk: tc.function.clone().and_then(|f| f.arguments),
                        });
                    }
                }
                (Some(text), None) => {
                    if !text.is_empty() {
                        events.push(TokenEvent::Text(text.to_string()));
                    }
                }
                (None, Some(tool_calls)) => {
                    for tc in tool_calls {
                        events.push(TokenEvent::ToolDelta {
                            index: tc.index as usize,
                            call_id: tc.id.clone().unwrap_or_default(),
                            name: tc.function.clone().and_then(|f| f.name),
                            args_chunk: tc.function.clone().and_then(|f| f.arguments),
                        });
                    }
                }
                (None, None) => {}
            }
        }
        events
    }
}

pub struct DeepSeekProvider;

impl ChatProvider for DeepSeekProvider {
    type Chunk = StreamChunk;

    fn build_request_json(
        model: &str,
        messages: &[JsonValue],
        skill_content: &str,
        tools_json: &JsonValue,
    ) -> JsonValue {
        let mut json = build_standard_request_json(model, messages, skill_content, tools_json);

        // DeepSeek requires: consecutive assistant tool_call messages merged into one,
        // and all tool_call messages must have "content": null
        if let Some(msgs) = json["messages"].as_array_mut() {
            let mut merged: Vec<JsonValue> = Vec::with_capacity(msgs.len());

            let mut i = 0;
            while i < msgs.len() {
                let msg = &msgs[i];
                let role = msg["role"].as_str().unwrap_or("");

                if role == "assistant" && msg.get("tool_calls").and_then(|t| t.as_array()).is_some() {
                    let mut combined_calls = Vec::new();
                    let mut reasoning = None;

                    while i < msgs.len() {
                        let cur = &msgs[i];
                        if cur["role"].as_str() != Some("assistant")
                            || cur.get("tool_calls").and_then(|t| t.as_array()).is_none()
                        {
                            break;
                        }
                        if let Some(calls) = cur["tool_calls"].as_array() {
                            combined_calls.extend(calls.iter().cloned());
                        }
                        if reasoning.is_none() {
                            reasoning = cur.get("reasoning_content").and_then(|r| r.as_str()).map(|s| s.to_string());
                        }
                        i += 1;
                    }

                    let mut merged_msg = serde_json::json!({
                        "role": "assistant",
                        "content": null,
                        "tool_calls": combined_calls,
                    });
                    if let Some(rc) = reasoning {
                        merged_msg["reasoning_content"] = serde_json::json!(rc);
                    }
                    merged.push(merged_msg);
                } else {
                    let mut m = msg.clone();
                    if role == "assistant" {
                        if let Some(arr) = m.get("tool_calls").and_then(|t| t.as_array()) {
                            if !arr.is_empty() && !m.as_object().unwrap().contains_key("content") {
                                m.as_object_mut().unwrap().insert("content".into(), JsonValue::Null);
                            }
                        }
                    }
                    merged.push(m);
                    i += 1;
                }
            }

            *msgs = merged;
        }

        json.as_object_mut()
            .unwrap()
            .insert("thinking".into(), serde_json::json!({"type": "enabled"}));
        json
    }

    async fn create_stream(
        client: &Client<OpenAIConfig>,
        request_json: JsonValue,
    ) -> Result<StreamResponse<Self::Chunk>, OpenAIError> {
        client
            .chat()
            .create_stream_byot::<JsonValue, Self::Chunk>(request_json)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_request_merges_consecutive_tool_calls() {
        let msgs = vec![
            json!({"role": "user", "content": "Weather in Tokyo, Beijing, Paris?"}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": r#"{"city":"Tokyo"}"#}}]}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_2", "type": "function", "function": {"name": "get_weather", "arguments": r#"{"city":"Beijing"}"#}}]}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_3", "type": "function", "function": {"name": "get_weather", "arguments": r#"{"city":"Paris"}"#}}]}),
            json!({"role": "tool", "tool_call_id": "call_1", "content": "22°C"}),
            json!({"role": "tool", "tool_call_id": "call_2", "content": "18°C"}),
            json!({"role": "tool", "tool_call_id": "call_3", "content": "25°C"}),
        ];
        let result = DeepSeekProvider::build_request_json("test-model", &msgs, "", &json!([]));
        let msgs_out = result["messages"].as_array().unwrap();
        assert_eq!(msgs_out.len(), 5, "expected 5: user + 1 merged assistant + 3 tool");
        assert_eq!(msgs_out[0]["role"], "user");
        assert_eq!(msgs_out[1]["role"], "assistant");
        let calls = msgs_out[1]["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0]["id"], "call_1");
        assert_eq!(calls[1]["id"], "call_2");
        assert_eq!(calls[2]["id"], "call_3");
        assert!(msgs_out[1]["content"].is_null());
        assert_eq!(msgs_out[2]["role"], "tool");
        assert_eq!(msgs_out[3]["role"], "tool");
        assert_eq!(msgs_out[4]["role"], "tool");
    }

    #[test]
    fn build_request_preserves_reasoning_on_merged() {
        let msgs = vec![
            json!({"role": "user", "content": "Weather in Tokyo?"}),
            json!({"role": "assistant", "reasoning_content": "Let me think...", "content": null, "tool_calls": [{"id": "call_1", "function": {"name": "get_weather", "arguments": r#"{"city":"Tokyo"}"#}}]}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_2", "function": {"name": "get_weather", "arguments": r#"{"city":"Osaka"}"#}}]}),
            json!({"role": "tool", "tool_call_id": "call_1", "content": "22°C"}),
            json!({"role": "tool", "tool_call_id": "call_2", "content": "20°C"}),
            json!({"role": "assistant", "content": "Done."}),
        ];
        let result = DeepSeekProvider::build_request_json("test-model", &msgs, "", &json!([]));
        let msgs_out = result["messages"].as_array().unwrap();
        assert_eq!(msgs_out[1]["reasoning_content"].as_str().unwrap(), "Let me think...");
        assert_eq!(msgs_out[1]["tool_calls"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn build_request_does_not_merge_non_consecutive() {
        let msgs = vec![
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "function": {"name": "t1", "arguments": "{}"}}]}),
            json!({"role": "assistant", "content": "I'll get that."}),
            json!({"role": "assistant", "content": null, "tool_calls": [{"id": "c2", "function": {"name": "t2", "arguments": "{}"}}]}),
        ];
        let result = DeepSeekProvider::build_request_json("test-model", &msgs, "", &json!([]));
        let msgs_out = result["messages"].as_array().unwrap();
        assert_eq!(msgs_out.len(), 4, "non-consecutive should NOT merge");
        let calls0 = msgs_out[1]["tool_calls"].as_array().unwrap();
        assert_eq!(calls0.len(), 1);
        let calls2 = msgs_out[3]["tool_calls"].as_array().unwrap();
        assert_eq!(calls2.len(), 1);
    }

    #[test]
    fn build_request_adds_content_null_to_tool_calls() {
        let msgs = vec![
            json!({"role": "user", "content": "hi"}),
            json!({"role": "assistant", "tool_calls": [{"id": "c1", "function": {"name": "t1", "arguments": "{}"}}]}),
        ];
        let result = DeepSeekProvider::build_request_json("test-model", &msgs, "", &json!([]));
        let msgs_out = result["messages"].as_array().unwrap();
        assert!(msgs_out[1]["content"].is_null());
        assert_eq!(msgs_out[1]["tool_calls"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn build_request_includes_thinking() {
        let msgs = vec![json!({"role": "user", "content": "hi"})];
        let result = DeepSeekProvider::build_request_json("test-model", &msgs, "", &json!([]));
        assert_eq!(result["thinking"]["type"], "enabled");
    }
}
