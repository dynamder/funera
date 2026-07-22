use async_openai::error::OpenAIError;
use async_openai::types::chat::{
    ChatChoiceStream as Choice, ChatCompletionMessageToolCallChunk, ChatCompletionResponseStream,
    ChatCompletionStreamResponseDelta as Delta, CreateChatCompletionStreamResponse as Chunk,
    FinishReason, FunctionCallStream, FunctionType,
};
use futures::stream;

fn base_chunk() -> Chunk {
    #[allow(deprecated)]
    Chunk {
        id: "chatcmpl-test".into(),
        object: "chat.completion.chunk".into(),
        created: 0,
        model: "gpt-4o-mini".into(),
        system_fingerprint: None,
        choices: vec![],
        usage: None,
        service_tier: None,
    }
}

#[allow(deprecated)]
pub fn mock_text_chunk(content: &str) -> Chunk {
    Chunk {
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: Some(content.into()),
                tool_calls: None,
                function_call: None,
                refusal: None,
            },
            finish_reason: None,
            logprobs: None,
        }],
        ..base_chunk()
    }
}

#[allow(deprecated)]
pub fn mock_finish_chunk(reason: FinishReason) -> Chunk {
    Chunk {
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: None,
                tool_calls: None,
                function_call: None,
                refusal: None,
            },
            finish_reason: Some(reason),
            logprobs: None,
        }],
        ..base_chunk()
    }
}

#[allow(deprecated)]
pub fn mock_tool_call_chunk(index: u32, call_id: &str, name: &str, args: &str) -> Chunk {
    Chunk {
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: None,
                tool_calls: Some(vec![ChatCompletionMessageToolCallChunk {
                    index,
                    id: Some(call_id.into()),
                    r#type: Some(FunctionType::Function),
                    function: Some(FunctionCallStream {
                        name: Some(name.into()),
                        arguments: Some(args.into()),
                    }),
                }]),
                function_call: None,
                refusal: None,
            },
            finish_reason: None,
            logprobs: None,
        }],
        ..base_chunk()
    }
}

pub fn mock_text_stream(texts: Vec<&str>) -> ChatCompletionResponseStream {
    let mut chunks: Vec<Result<Chunk, OpenAIError>> =
        texts.into_iter().map(|t| Ok(mock_text_chunk(t))).collect();
    chunks.push(Ok(mock_finish_chunk(FinishReason::Stop)));
    Box::pin(stream::iter(chunks))
}

pub fn mock_tool_stream(name: &str, args: &str, call_id: &str) -> ChatCompletionResponseStream {
    let chunks: Vec<Result<Chunk, OpenAIError>> = vec![
        Ok(mock_tool_call_chunk(0, call_id, name, args)),
        Ok(mock_finish_chunk(FinishReason::ToolCalls)),
    ];
    Box::pin(stream::iter(chunks))
}

pub fn mock_multi_tool_stream(calls: Vec<(u32, &str, &str, &str)>) -> ChatCompletionResponseStream {
    let mut chunks: Vec<Result<Chunk, OpenAIError>> = calls
        .into_iter()
        .map(|(idx, id, name, args)| Ok(mock_tool_call_chunk(idx, id, name, args)))
        .collect();
    chunks.push(Ok(mock_finish_chunk(FinishReason::ToolCalls)));
    Box::pin(stream::iter(chunks))
}

pub fn mock_text_plus_tool_stream(
    text: &str,
    name: &str,
    args: &str,
    call_id: &str,
) -> ChatCompletionResponseStream {
    let chunks: Vec<Result<Chunk, OpenAIError>> = vec![
        Ok(mock_text_chunk(text)),
        Ok(mock_tool_call_chunk(0, call_id, name, args)),
        Ok(mock_finish_chunk(FinishReason::ToolCalls)),
    ];
    Box::pin(stream::iter(chunks))
}

pub fn mock_error_stream() -> ChatCompletionResponseStream {
    let chunks: Vec<Result<Chunk, OpenAIError>> =
        vec![Err(OpenAIError::InvalidArgument("simulated error".into()))];
    Box::pin(stream::iter(chunks))
}

pub fn mock_empty_stream() -> ChatCompletionResponseStream {
    Box::pin(stream::empty())
}
