use std::{marker::PhantomData, result};

use async_openai::{
    Client,
    config::OpenAIConfig,
    error::OpenAIError,
    types::chat::{
        ChatCompletionMessageToolCallChunk, ChatCompletionResponseStream,
        CreateChatCompletionStreamResponse, FinishReason,
    },
};
use chrono::format::Item;
use futures::StreamExt;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum TokenEvent {
    Text(String),
    ToolDelta {
        index: usize,
        call_id: Option<String>,
        name: Option<String>,
        args_chunk: Option<String>,
    },
    Finish(FinishReason),
}
pub struct TokenBus {
    token_tx: broadcast::Sender<TokenEvent>,
    raw_response_stream: ChatCompletionResponseStream,
}

impl TokenBus {
    pub fn new(token_receiver: ChatCompletionResponseStream) -> Self {
        let (token_tx, _) = broadcast::channel(50);
        Self {
            token_tx,
            raw_response_stream: token_receiver,
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<TokenEvent> {
        self.token_tx.subscribe()
    }
    pub async fn recv(&mut self) -> Option<Result<Vec<TokenEvent>, OpenAIError>> {
        if let Some(result) = self.raw_response_stream.next().await {
            match result {
                Err(e) => Some(Err(e)),
                Ok(response) => {
                    let mut events = Vec::new();

                    for choice in response.choices {
                        let finish_reason = choice.finish_reason;

                        match (choice.delta.content, choice.delta.tool_calls) {
                            (Some(content), Some(tool_calls)) => {
                                events.push(TokenEvent::Text(content));
                                let tool_event = self.parse_tool_calls(tool_calls);
                                events.extend(tool_event);
                            }
                            (Some(content), None) => {
                                events.push(TokenEvent::Text(content));
                            }
                            (None, Some(tool_calls)) => {
                                let tool_event = self.parse_tool_calls(tool_calls);
                                events.extend(tool_event);
                            }
                            (None, None) => {}
                        };

                        if let Some(finish_reason) = finish_reason {
                            events.push(TokenEvent::Finish(finish_reason));
                        }
                    }
                    Some(Ok(events))
                }
            }
        } else {
            None
        }
    }
    fn parse_tool_calls(
        &self,
        tool_calls_chunks: Vec<ChatCompletionMessageToolCallChunk>,
    ) -> impl Iterator<Item = TokenEvent> {
        tool_calls_chunks.into_iter().map(|chunk| {
            let index = chunk.index as usize;
            if let Some(function_chunk) = chunk.function {
                TokenEvent::ToolDelta {
                    index,
                    call_id: chunk.id,
                    name: function_chunk.name,
                    args_chunk: function_chunk.arguments,
                }
            } else {
                TokenEvent::ToolDelta {
                    index,
                    call_id: chunk.id,
                    name: None,
                    args_chunk: None,
                }
            }
        })
    }
    pub async fn send(&self, event: TokenEvent) -> anyhow::Result<usize> {
        self.token_tx.send(event).map_err(|e| anyhow::anyhow!(e))
    }
}
