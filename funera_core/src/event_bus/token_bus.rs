use async_openai::error::OpenAIError;
use async_openai::types::stream::StreamResponse;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::provider::StreamChunkExt;

/// Token usage statistics for one LLM turn.
///
/// Providers report usage on the final streamed chunk (when the request asks
/// for it via `stream_options.include_usage`). The framework only tracks
/// tokens — cost computation is left to callers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Tokens in the request prompt.
    pub prompt_tokens: u32,
    /// Tokens in the generated completion.
    pub completion_tokens: u32,
    /// `prompt_tokens + completion_tokens`.
    pub total_tokens: u32,
    /// Prompt tokens served from the provider's cache.
    ///
    /// Serialized as `prompt_cache_hit_tokens` (DeepSeek's field name), or
    /// mapped from OpenAI's `prompt_tokens_details.cached_tokens`.
    #[serde(default, rename = "prompt_cache_hit_tokens")]
    pub cache_read_tokens: u32,
    /// Prompt tokens written to the provider's cache.
    ///
    /// Serialized as `prompt_cache_miss_tokens` (DeepSeek's field name);
    /// OpenAI does not report cache writes, so this stays `0` there.
    #[serde(default, rename = "prompt_cache_miss_tokens")]
    pub cache_write_tokens: u32,
}

impl TokenUsage {
    /// Map OpenAI's `CompletionUsage` (present on the final streamed chunk)
    /// into the framework's unified usage record.
    pub fn from_openai(usage: &async_openai::types::chat::CompletionUsage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            cache_read_tokens: usage
                .prompt_tokens_details
                .as_ref()
                .and_then(|d| d.cached_tokens)
                .unwrap_or(0),
            cache_write_tokens: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TokenEvent {
    Text(String),
    ToolDelta {
        index: usize,
        call_id: String,
        name: Option<String>,
        args_chunk: Option<String>,
    },
    Finish(async_openai::types::chat::FinishReason),
    Reasoning(String),
    /// Per-turn token usage, emitted once (usually on the final chunk).
    Usage(TokenUsage),
}

pub struct TokenBus<C: StreamChunkExt> {
    token_tx: broadcast::Sender<TokenEvent>,
    raw_response_stream: StreamResponse<C>,
}

impl<C: StreamChunkExt> TokenBus<C> {
    pub fn new(stream: StreamResponse<C>) -> Self {
        let (token_tx, _) = broadcast::channel(50);
        Self {
            token_tx,
            raw_response_stream: stream,
        }
    }

    pub fn with_sender(token_tx: broadcast::Sender<TokenEvent>, stream: StreamResponse<C>) -> Self {
        Self {
            token_tx,
            raw_response_stream: stream,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TokenEvent> {
        self.token_tx.subscribe()
    }

    pub async fn recv(&mut self) -> Option<Result<Vec<TokenEvent>, OpenAIError>> {
        match self.raw_response_stream.next().await {
            None => None,
            Some(Err(e)) => Some(Err(e)),
            Some(Ok(chunk)) => {
                let events = chunk.extract_events();
                for event in &events {
                    self.token_tx.send(event.clone()).ok();
                }
                Some(Ok(events))
            }
        }
    }

    pub async fn send(&self, event: TokenEvent) -> anyhow::Result<usize> {
        self.token_tx.send(event).map_err(|e| anyhow::anyhow!(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::types::chat::CreateChatCompletionStreamResponse;

    fn make_bus() -> TokenBus<CreateChatCompletionStreamResponse> {
        let stream = Box::pin(futures::stream::empty::<
            Result<CreateChatCompletionStreamResponse, async_openai::error::OpenAIError>,
        >());
        TokenBus::new(stream)
    }

    #[tokio::test]
    async fn token_bus_send_and_receive() {
        let bus = make_bus();
        let mut rx = bus.subscribe();

        bus.send(TokenEvent::Text("hello".into())).await.unwrap();
        assert!(matches!(rx.try_recv(), Ok(TokenEvent::Text(t)) if t == "hello"));
    }

    #[tokio::test]
    async fn token_bus_multiple_subscribers() {
        let bus = make_bus();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.send(TokenEvent::Text("broadcast".into()))
            .await
            .unwrap();
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[tokio::test]
    async fn token_bus_event_types() {
        let bus = make_bus();
        let mut rx = bus.subscribe();

        bus.send(TokenEvent::Reasoning("thinking...".into()))
            .await
            .unwrap();
        bus.send(TokenEvent::Finish(
            async_openai::types::chat::FinishReason::Stop,
        ))
        .await
        .unwrap();
        bus.send(TokenEvent::ToolDelta {
            index: 0,
            call_id: "call_1".into(),
            name: Some("tool".into()),
            args_chunk: Some("{}".into()),
        })
        .await
        .unwrap();

        let events: Vec<TokenEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], TokenEvent::Reasoning(_)));
        assert!(matches!(events[1], TokenEvent::Finish(_)));
        assert!(matches!(events[2], TokenEvent::ToolDelta { .. }));
    }

    #[tokio::test]
    async fn token_bus_with_sender() {
        let (tx, _rx) = tokio::sync::broadcast::channel(50);
        let stream = Box::pin(futures::stream::empty::<
            Result<CreateChatCompletionStreamResponse, async_openai::error::OpenAIError>,
        >());
        let bus = TokenBus::<CreateChatCompletionStreamResponse>::with_sender(tx, stream);

        let mut rx = bus.subscribe();
        bus.send(TokenEvent::Text("custom sender".into()))
            .await
            .unwrap();
        assert!(matches!(rx.try_recv(), Ok(TokenEvent::Text(t)) if t == "custom sender"));
    }
}
