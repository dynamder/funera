use async_openai::error::OpenAIError;
use async_openai::types::chat::ChatCompletionResponseStream;
use futures::stream::{self};

pub fn empty_stream() -> ChatCompletionResponseStream {
    Box::pin(stream::empty())
}

/// Creates a simple stream that yields a single text chunk and a finish reason.
/// Uses the actual async-openai streaming types.
pub fn single_text_chunk(content: &str) -> ChatCompletionResponseStream {
    use async_openai::types::chat::{
        FinishReason,
        CreateChatCompletionStreamResponse as Chunk,
        ChatCompletionStreamResponseDelta as Delta, ChatChoiceStream as Choice,
    };

    let chunk = Chunk {
        id: "chatcmpl-123".into(),
        object: "chat.completion.chunk".into(),
        created: 0,
        model: "gpt-4o-mini".into(),
        system_fingerprint: None,
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: Some(content.into()),
                tool_calls: None,
                function_call: None,
                refusal: None,
            },
            finish_reason: Some(FinishReason::Stop),
            logprobs: None,
        }],
        usage: None,
        service_tier: None,
    };

    let result: Result<Chunk, OpenAIError> = Ok(chunk);
    Box::pin(stream::once(async move { result }))
}
