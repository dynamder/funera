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
