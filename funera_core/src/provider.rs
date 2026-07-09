use async_openai::{
    config::OpenAIConfig,
    error::OpenAIError,
    types::{
        chat::{CreateChatCompletionStreamResponse, FinishReason},
        stream::StreamResponse,
    },
    Client,
};
use serde::de::DeserializeOwned;
use serde_json::Value as JsonValue;
use std::future::Future;

use crate::event_bus::token_bus::TokenEvent;

#[cfg(feature = "deepseek")]
pub mod deepseek;
#[cfg(feature = "openai")]
pub mod openai;

pub trait StreamChunkExt: DeserializeOwned + Send + 'static {
    fn extract_events(&self) -> Vec<TokenEvent>;
}

pub trait ChatProvider: Send + Sync + 'static {
    type Chunk: StreamChunkExt;

    fn build_request_json(
        model: &str,
        messages: &[JsonValue],
        skill_content: &str,
        tools_json: &JsonValue,
    ) -> JsonValue;

    fn create_stream(
        client: &Client<OpenAIConfig>,
        request_json: JsonValue,
    ) -> impl Future<Output = Result<StreamResponse<Self::Chunk>, OpenAIError>> + Send;
}

// ── OpenAI response chunk impl (always available) ─────────────

impl StreamChunkExt for CreateChatCompletionStreamResponse {
    fn extract_events(&self) -> Vec<TokenEvent> {
        let mut events = Vec::new();
        for choice in &self.choices {
            if let Some(finish_reason) = choice.finish_reason {
                events.push(TokenEvent::Finish(finish_reason));
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

/// Build a standard OpenAI-compatible request JSON.
pub fn build_standard_request_json(
    model: &str,
    messages: &[JsonValue],
    skill_content: &str,
    tools_json: &JsonValue,
) -> JsonValue {
    let mut msgs: Vec<JsonValue> = messages.to_vec();
    if !skill_content.is_empty() {
        msgs.push(JsonValue::Object(
            [("role".into(), "system".into()), ("content".into(), skill_content.into())]
                .into_iter()
                .collect(),
        ));
    }
    let mut req = JsonValue::Object(
        [
            ("model".into(), model.into()),
            ("messages".into(), JsonValue::Array(msgs)),
            ("stream".into(), true.into()),
        ]
        .into_iter()
        .collect(),
    );
    if let Some(arr) = tools_json.as_array() {
        if !arr.is_empty() {
            req.as_object_mut()
                .unwrap()
                .insert("tools".into(), tools_json.clone());
        }
    }
    req
}
