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
