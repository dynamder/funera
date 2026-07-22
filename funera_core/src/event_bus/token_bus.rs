use async_openai::error::OpenAIError;
use async_openai::types::stream::StreamResponse;
use futures::StreamExt;
use tokio::sync::broadcast;

use crate::provider::StreamChunkExt;

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
