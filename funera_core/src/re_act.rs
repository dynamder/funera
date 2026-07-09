use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionMessageToolCalls, ChatCompletionRequestAssistantMessageArgs,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestToolMessageArgs, ChatCompletionRequestUserMessageArgs, ChatCompletionTools,
    CreateChatCompletionRequest, CreateChatCompletionRequestArgs, FinishReason,
};
use serde_json::Value as JsonValue;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::chat::message::{
    FuneraMessage, MsgVariant, Role, TextMessage, ToolRequestMessage, ToolResponseMessage,
};
use crate::env::FuneraEnvWatcher;
use crate::event_bus::env_state_bus::{EnvStateEvent, TurnHighWayHandle};
use crate::event_bus::react_bus::{ReactBus, ReactEvent, ToolCallRequest, ToolCallResponse};
use crate::event_bus::token_bus::{TokenBus, TokenEvent};
use crate::event_bus::tool_bus::ToolBus;
use crate::re_act::tool::{ToolCallError, ToolType};

pub mod skills;
pub mod tool;
pub mod tool_executor;

pub struct ReActLoopConfig {
    pub buffer: usize,
    pub max_iteration: usize,
    pub env_watcher: FuneraEnvWatcher,
    pub tool_bus: ToolBus,
    pub env_state_tx: broadcast::Sender<EnvStateEvent>,
    pub turn_highway_handle: TurnHighWayHandle,
    pub session_msgs: Option<Arc<parking_lot::RwLock<Vec<FuneraMessage>>>>,
}

impl ReActLoopConfig {
    pub fn new(
        buffer: usize,
        max_iteration: usize,
        env_watcher: FuneraEnvWatcher,
        tool_bus: ToolBus,
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        turn_highway_handle: TurnHighWayHandle,
    ) -> Self {
        Self {
            buffer,
            max_iteration,
            env_watcher,
            tool_bus,
            env_state_tx,
            turn_highway_handle,
            session_msgs: None,
        }
    }
}

pub struct ReActLoop {
    buf_msg_rx: mpsc::Receiver<FuneraMessage>,
    buf_msg_tx: mpsc::Sender<FuneraMessage>,
    session_msgs: Arc<parking_lot::RwLock<Vec<FuneraMessage>>>,
    max_iteration: usize,
    env_watcher: FuneraEnvWatcher,
    tool_bus: ToolBus,
    env_state_tx: broadcast::Sender<EnvStateEvent>,
    turn_highway_handle: TurnHighWayHandle,
}

impl ReActLoop {
    pub fn new(
        buffer: usize,
        max_iteration: usize,
        session_msgs: Arc<parking_lot::RwLock<Vec<FuneraMessage>>>,
        env_watcher: FuneraEnvWatcher,
        tool_bus: ToolBus,
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        turn_highway_handle: TurnHighWayHandle,
    ) -> Self {
        let (tx, rx) = mpsc::channel(buffer);
        Self {
            buf_msg_rx: rx,
            buf_msg_tx: tx,
            session_msgs,
            max_iteration,
            env_watcher,
            tool_bus,
            env_state_tx,
            turn_highway_handle,
        }
    }

    pub fn from_config(config: ReActLoopConfig) -> Self {
        let session_msgs = config.session_msgs.unwrap_or_default();
        Self::new(
            config.buffer,
            config.max_iteration,
            session_msgs,
            config.env_watcher,
            config.tool_bus,
            config.env_state_tx,
            config.turn_highway_handle,
        )
    }

    fn build_history_from(msgs: &Arc<parking_lot::RwLock<Vec<FuneraMessage>>>) -> Vec<JsonValue> {
        msgs.read().iter().map(|msg| msg.format_json()).collect()
    }

    pub fn sender(&self) -> mpsc::Sender<FuneraMessage> {
        self.buf_msg_tx.clone()
    }

    pub fn run(mut self) -> ReActLoopHandle {
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let sender = self.sender();

        let task = tokio::spawn(async move {
            let mut iteration = 0;
            let mut env_watcher = self.env_watcher;

            while iteration < self.max_iteration {
                // 1. Receive and batch pending messages
                let mut msgs = Vec::new();
                msgs.push(match self.buf_msg_rx.recv().await {
                    Some(msg) => msg,
                    None => break,
                });
                while let Ok(msg) = self.buf_msg_rx.try_recv() {
                    msgs.push(msg);
                }

                // Sync incoming messages to session
                for msg in &msgs {
                    self.session_msgs.write().push(msg.clone());
                }

                // 2. Get current env state (client, tools, model, skills)
                let client = env_watcher.watch_client();
                let tools_json = env_watcher.watch_tool();
                let model = env_watcher.watch_model();
                let skill_content = env_watcher.watch_skill();

                // 3. Build history from session (single source of truth)
                let history_json = Self::build_history_from(&self.session_msgs);

                // 4. TurnHighWay handshake
                let (token_tx, react_bus) = self.turn_highway_handle.prepare_turn().await;

                // 5. Format messages for LLM (with skill content appended after system prompts)
                let llm_messages = build_llm_messages(&history_json, &skill_content)?;

                // 6. Build the API request
                let request = build_chat_request(&model, llm_messages, &tools_json)?;

                // 7. Call LLM streaming
                react_bus.send(ReactEvent::TurnStart).ok();
                let stream = client.chat().create_stream(request).await?;

                // 8. Process stream
                let mut token_bus = TokenBus::with_sender(token_tx, stream);
                let (assistant_content, tool_call_accums, turn_finish_reason) =
                    process_token_stream(&mut token_bus, &react_bus).await?;

                // 9. Handle finish reason
                let should_continue = handle_turn_finish(
                    turn_finish_reason.as_ref(),
                    &assistant_content,
                    &tool_call_accums,
                    &react_bus,
                    &self.tool_bus,
                    &self.buf_msg_tx,
                    &self.session_msgs,
                )
                .await?;

                if react_bus.send(ReactEvent::TurnEnd).is_err() {
                    eprintln!("warn: TurnEnd broadcast failed — no subscribers");
                }

                if !should_continue {
                    break;
                }

                iteration += 1;
            }

            Ok(())
        });

        ReActLoopHandle {
            cancel_token: token_clone,
            task,
            sender,
        }
    }
}

fn build_llm_messages(
    history: &[JsonValue],
    skill_content: &str,
) -> Result<Vec<ChatCompletionRequestMessage>> {
    let mut messages = Vec::new();
    for entry in history {
        let role = entry["role"].as_str().unwrap_or("user");
        match role {
            "user" => {
                let content = entry["content"].as_str().unwrap_or("");
                let msg = ChatCompletionRequestMessage::User(
                    ChatCompletionRequestUserMessageArgs::default()
                        .content(content)
                        .build()?,
                );
                messages.push(msg);
            }
            "assistant" => {
                if let Some(tool_calls) = entry.get("tool_calls").and_then(|t| t.as_array()) {
                    let tool_calls: Vec<ChatCompletionMessageToolCalls> =
                        serde_json::from_value(JsonValue::Array(tool_calls.clone()))
                            .unwrap_or_default();
                    messages.push(ChatCompletionRequestMessage::Assistant(
                        ChatCompletionRequestAssistantMessageArgs::default()
                            .tool_calls(tool_calls)
                            .build()?,
                    ));
                } else {
                    let content = entry["content"].as_str().unwrap_or("");
                    messages.push(ChatCompletionRequestMessage::Assistant(
                        ChatCompletionRequestAssistantMessageArgs::default()
                            .content(content)
                            .build()?,
                    ));
                }
            }
            "tool" => {
                let tool_call_id = entry["tool_call_id"].as_str().unwrap_or("");
                let content = entry["content"].as_str().unwrap_or("");
                let msg = ChatCompletionRequestMessage::Tool(
                    ChatCompletionRequestToolMessageArgs::default()
                        .tool_call_id(tool_call_id)
                        .content(content)
                        .build()?,
                );
                messages.push(msg);
            }
            "system" => {
                let content = entry["content"].as_str().unwrap_or("");
                messages.push(ChatCompletionRequestMessage::System(
                    ChatCompletionRequestSystemMessageArgs::default()
                        .content(content)
                        .build()?,
                ));
            }
            _ => {}
        }
    }

    // Append active skill content as additional system message(s) after existing system prompts
    if !skill_content.is_empty() {
        messages.push(ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessageArgs::default()
                .content(skill_content)
                .build()?,
        ));
    }

    Ok(messages)
}

fn build_chat_request(
    model: &str,
    messages: Vec<ChatCompletionRequestMessage>,
    tools_json: &JsonValue,
) -> Result<CreateChatCompletionRequest> {
    let mut builder = CreateChatCompletionRequestArgs::default();
    builder.model(model);
    builder.messages(messages);
    builder.stream(true);

    if let Some(tools_array) = tools_json.as_array() {
        if !tools_array.is_empty() {
            let chat_tools: Vec<ChatCompletionTools> =
                serde_json::from_value(tools_json.clone()).unwrap_or_default();
            if !chat_tools.is_empty() {
                builder.tools(chat_tools);
            }
        }
    }

    Ok(builder.build()?)
}

async fn process_token_stream(
    token_bus: &mut TokenBus,
    react_bus: &ReactBus,
) -> Result<(
    String,
    HashMap<usize, ToolCallAccumulator>,
    Option<FinishReason>,
)> {
    let mut assistant_content = String::new();
    let mut tool_call_accums: HashMap<usize, ToolCallAccumulator> = HashMap::new();
    let mut turn_finish_reason: Option<FinishReason> = None;

    while let Some(result) = token_bus.recv().await {
        let events = result?;
        for event in events {
            match event {
                TokenEvent::Text(t) => {
                    assistant_content.push_str(&t);
                    let text_msg = FuneraMessage::new(
                        Role::Assistant,
                        MsgVariant::Text(TextMessage {
                            text: t.clone().into(),
                        }),
                    );
                    react_bus.send(ReactEvent::MessageQueued(text_msg)).ok();
                }
                TokenEvent::ToolDelta {
                    index,
                    call_id,
                    name,
                    args_chunk,
                } => {
                    let acc =
                        tool_call_accums
                            .entry(index)
                            .or_insert_with(|| ToolCallAccumulator {
                                index,
                                call_id: String::new(),
                                name: String::new(),
                                args: String::new(),
                            });
                    if let Some(id) = call_id {
                        acc.call_id = id;
                    }
                    if let Some(n) = name {
                        acc.name = n;
                    }
                    if let Some(chunk) = args_chunk {
                        acc.args.push_str(&chunk);
                    }
                }
                TokenEvent::Finish(reason) => {
                    turn_finish_reason = Some(reason);
                }
            }
        }
    }

    Ok((assistant_content, tool_call_accums, turn_finish_reason))
}

async fn handle_turn_finish(
    finish_reason: Option<&FinishReason>,
    assistant_content: &str,
    tool_call_accums: &HashMap<usize, ToolCallAccumulator>,
    react_bus: &ReactBus,
    tool_bus: &ToolBus,
    buf_msg_tx: &mpsc::Sender<FuneraMessage>,
    session_msgs: &Arc<parking_lot::RwLock<Vec<FuneraMessage>>>,
) -> Result<bool> {
    match finish_reason {
        None | Some(FinishReason::Stop) => {
            if !assistant_content.is_empty() {
                session_msgs.write().push(FuneraMessage::new(
                    Role::Assistant,
                    MsgVariant::Text(TextMessage {
                        text: assistant_content.into(),
                    }),
                ));
            }
            Ok(false)
        }

        Some(FinishReason::ToolCalls) | Some(FinishReason::Length) => {
            let mut accums: Vec<_> = tool_call_accums.values().collect();
            accums.sort_by_key(|a| a.index);

            // Sync the assistant tool call messages to session
            let mut acc_sorted = accums.clone();
            acc_sorted.sort_by_key(|a| a.index);
            for acc in &acc_sorted {
                let args: JsonValue = serde_json::from_str(&acc.args).unwrap_or(JsonValue::Null);
                session_msgs.write().push(FuneraMessage::new(
                    Role::Assistant,
                    MsgVariant::ToolRequest(ToolRequestMessage {
                        tool_call_id: Uuid::parse_str(&acc.call_id).unwrap_or_default(),
                        tool_type: ToolType::Function,
                        function_name: acc.name.clone().into(),
                        function_args: args,
                    }),
                ));
            }

            // Broadcast all requests
            for acc in &accums {
                let args: JsonValue = serde_json::from_str(&acc.args).unwrap_or(JsonValue::Null);
                react_bus
                    .send(ReactEvent::ToolExecRequest(ToolCallRequest {
                        index: acc.index,
                        call_id: acc.call_id.clone(),
                        name: acc.name.clone(),
                        args,
                    }))
                    .ok();
            }

            // Execute all tools in parallel
            use futures::future::join_all;
            let results: Vec<Result<String, ToolCallError>> = join_all(accums.iter().map(|acc| {
                let args: JsonValue = serde_json::from_str(&acc.args).unwrap_or(JsonValue::Null);
                tool_bus.execute(acc.call_id.clone(), acc.name.clone(), args)
            }))
            .await;

            // Handle results in order
            for (acc, result) in accums.iter().zip(results.iter()) {
                match result {
                    Ok(response) => {
                        react_bus
                            .send(ReactEvent::ToolExecResponse(Ok(ToolCallResponse {
                                call_id: acc.call_id.clone(),
                                result: response.clone(),
                            })))
                            .ok();

                        let tool_response_msg = FuneraMessage::new(
                            Role::Tool,
                            MsgVariant::ToolResponse(ToolResponseMessage {
                                tool_call_id: Uuid::parse_str(&acc.call_id).unwrap_or_default(),
                                result: response.clone().into(),
                            }),
                        );
                        buf_msg_tx.send(tool_response_msg).await?;
                    }
                    Err(e) => {
                        react_bus
                            .send(ReactEvent::ToolExecResponse(Err(e.to_string())))
                            .ok();
                    }
                }
            }

            Ok(true)
        }

        _ => Ok(false),
    }
}

#[derive(Debug)]
struct ToolCallAccumulator {
    index: usize,
    call_id: String,
    name: String,
    args: String,
}

#[derive(Debug)]
pub struct ReActLoopHandle {
    pub cancel_token: CancellationToken,
    pub task: JoinHandle<Result<()>>,
    pub sender: mpsc::Sender<FuneraMessage>,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_openai::types::chat::FinishReason;
    use serde_json::json;
    use tokio::sync::mpsc;

    use crate::event_bus::react_bus::ReactBus;
    use crate::event_bus::tool_bus::ToolBus;
    use crate::test_helpers;

    use super::*;

    // ── build_llm_messages ───────────────────────────────────────

    #[test]
    fn build_msgs_empty_history() {
        let msgs = build_llm_messages(&[], "").unwrap();
        assert!(msgs.is_empty());
    }

    #[test]
    fn build_msgs_user() {
        let history = vec![json!({"role": "user", "content": "hello"})];
        let msgs = build_llm_messages(&history, "").unwrap();
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn build_msgs_system_user() {
        let history = vec![
            json!({"role": "system", "content": "be helpful"}),
            json!({"role": "user", "content": "hello"}),
        ];
        let msgs = build_llm_messages(&history, "").unwrap();
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn build_msgs_all_roles() {
        let history = vec![
            json!({"role": "system", "content": "sys"}),
            json!({"role": "user", "content": "usr"}),
            json!({"role": "assistant", "content": "asst"}),
            json!({"role": "tool", "tool_call_id": "call_1", "content": "tool result"}),
        ];
        let msgs = build_llm_messages(&history, "").unwrap();
        assert_eq!(msgs.len(), 4);
        let debug_strs: Vec<String> = msgs.iter().map(|m| format!("{:?}", m)).collect();
        assert!(
            debug_strs[0].starts_with("System"),
            "got: {}",
            debug_strs[0]
        );
        assert!(debug_strs[1].starts_with("User"), "got: {}", debug_strs[1]);
        assert!(
            debug_strs[2].starts_with("Assistant"),
            "got: {}",
            debug_strs[2]
        );
        assert!(debug_strs[3].starts_with("Tool"), "got: {}", debug_strs[3]);
    }

    #[test]
    fn build_msgs_with_skill_content_appended() {
        let history = vec![
            json!({"role": "system", "content": "You are a bot"}),
            json!({"role": "user", "content": "hello"}),
        ];
        let msgs = build_llm_messages(&history, "Skill instruction here").unwrap();
        assert_eq!(msgs.len(), 3);
        let debug_strs: Vec<String> = msgs.iter().map(|m| format!("{:?}", m)).collect();
        assert!(
            debug_strs[0].starts_with("System"),
            "got: {}",
            debug_strs[0]
        );
        assert!(debug_strs[1].starts_with("User"), "got: {}", debug_strs[1]);
        assert!(
            debug_strs[2].starts_with("System"),
            "got: {}",
            debug_strs[2]
        );
    }

    #[test]
    fn build_msgs_skill_content_empty_noop() {
        let history = vec![json!({"role": "user", "content": "hi"})];
        let msgs = build_llm_messages(&history, "").unwrap();
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn build_msgs_assistant_with_tool_calls() {
        let history = vec![
            json!({"role": "user", "content": "check weather"}),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_abc",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": r#"{"city":"Tokyo"}"#
                    }
                }]
            }),
            json!({"role": "tool", "tool_call_id": "call_abc", "content": "22°C"}),
        ];
        let msgs = build_llm_messages(&history, "").unwrap();
        assert_eq!(msgs.len(), 3);
        // Tool calls must not produce empty assistant — serialized form should contain tool_calls
        let debug = format!("{:?}", msgs[1]);
        assert!(debug.contains("tool_calls"), "expected tool_calls in assistant msg: {debug}");
    }

    #[test]
    fn build_msgs_unknown_role_skipped() {
        let history = vec![json!({"role": "unknown_role", "content": "???", "extra": "field"})];
        let msgs = build_llm_messages(&history, "").unwrap();
        assert_eq!(msgs.len(), 0);
    }

    #[test]
    fn build_msgs_missing_role_defaults_to_user() {
        let history = vec![json!({"content": "no role"})];
        let msgs = build_llm_messages(&history, "").unwrap();
        assert_eq!(msgs.len(), 1);
    }

    // ── build_chat_request ──────────────────────────────────────

    #[test]
    fn build_request_no_tools() {
        let msgs = build_llm_messages(&[json!({"role": "user", "content": "hi"})], "").unwrap();
        let tools_json = json!([]);
        let req = build_chat_request("gpt-4o", msgs, &tools_json).unwrap();
        assert!(req.tools.is_none() || req.tools.as_ref().map(|t| t.is_empty()).unwrap_or(true));
    }

    #[test]
    fn build_request_with_tools() {
        let msgs = build_llm_messages(&[json!({"role": "user", "content": "hi"})], "").unwrap();
        let tools_json = json!([{
            "type": "function",
            "function": {
                "name": "test_tool",
                "description": "A test",
                "parameters": {"type": "object", "properties": {}, "required": []}
            }
        }]);
        let req = build_chat_request("gpt-4o", msgs, &tools_json).unwrap();
        assert!(req.tools.is_some());
        assert!(req.tools.unwrap().len() > 0);
    }

    #[test]
    fn build_request_tools_not_array() {
        let msgs = build_llm_messages(&[json!({"role": "user", "content": "hi"})], "").unwrap();
        let tools_json = json!("not_an_array");
        let req = build_chat_request("gpt-4o", msgs, &tools_json).unwrap();
        assert!(req.tools.is_none());
    }

    #[test]
    fn build_request_empty_tools_array() {
        let msgs = build_llm_messages(&[json!({"role": "user", "content": "hi"})], "").unwrap();
        let tools_json = json!([]);
        let req = build_chat_request("gpt-4o", msgs, &tools_json).unwrap();
        assert!(req.tools.is_none() || req.tools.as_ref().map(|t| t.is_empty()).unwrap_or(true));
    }

    // ── process_token_stream ────────────────────────────────────

    #[tokio::test]
    async fn process_stream_empty() {
        let (tx, _) = tokio::sync::broadcast::channel(50);
        let stream = test_helpers::mock_empty_stream();
        let mut token_bus = TokenBus::with_sender(tx, stream);
        let react_bus = ReactBus::new();

        let (content, accums, reason) = process_token_stream(&mut token_bus, &react_bus)
            .await
            .unwrap();
        assert_eq!(content, "");
        assert!(accums.is_empty());
        assert!(reason.is_none());
    }

    #[tokio::test]
    async fn process_stream_text() {
        let (tx, _) = tokio::sync::broadcast::channel(50);
        let stream = test_helpers::mock_text_stream(vec!["Hello", " ", "World"]);
        let mut token_bus = TokenBus::with_sender(tx, stream);
        let react_bus = ReactBus::new();

        let (content, accums, reason) = process_token_stream(&mut token_bus, &react_bus)
            .await
            .unwrap();
        assert_eq!(content, "Hello World");
        assert!(accums.is_empty());
        assert!(matches!(reason, Some(FinishReason::Stop)));
    }

    #[tokio::test]
    async fn process_stream_tool_call() {
        let (tx, _) = tokio::sync::broadcast::channel(50);
        let stream = test_helpers::mock_tool_stream("calc", r#"{"n":42}"#, "call_1");
        let mut token_bus = TokenBus::with_sender(tx, stream);
        let react_bus = ReactBus::new();

        let (content, accums, reason) = process_token_stream(&mut token_bus, &react_bus)
            .await
            .unwrap();
        assert_eq!(content, "");
        assert_eq!(accums.len(), 1);
        assert!(matches!(reason, Some(FinishReason::ToolCalls)));
    }

    #[tokio::test]
    async fn process_stream_error() {
        let (tx, _) = tokio::sync::broadcast::channel(50);
        let stream = test_helpers::mock_error_stream();
        let mut token_bus = TokenBus::with_sender(tx, stream);
        let react_bus = ReactBus::new();

        let result = process_token_stream(&mut token_bus, &react_bus).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn process_stream_text_and_tool() {
        let (tx, _) = tokio::sync::broadcast::channel(50);
        let stream = test_helpers::mock_text_plus_tool_stream(
            "Thinking...",
            "calc",
            r#"{"n":42}"#,
            "call_1",
        );
        let mut token_bus = TokenBus::with_sender(tx, stream);
        let react_bus = ReactBus::new();

        let (content, accums, reason) = process_token_stream(&mut token_bus, &react_bus)
            .await
            .unwrap();
        assert_eq!(content, "Thinking...");
        assert_eq!(accums.len(), 1);
        assert!(matches!(reason, Some(FinishReason::ToolCalls)));
    }

    fn empty_session_msgs() -> Arc<parking_lot::RwLock<Vec<FuneraMessage>>> {
        Arc::new(parking_lot::RwLock::new(Vec::new()))
    }

    // ── handle_turn_finish ───────────────────────────────────────

    #[tokio::test]
    async fn handle_finish_stop_with_content() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = ToolBus::new();
        let (tx, _rx) = mpsc::channel(10);
        let session_msgs = empty_session_msgs();

        let should_continue = handle_turn_finish(
            Some(&FinishReason::Stop),
            "Hello!",
            &HashMap::new(),
            &react_bus,
            &tool_bus,
            &tx,
            &session_msgs,
        )
        .await
        .unwrap();

        assert!(!should_continue);
        let guard = session_msgs.read();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].role().to_string(), "assistant");
    }

    #[tokio::test]
    async fn handle_finish_stop_empty_content() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = ToolBus::new();
        let (tx, _rx) = mpsc::channel(10);
        let session_msgs = empty_session_msgs();
        // Seed with one existing msg
        session_msgs.write().push(FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage { text: "hi".into() }),
        ));

        let should_continue = handle_turn_finish(
            Some(&FinishReason::Stop),
            "",
            &HashMap::new(),
            &react_bus,
            &tool_bus,
            &tx,
            &session_msgs,
        )
        .await
        .unwrap();

        assert!(!should_continue);
        // No new message added
        assert_eq!(session_msgs.read().len(), 1);
    }

    #[tokio::test]
    async fn handle_finish_none() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = ToolBus::new();
        let (tx, _rx) = mpsc::channel(10);
        let session_msgs = empty_session_msgs();

        let should_continue = handle_turn_finish(
            None,
            "Hello!",
            &HashMap::new(),
            &react_bus,
            &tool_bus,
            &tx,
            &session_msgs,
        )
        .await
        .unwrap();

        assert!(!should_continue);
        assert_eq!(session_msgs.read().len(), 1);
    }

    #[tokio::test]
    async fn handle_finish_tool_calls_with_executor() {
        let react_bus = ReactBus::new();
        let (tool_bus, mut exec_rx) = ToolBus::new();
        let (buf_tx, mut buf_rx) = mpsc::channel(10);
        let session_msgs = empty_session_msgs();

        let mut accums = HashMap::new();
        let accum = ToolCallAccumulator {
            index: 0,
            call_id: "call_abc".into(),
            name: "mock_tool".into(),
            args: r#"{"x":1}"#.into(),
        };
        accums.insert(0, accum);

        tokio::spawn(async move {
            if let Some(cmd) = exec_rx.recv().await {
                let _ = cmd.resp_tx.send(Ok("tool_result_ok".into()));
            }
        });

        let should_continue = handle_turn_finish(
            Some(&FinishReason::ToolCalls),
            "",
            &accums,
            &react_bus,
            &tool_bus,
            &buf_tx,
            &session_msgs,
        )
        .await
        .unwrap();

        assert!(should_continue);
        let received = buf_rx.try_recv().unwrap();
        assert!(matches!(
            received.msg_variant(),
            MsgVariant::ToolResponse(_)
        ));
    }

    #[tokio::test]
    async fn handle_finish_length() {
        let react_bus = ReactBus::new();
        let (tool_bus, exec_rx) = ToolBus::new();
        let (tx, _) = mpsc::channel(10);
        let session_msgs = empty_session_msgs();

        drop(exec_rx); // close receiver so ToolBus::execute fails immediately

        let mut accums = HashMap::new();
        let accum = ToolCallAccumulator {
            index: 0,
            call_id: "call_1".into(),
            name: "t".into(),
            args: "{}".into(),
        };
        accums.insert(0, accum);

        let should_continue = handle_turn_finish(
            Some(&FinishReason::Length),
            "",
            &accums,
            &react_bus,
            &tool_bus,
            &tx,
            &session_msgs,
        )
        .await;

        assert!(should_continue.is_ok());
    }

    // ── ToolCallAccumulator behavior ────────────────────────────

    #[test]
    fn tool_call_accumulator_fields() {
        let mut acc = ToolCallAccumulator {
            index: 1,
            call_id: "call_1".into(),
            name: "test".into(),
            args: r#"{"key":"val"}"#.into(),
        };
        assert_eq!(acc.index, 1);
        assert_eq!(acc.call_id, "call_1");
        assert_eq!(acc.name, "test");
        assert_eq!(acc.args, r#"{"key":"val"}"#);

        acc.call_id.push_str("_extended");
        assert_eq!(acc.call_id, "call_1_extended");
    }

    #[test]
    fn tool_call_accumulator_default_via_or_insert() {
        let mut map: HashMap<usize, ToolCallAccumulator> = HashMap::new();
        let acc = map.entry(5).or_insert_with(|| ToolCallAccumulator {
            index: 5,
            call_id: String::new(),
            name: String::new(),
            args: String::new(),
        });
        assert_eq!(acc.index, 5);
        assert!(acc.call_id.is_empty());
        assert!(acc.name.is_empty());
        assert!(acc.args.is_empty());
    }

    #[test]
    fn tool_call_accumulator_multiple_inserts() {
        let mut map: HashMap<usize, ToolCallAccumulator> = HashMap::new();
        map.insert(
            0,
            ToolCallAccumulator {
                index: 0,
                call_id: "a".into(),
                name: "t1".into(),
                args: "{}".into(),
            },
        );
        map.insert(
            1,
            ToolCallAccumulator {
                index: 1,
                call_id: "b".into(),
                name: "t2".into(),
                args: "[]".into(),
            },
        );
        assert_eq!(map.len(), 2);
    }
}
