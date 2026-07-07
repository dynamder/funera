use async_openai::{
    error::OpenAIError,
    types::chat::{
        ChatCompletionMessageToolCallChunk, ChatCompletionResponseStream, FinishReason,
    },
};
use futures::StreamExt;
use tokio::sync::broadcast;

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

    pub fn with_sender(
        token_tx: broadcast::Sender<TokenEvent>,
        stream: ChatCompletionResponseStream,
    ) -> Self {
        Self {
            token_tx,
            raw_response_stream: stream,
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

#[cfg(test)]
mod tests {
    use async_openai::types::chat::{ChatCompletionMessageToolCallChunk, FunctionCallStream, FunctionType};

    use crate::test_helpers;

    use super::*;

    #[tokio::test]
    async fn tokenbus_recv_text() {
        let stream = test_helpers::mock_text_stream(vec!["hello"]);
        let mut bus = TokenBus::new(stream);

        let r1 = bus.recv().await;
        assert!(r1.is_some());
        let events1 = r1.unwrap().unwrap();
        assert_eq!(events1.len(), 1);
        assert!(matches!(&events1[0], TokenEvent::Text(t) if t == "hello"));

        let r2 = bus.recv().await;
        assert!(r2.is_some());
        let events2 = r2.unwrap().unwrap();
        assert_eq!(events2.len(), 1);
        assert!(matches!(&events2[0], TokenEvent::Finish(FinishReason::Stop)));
    }

    #[tokio::test]
    async fn tokenbus_recv_tool_delta() {
        let stream = test_helpers::mock_tool_stream("my_tool", r#"{"a":1}"#, "call_1");
        let mut bus = TokenBus::new(stream);

        let r1 = bus.recv().await;
        assert!(r1.is_some());
        let events1 = r1.unwrap().unwrap();
        assert_eq!(events1.len(), 1);
        match &events1[0] {
            TokenEvent::ToolDelta { index, call_id, name, args_chunk } => {
                assert_eq!(*index, 0);
                assert_eq!(call_id.as_deref(), Some("call_1"));
                assert_eq!(name.as_deref(), Some("my_tool"));
                assert_eq!(args_chunk.as_deref(), Some(r#"{"a":1}"#));
            }
            e => panic!("expected ToolDelta, got {:?}", e),
        }

        let r2 = bus.recv().await;
        assert!(r2.is_some());
        let events2 = r2.unwrap().unwrap();
        assert_eq!(events2.len(), 1);
        assert!(matches!(&events2[0], TokenEvent::Finish(FinishReason::ToolCalls)));
    }

    #[tokio::test]
    async fn tokenbus_recv_empty_stream() {
        let stream = test_helpers::mock_empty_stream();
        let mut bus = TokenBus::new(stream);
        let result = bus.recv().await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn tokenbus_recv_error_stream() {
        let stream = test_helpers::mock_error_stream();
        let mut bus = TokenBus::new(stream);
        let result = bus.recv().await;
        assert!(result.is_some());
        assert!(result.unwrap().is_err());
    }

    #[tokio::test]
    async fn tokenbus_send_and_subscribe() {
        let stream = test_helpers::mock_empty_stream();
        let bus = TokenBus::new(stream);
        let mut rx = bus.subscribe();
        bus.send(TokenEvent::Text("hello".into())).await.unwrap();
        let received = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            rx.recv(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(received, TokenEvent::Text(t) if t == "hello"));
    }

    #[tokio::test]
    async fn tokenbus_with_sender() {
        let (tx, _) = broadcast::channel(50);
        let stream = test_helpers::mock_empty_stream();
        let bus = TokenBus::with_sender(tx.clone(), stream);
        let mut rx = tx.subscribe();
        bus.send(TokenEvent::Text("with_sender".into())).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert!(matches!(received, TokenEvent::Text(t) if t == "with_sender"));
    }

    #[test]
    fn tokenbus_parse_tool_calls_partial() {
        let stream = test_helpers::mock_empty_stream();
        let bus = TokenBus::new(stream);
        let chunks = vec![ChatCompletionMessageToolCallChunk {
            index: 0,
            id: Some("call_partial".into()),
            r#type: Some(FunctionType::Function),
            function: Some(FunctionCallStream {
                name: None,
                arguments: Some(r#"{"x""#.into()),
            }),
        }];
        let events: Vec<TokenEvent> = bus.parse_tool_calls(chunks).collect();
        assert_eq!(events.len(), 1);
        match &events[0] {
            TokenEvent::ToolDelta { index, call_id, name, args_chunk } => {
                assert_eq!(*index, 0);
                assert_eq!(call_id.as_deref(), Some("call_partial"));
                assert!(name.is_none());
                assert_eq!(args_chunk.as_deref(), Some(r#"{"x""#));
            }
            e => panic!("expected ToolDelta, got {:?}", e),
        }
    }

    #[test]
    fn tokenbus_parse_tool_calls_no_function() {
        let stream = test_helpers::mock_empty_stream();
        let bus = TokenBus::new(stream);
        let chunks = vec![ChatCompletionMessageToolCallChunk {
            index: 1,
            id: Some("call_no_fn".into()),
            r#type: Some(FunctionType::Function),
            function: None,
        }];
        let events: Vec<TokenEvent> = bus.parse_tool_calls(chunks).collect();
        assert_eq!(events.len(), 1);
        match &events[0] {
            TokenEvent::ToolDelta { index, call_id, name, args_chunk } => {
                assert_eq!(*index, 1);
                assert_eq!(call_id.as_deref(), Some("call_no_fn"));
                assert!(name.is_none());
                assert!(args_chunk.is_none());
            }
            e => panic!("expected ToolDelta, got {:?}", e),
        }
    }
}
