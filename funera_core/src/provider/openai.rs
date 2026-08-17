use async_openai::{
    Client,
    config::OpenAIConfig,
    error::OpenAIError,
    types::{chat::CreateChatCompletionStreamResponse, stream::StreamResponse},
};
use serde_json::Value as JsonValue;

use crate::provider::{ChatProvider, build_standard_request_json};

pub struct OpenAIProvider;

impl ChatProvider for OpenAIProvider {
    type Chunk = CreateChatCompletionStreamResponse;

    fn build_request_json(
        model: &str,
        messages: &[JsonValue],
        skill_content: &str,
        tools_json: &JsonValue,
    ) -> JsonValue {
        build_standard_request_json(model, messages, skill_content, tools_json)
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
