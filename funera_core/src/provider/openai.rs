use async_openai::{
    Client,
    config::OpenAIConfig,
    error::OpenAIError,
    types::{chat::CreateChatCompletionStreamResponse, stream::StreamResponse},
};
use serde_json::Value as JsonValue;

use crate::provider::{ChatProvider, ReasoningLevel, build_standard_request_json};

pub struct OpenAIProvider;

impl ChatProvider for OpenAIProvider {
    type Chunk = CreateChatCompletionStreamResponse;

    fn build_request_json(
        model: &str,
        messages: &[JsonValue],
        skill_content: &str,
        tools_json: &JsonValue,
        reasoning_level: ReasoningLevel,
    ) -> JsonValue {
        let mut json = build_standard_request_json(model, messages, skill_content, tools_json);
        match reasoning_level {
            ReasoningLevel::Off => {}
            ReasoningLevel::Minimal => {
                json.as_object_mut()
                    .unwrap()
                    .insert("reasoning_effort".into(), serde_json::json!("minimal"));
            }
            ReasoningLevel::Low => {
                json.as_object_mut()
                    .unwrap()
                    .insert("reasoning_effort".into(), serde_json::json!("low"));
            }
            ReasoningLevel::Medium => {
                json.as_object_mut()
                    .unwrap()
                    .insert("reasoning_effort".into(), serde_json::json!("medium"));
            }
            ReasoningLevel::High => {
                json.as_object_mut()
                    .unwrap()
                    .insert("reasoning_effort".into(), serde_json::json!("high"));
            }
            // OpenAI exposes only up to `high`; clamp the stronger levels.
            ReasoningLevel::XHigh | ReasoningLevel::Max => {
                json.as_object_mut()
                    .unwrap()
                    .insert("reasoning_effort".into(), serde_json::json!("high"));
            }
        }
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
    fn reasoning_effort_mapping() {
        let msgs = vec![json!({"role": "user", "content": "hi"})];
        let tools = json!([]);
        for (level, expected) in [
            (ReasoningLevel::Off, None),
            (ReasoningLevel::Minimal, Some("minimal")),
            (ReasoningLevel::Low, Some("low")),
            (ReasoningLevel::Medium, Some("medium")),
            (ReasoningLevel::High, Some("high")),
            (ReasoningLevel::XHigh, Some("high")),
            (ReasoningLevel::Max, Some("high")),
        ] {
            let req = OpenAIProvider::build_request_json("m", &msgs, "", &tools, level);
            match expected {
                Some(v) => assert_eq!(req["reasoning_effort"], json!(v), "level {level:?}"),
                None => assert!(req.get("reasoning_effort").is_none(), "level {level:?}"),
            }
        }
    }
}
