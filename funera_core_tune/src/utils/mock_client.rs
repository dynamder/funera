use async_openai::error::OpenAIError;
use async_openai::types::chat::ChatCompletionResponseStream;
use futures::stream;

pub fn empty_stream() -> ChatCompletionResponseStream {
    Box::pin(stream::empty())
}

/// Creates a simple stream that yields a single text chunk and a finish reason.
pub fn single_text_chunk(content: &str) -> ChatCompletionResponseStream {
    use async_openai::types::chat::{
        FinishReason, CreateChatCompletionStreamResponse as Chunk,
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

/// Creates a stream with tool call delta chunks + FinishReason::ToolCalls.
pub fn tool_call_stream(name: &str, args: &str, call_id: &str) -> ChatCompletionResponseStream {
    use async_openai::types::chat::{
        FinishReason, CreateChatCompletionStreamResponse as Chunk,
        ChatCompletionStreamResponseDelta as Delta, ChatChoiceStream as Choice,
        ChatCompletionMessageToolCallChunk, FunctionCallStream, FunctionType,
    };

    let tool_chunk = Chunk {
        id: "chatcmpl-tool".into(),
        object: "chat.completion.chunk".into(),
        created: 0,
        model: "gpt-4o-mini".into(),
        system_fingerprint: None,
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: None,
                tool_calls: Some(vec![ChatCompletionMessageToolCallChunk {
                    index: 0,
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
        usage: None,
        service_tier: None,
    };

    let finish_chunk = Chunk {
        id: "chatcmpl-tool-fin".into(),
        object: "chat.completion.chunk".into(),
        created: 0,
        model: "gpt-4o-mini".into(),
        system_fingerprint: None,
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: None,
                tool_calls: None,
                function_call: None,
                refusal: None,
            },
            finish_reason: Some(FinishReason::ToolCalls),
            logprobs: None,
        }],
        usage: None,
        service_tier: None,
    };

    let results: Vec<Result<Chunk, OpenAIError>> = vec![Ok(tool_chunk), Ok(finish_chunk)];
    Box::pin(stream::iter(results))
}

/// Creates a stream with multiple tool calls in one turn.
pub fn multi_tool_stream(
    calls: Vec<(u32, &str, &str, &str)>,
) -> ChatCompletionResponseStream {
    use async_openai::types::chat::{
        FinishReason, CreateChatCompletionStreamResponse as Chunk,
        ChatCompletionStreamResponseDelta as Delta, ChatChoiceStream as Choice,
        ChatCompletionMessageToolCallChunk, FunctionCallStream, FunctionType,
    };

    let mut chunks: Vec<Chunk> = calls
        .into_iter()
        .map(|(idx, id, name, args)| Chunk {
            id: format!("chatcmpl-tool-{}", idx),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "gpt-4o-mini".into(),
            system_fingerprint: None,
            choices: vec![Choice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![ChatCompletionMessageToolCallChunk {
                        index: idx,
                        id: Some(id.into()),
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
            usage: None,
            service_tier: None,
        })
        .collect();

    chunks.push(Chunk {
        id: "chatcmpl-fin".into(),
        object: "chat.completion.chunk".into(),
        created: 0,
        model: "gpt-4o-mini".into(),
        system_fingerprint: None,
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: None,
                tool_calls: None,
                function_call: None,
                refusal: None,
            },
            finish_reason: Some(FinishReason::ToolCalls),
            logprobs: None,
        }],
        usage: None,
        service_tier: None,
    });

    let results: Vec<Result<Chunk, OpenAIError>> = chunks.into_iter().map(Ok).collect();
    Box::pin(stream::iter(results))
}

/// Creates a stream with text followed by tool call.
pub fn text_plus_tool_stream(
    text: &str,
    name: &str,
    args: &str,
    call_id: &str,
) -> ChatCompletionResponseStream {
    use async_openai::types::chat::{
        FinishReason, CreateChatCompletionStreamResponse as Chunk,
        ChatCompletionStreamResponseDelta as Delta, ChatChoiceStream as Choice,
        ChatCompletionMessageToolCallChunk, FunctionCallStream, FunctionType,
    };

    let text_chunk = Chunk {
        id: "chatcmpl-1".into(),
        object: "chat.completion.chunk".into(),
        created: 0,
        model: "gpt-4o-mini".into(),
        system_fingerprint: None,
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: Some(text.into()),
                tool_calls: None,
                function_call: None,
                refusal: None,
            },
            finish_reason: None,
            logprobs: None,
        }],
        usage: None,
        service_tier: None,
    };

    let tool_chunk = Chunk {
        id: "chatcmpl-2".into(),
        object: "chat.completion.chunk".into(),
        created: 0,
        model: "gpt-4o-mini".into(),
        system_fingerprint: None,
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: None,
                tool_calls: Some(vec![ChatCompletionMessageToolCallChunk {
                    index: 0,
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
        usage: None,
        service_tier: None,
    };

    let finish_chunk = Chunk {
        id: "chatcmpl-3".into(),
        object: "chat.completion.chunk".into(),
        created: 0,
        model: "gpt-4o-mini".into(),
        system_fingerprint: None,
        choices: vec![Choice {
            index: 0,
            delta: Delta {
                role: None,
                content: None,
                tool_calls: None,
                function_call: None,
                refusal: None,
            },
            finish_reason: Some(FinishReason::ToolCalls),
            logprobs: None,
        }],
        usage: None,
        service_tier: None,
    };

    let results: Vec<Result<Chunk, OpenAIError>> =
        vec![Ok(text_chunk), Ok(tool_chunk), Ok(finish_chunk)];
    Box::pin(stream::iter(results))
}

/// Creates a stream that yields an OpenAIError immediately.
pub fn error_stream() -> ChatCompletionResponseStream {
    let err: Result<
        async_openai::types::chat::CreateChatCompletionStreamResponse,
        OpenAIError,
    > = Err(OpenAIError::InvalidArgument("simulated error".into()));
    Box::pin(stream::once(async move { err }))
}
