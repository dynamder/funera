use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use anyhow::Result;
use async_openai::types::chat::FinishReason;
use serde_json::Value as JsonValue;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};
use crate::chat::session::SessionCmd;
use crate::env::FuneraEnvWatcher;
use crate::event_bus::env_state_bus::{EnvStateEvent, TurnHighWayHandle};
use crate::event_bus::react_bus::{ReactBus, ReactEvent, ToolCallErrorInfo, ToolCallRequest, ToolCallResponse};
use crate::event_bus::token_bus::{TokenBus, TokenEvent};
use crate::event_bus::tool_bus::ToolBus;
use crate::middleware::{ErrorsEnabled, EventSenderFn, MiddlewareChain, MiddlewareEvent};
use crate::provider::ChatProvider;
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
    pub session_tx: Option<mpsc::UnboundedSender<SessionCmd>>,
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
            session_tx: None,
        }
    }
}

pub struct ReActLoop<P: ChatProvider> {
    session_tx: Option<mpsc::UnboundedSender<SessionCmd>>,
    max_iteration: usize,
    env_watcher: FuneraEnvWatcher,
    tool_bus: ToolBus,
    env_state_tx: broadcast::Sender<EnvStateEvent>,
    turn_highway_handle: TurnHighWayHandle,
    _phantom: PhantomData<P>,
}

impl<P: ChatProvider> ReActLoop<P> {
    pub fn new(
        max_iteration: usize,
        session_tx: Option<mpsc::UnboundedSender<SessionCmd>>,
        env_watcher: FuneraEnvWatcher,
        tool_bus: ToolBus,
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        turn_highway_handle: TurnHighWayHandle,
    ) -> Self {
        Self {
            session_tx,
            max_iteration,
            env_watcher,
            tool_bus,
            env_state_tx,
            turn_highway_handle,
            _phantom: PhantomData,
        }
    }

    pub fn from_config(config: ReActLoopConfig) -> Self {
        Self::new(
            config.max_iteration,
            config.session_tx,
            config.env_watcher,
            config.tool_bus,
            config.env_state_tx,
            config.turn_highway_handle,
        )
    }

    async fn build_history_from(tx: &mpsc::UnboundedSender<SessionCmd>) -> Vec<JsonValue> {
        let (respond, rx) = oneshot::channel();
        let _ = tx.send(SessionCmd::FetchContext { respond });
        rx.await.unwrap_or_default()
    }

    pub fn run<E: MiddlewareEvent>(
        mut self,
        middleware: Option<Arc<MiddlewareChain<E, ErrorsEnabled>>>,
        event_sender: Option<EventSenderFn<E>>,
    ) -> ReActLoopHandle {
        let token = CancellationToken::new();
        let token_clone = token.clone();

        let task = tokio::spawn(async move {
            let mut iteration = 0;
            let mut env_watcher = self.env_watcher;

            while iteration < self.max_iteration {
                // Get current env state
                let client = env_watcher.watch_client();
                let tools_json = env_watcher.watch_tool();
                let model = env_watcher.watch_model();
                let skill_content = env_watcher.watch_skill();

                // Build history from session actor (includes init msg + tool results)
                let history_json = if let Some(ref tx) = self.session_tx {
                    Self::build_history_from(tx).await
                } else {
                    Vec::new()
                };

                // TurnHighWay handshake
                let (token_tx, react_bus) =
                    self.turn_highway_handle.prepare_turn().await;

                // Build LLM request
                let request_json = P::build_request_json(
                    &model, &history_json, &skill_content, &tools_json,
                );

                react_bus.send(ReactEvent::TurnStart).ok();

                // Emit TurnStart via middleware pipeline
                emit_event(&event_sender, E::turn_start());

                let stream = P::create_stream(&client, request_json).await?;

                // Process stream
                let mut token_bus = TokenBus::<P::Chunk>::with_sender(token_tx, stream);
                let (
                    assistant_content,
                    reasoning_content,
                    tool_call_accums,
                    turn_finish_reason,
                ) = process_token_stream(&mut token_bus, &react_bus).await?;

                let finish_reason_str = turn_finish_reason
                    .as_ref()
                    .map(|r| format!("{:?}", r));

                // 7. Build turn events from aggregated data
                let mut turn_events: Vec<E> = Vec::new();

                let reasoning = if reasoning_content.is_empty() {
                    None
                } else {
                    Some(reasoning_content.clone())
                };

                if !assistant_content.is_empty() {
                    turn_events
                        .push(E::assistant_text(assistant_content.clone(), reasoning));
                }

                let mut accums_sorted: Vec<_> = tool_call_accums.values().collect();
                accums_sorted.sort_by_key(|a| a.index);
                for acc in &accums_sorted {
                    let args: JsonValue =
                        serde_json::from_str(&acc.args).unwrap_or(JsonValue::Null);
                    turn_events.push(E::tool_call_request(
                        acc.call_id.clone().into(),
                        acc.name.clone(),
                        args,
                    ));
                }

                // Filter, emit, and store events through middleware
                filter_and_store(
                    turn_events,
                    &middleware,
                    &event_sender,
                    &self.session_tx,
                );

                // 8. Handle finish reason — execute tools, collect results
                let (should_continue, tool_results) = handle_turn_finish(
                    turn_finish_reason.as_ref(),
                    &assistant_content,
                    &reasoning_content,
                    &tool_call_accums,
                    &react_bus,
                    &self.tool_bus,
                )
                .await?;

                // 9. Filter tool results through middleware, emit, store
                let mut result_events: Vec<E> = Vec::new();
                for r in tool_results {
                    let result = match r.result {
                        Ok(s) => Ok(s),
                        Err(e) => Err(e.to_string().into()),
                    };
                    result_events.push(E::tool_response(r.call_id.into(), r.name, result));
                }
                filter_and_store(
                    result_events,
                    &middleware,
                    &event_sender,
                    &self.session_tx,
                );

                // 10. Emit TurnEnd
                emit_event(&event_sender, E::turn_end(finish_reason_str));
                react_bus.send(ReactEvent::TurnEnd).ok();

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
        }
    }
}

/// Tool execution result returned by `handle_turn_finish`.
struct ToolExecResult {
    call_id: String,
    name: String,
    result: Result<String, ToolCallError>,
}

/// Emit a single event if sender is present.
fn emit_event<E: MiddlewareEvent>(sender: &Option<EventSenderFn<E>>, event: E) {
    if let Some(s) = sender {
        s(event);
    }
}

/// Run a batch of events through middleware, emit filtered events, and store in session.
fn filter_and_store<E: MiddlewareEvent>(
    events: Vec<E>,
    middleware: &Option<Arc<MiddlewareChain<E, ErrorsEnabled>>>,
    event_sender: &Option<EventSenderFn<E>>,
    session_tx: &Option<mpsc::UnboundedSender<SessionCmd>>,
) {
    for event in events {
        let filtered = if let Some(chain) = middleware {
            match chain.process(event) {
                Ok(e) => e,
                Err(_) => continue,
            }
        } else {
            event
        };
        if let Some((role, variant)) = filtered.clone().into_session_message() {
            if let Some(tx) = session_tx {
                let _ = tx.send(SessionCmd::PushMessages {
                    msgs: vec![FuneraMessage::new(role, variant)],
                });
            }
        }
        if let Some(sender) = event_sender {
            sender(filtered);
        }
    }
}

async fn process_token_stream<C: crate::provider::StreamChunkExt>(
    token_bus: &mut TokenBus<C>,
    react_bus: &ReactBus,
) -> Result<(
    String,
    String,
    HashMap<usize, ToolCallAccumulator>,
    Option<FinishReason>,
)> {
    let mut assistant_content = String::new();
    let mut reasoning_content = String::new();
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
                            reasoning_content: None,
                        }),
                    );
                    react_bus.send(ReactEvent::MessageQueued(text_msg)).ok();
                }
                TokenEvent::Reasoning(r) => {
                    reasoning_content.push_str(&r);
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
                    if !call_id.is_empty() {
                        acc.call_id = call_id;
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

    Ok((assistant_content, reasoning_content, tool_call_accums, turn_finish_reason))
}

async fn handle_turn_finish(
    finish_reason: Option<&FinishReason>,
    _assistant_content: &str,
    _reasoning_content: &str,
    tool_call_accums: &HashMap<usize, ToolCallAccumulator>,
    react_bus: &ReactBus,
    tool_bus: &ToolBus,
) -> Result<(bool, Vec<ToolExecResult>)> {
    match finish_reason {
        None | Some(FinishReason::Stop) => Ok((false, Vec::new())),

        Some(FinishReason::ToolCalls) | Some(FinishReason::Length) => {
            let mut accums: Vec<_> = tool_call_accums.values().collect();
            accums.sort_by_key(|a| a.index);

            // Broadcast all tool execution requests via react_bus
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

            // Collect results for middleware filtering
            let mut tool_results = Vec::new();
            for (acc, result) in accums.iter().zip(results.into_iter()) {
                match result {
                    Ok(response) => {
                        react_bus
                            .send(ReactEvent::ToolExecResponse(Ok(ToolCallResponse {
                                call_id: acc.call_id.clone(),
                                name: acc.name.clone(),
                                result: response.clone(),
                            })))
                            .ok();
                        tool_results.push(ToolExecResult {
                            call_id: acc.call_id.clone(),
                            name: acc.name.clone(),
                            result: Ok(response),
                        });
                    }
                    Err(e) => {
                        react_bus
                            .send(ReactEvent::ToolExecResponse(Err(ToolCallErrorInfo {
                                call_id: acc.call_id.clone(),
                                name: acc.name.clone(),
                                error: e.to_string(),
                            })))
                            .ok();
                        tool_results.push(ToolExecResult {
                            call_id: acc.call_id.clone(),
                            name: acc.name.clone(),
                            result: Err(e),
                        });
                    }
                }
            }

            Ok((true, tool_results))
        }

        _ => Ok((false, Vec::new())),
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
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_openai::types::chat::FinishReason;
    use serde_json::json;
    use tokio::sync::mpsc;

    use crate::event_bus::env_state_bus::TurnHighWayEvent;
    use crate::event_bus::react_bus::ReactBus;
    use crate::event_bus::tool_bus::ToolBus;
    use crate::test_helpers;

    use super::*;

    /// Dummy event type for tests that require a `MiddlewareEvent` impl.
    #[derive(Debug, Clone)]
    struct TestEvent(String);

    impl MiddlewareEvent for TestEvent {
        type Error = String;

        fn assistant_text(content: String, _reasoning: Option<String>) -> Self {
            TestEvent(content)
        }
        fn tool_call_request(_call_id: Arc<str>, name: String, _args: JsonValue) -> Self {
            TestEvent(name)
        }
        fn tool_response(
            _call_id: Arc<str>,
            _name: String,
            _result: Result<String, String>,
        ) -> Self {
            TestEvent("tool_result".into())
        }
        fn turn_start() -> Self {
            TestEvent("turn_start".into())
        }
        fn turn_end(_finish_reason: Option<String>) -> Self {
            TestEvent("turn_end".into())
        }
        fn done() -> Self {
            TestEvent("done".into())
        }
        fn into_session_message(self) -> Option<(Role, MsgVariant)> {
            Some((
                Role::Assistant,
                MsgVariant::Text(TextMessage { text: self.0.into(), reasoning_content: None }),
            ))
        }
    }

    fn empty_session_msgs() -> Arc<parking_lot::RwLock<Vec<FuneraMessage>>> {
        Arc::new(parking_lot::RwLock::new(Vec::new()))
    }

    // ── process_token_stream ────────────────────────────────────

    #[tokio::test]
    async fn process_stream_empty() {
        let (tx, _) = tokio::sync::broadcast::channel(50);
        let stream = test_helpers::mock_empty_stream();
        let mut token_bus = TokenBus::with_sender(tx, stream);
        let react_bus = ReactBus::new();

        let (content, reasoning, accums, reason) =
            process_token_stream(&mut token_bus, &react_bus).await.unwrap();
        assert_eq!(content, "");
        assert_eq!(reasoning, "");
        assert!(accums.is_empty());
        assert!(reason.is_none());
    }

    #[tokio::test]
    async fn process_stream_text() {
        let (tx, _) = tokio::sync::broadcast::channel(50);
        let stream = test_helpers::mock_text_stream(vec!["Hello", " ", "World"]);
        let mut token_bus = TokenBus::with_sender(tx, stream);
        let react_bus = ReactBus::new();

        let (content, reasoning, accums, reason) =
            process_token_stream(&mut token_bus, &react_bus).await.unwrap();
        assert_eq!(content, "Hello World");
        assert_eq!(reasoning, "");
        assert!(accums.is_empty());
        assert!(matches!(reason, Some(FinishReason::Stop)));
    }

    #[tokio::test]
    async fn process_stream_tool_call() {
        let (tx, _) = tokio::sync::broadcast::channel(50);
        let stream = test_helpers::mock_tool_stream("calc", r#"{"n":42}"#, "call_1");
        let mut token_bus = TokenBus::with_sender(tx, stream);
        let react_bus = ReactBus::new();

        let (content, reasoning, accums, reason) =
            process_token_stream(&mut token_bus, &react_bus).await.unwrap();
        assert_eq!(content, "");
        assert_eq!(reasoning, "");
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

        let (content, reasoning, accums, reason) =
            process_token_stream(&mut token_bus, &react_bus).await.unwrap();
        assert_eq!(content, "Thinking...");
        assert_eq!(reasoning, "");
        assert_eq!(accums.len(), 1);
        assert!(matches!(reason, Some(FinishReason::ToolCalls)));
    }

    // ── handle_turn_finish ───────────────────────────────────────

    #[tokio::test]
    async fn handle_finish_stop_with_content() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = ToolBus::new();
        let _session_msgs = empty_session_msgs();

        let (should_continue, results) = handle_turn_finish(
            Some(&FinishReason::Stop),
            "Hello!",
            "",
            &HashMap::new(),
            &react_bus,
            &tool_bus,
        )
        .await
        .unwrap();

        assert!(!should_continue);
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn handle_finish_stop_empty_content() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = ToolBus::new();
        let session_msgs = empty_session_msgs();
        session_msgs.write().push(FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage { text: "hi".into(), reasoning_content: None }),
        ));

        let (should_continue, results) = handle_turn_finish(
            Some(&FinishReason::Stop),
            "",
            "",
            &HashMap::new(),
            &react_bus,
            &tool_bus,
        )
        .await
        .unwrap();

        assert!(!should_continue);
        assert!(results.is_empty());
        assert_eq!(session_msgs.read().len(), 1);
    }

    #[tokio::test]
    async fn handle_finish_none() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = ToolBus::new();

        let (should_continue, results) = handle_turn_finish(
            None,
            "Hello!",
            "",
            &HashMap::new(),
            &react_bus,
            &tool_bus,
        )
        .await
        .unwrap();

        assert!(!should_continue);
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn handle_finish_tool_calls_with_executor() {
        let react_bus = ReactBus::new();
        let (tool_bus, mut exec_rx) = ToolBus::new();

        let mut accums = HashMap::new();
        accums.insert(0, ToolCallAccumulator {
            index: 0,
            call_id: "call_abc".into(),
            name: "mock_tool".into(),
            args: r#"{"x":1}"#.into(),
        });

        tokio::spawn(async move {
            if let Some(cmd) = exec_rx.recv().await {
                let _ = cmd.resp_tx.send(Ok("tool_result_ok".into()));
            }
        });

        let (should_continue, results) = handle_turn_finish(
            Some(&FinishReason::ToolCalls),
            "",
            "",
            &accums,
            &react_bus,
            &tool_bus,
        )
        .await
        .unwrap();

        assert!(should_continue);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].call_id, "call_abc");
        assert_eq!(results[0].result.as_ref().unwrap(), "tool_result_ok");
    }

    #[tokio::test]
    async fn handle_finish_tool_calls_preserves_reasoning() {
        let react_bus = ReactBus::new();
        let (tool_bus, mut exec_rx) = ToolBus::new();

        let mut accums = HashMap::new();
        accums.insert(0, ToolCallAccumulator {
            index: 0,
            call_id: "call_abc".into(),
            name: "mock_tool".into(),
            args: r#"{"x":1}"#.into(),
        });

        tokio::spawn(async move {
            if let Some(cmd) = exec_rx.recv().await {
                let _ = cmd.resp_tx.send(Ok("ok".into()));
            }
        });

        let (should_continue, _results) = handle_turn_finish(
            Some(&FinishReason::ToolCalls),
            "",
            "I need to think about this first",
            &accums,
            &react_bus,
            &tool_bus,
        )
        .await
        .unwrap();

        assert!(should_continue);
    }

    #[tokio::test]
    async fn handle_finish_length() {
        let react_bus = ReactBus::new();
        let (tool_bus, exec_rx) = ToolBus::new();
        drop(exec_rx);

        let mut accums = HashMap::new();
        accums.insert(0, ToolCallAccumulator {
            index: 0,
            call_id: "call_1".into(),
            name: "t".into(),
            args: "{}".into(),
        });

        let result = handle_turn_finish(
            Some(&FinishReason::Length),
            "",
            "",
            &accums,
            &react_bus,
            &tool_bus,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn handle_finish_stop_with_reasoning() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = ToolBus::new();

        let (should_continue, results) = handle_turn_finish(
            Some(&FinishReason::Stop),
            "Final answer",
            "I need to think about this...",
            &HashMap::new(),
            &react_bus,
            &tool_bus,
        )
        .await
        .unwrap();

        assert!(!should_continue);
        assert!(results.is_empty());
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

    // ── end-to-end ReActLoop with MockProvider ──────────────────

    struct MockProvider;

    impl ChatProvider for MockProvider {
        type Chunk = async_openai::types::chat::CreateChatCompletionStreamResponse;

        fn build_request_json(
            _model: &str,
            messages: &[JsonValue],
            _skill_content: &str,
            _tools_json: &JsonValue,
        ) -> JsonValue {
            // Return minimal request shape so the plumbing works
            serde_json::json!({
                "model": "mock",
                "messages": messages,
                "stream": true,
            })
        }

        async fn create_stream(
            _client: &async_openai::Client<async_openai::config::OpenAIConfig>,
            _request_json: JsonValue,
        ) -> Result<async_openai::types::stream::StreamResponse<Self::Chunk>, async_openai::error::OpenAIError>
        {
            Ok(test_helpers::mock_text_stream(vec!["Mock response"]))
        }
    }

    #[tokio::test]
    async fn react_loop_mock_provider_completes() {
        let (env_state_tx, _env_state_rx) = tokio::sync::broadcast::channel(20);
        let (tool_bus, _exec_rx) = ToolBus::new();

        // Set up a TurnHighWay with a simple dispatcher that responds to prepare_turn
        let (loop_tx, mut rx_from_loop) = tokio::sync::mpsc::channel(5);
        let (tx_to_loop, loop_rx) = tokio::sync::mpsc::channel(5);

        // Spawn a mini-dispatcher that responds to TurnPrepareRequest
        tokio::spawn(async move {
            let _req = rx_from_loop.recv().await;
            let (token_tx, _) = tokio::sync::broadcast::channel(50);
            let react_bus = ReactBus::new();
            let _ = tx_to_loop.send(TurnHighWayEvent::TurnPrepareResponse {
                token_tx,
                react_bus,
            }).await;
        });

        let handle = TurnHighWayHandle {
            turn_high_way_tx: loop_tx,
            turn_high_way_rx: loop_rx,
        };

        let session_tx = crate::chat::session::spawn_session_actor();
        let loop_instance = ReActLoop::<MockProvider>::new(
            1, Some(session_tx.clone()),
            crate::env::FuneraEnv::new(
                crate::re_act::tool::ToolRegistry::new(),
                async_openai::Client::new(),
                "mock-model",
            ).1,
            tool_bus,
            env_state_tx,
            handle,
        );

        let loop_handle = loop_instance.run::<TestEvent>(None, None);
        let msg = FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage { text: "ping".into(), reasoning_content: None }),
        );
        let _ = session_tx.send(crate::chat::session::SessionCmd::PushMessages {
            msgs: vec![msg],
        });

        let result = loop_handle.task.await;
        assert!(result.is_ok(), "loop task should complete successfully");
        let inner = result.unwrap();
        assert!(inner.is_ok(), "loop should return Ok(())");
    }
}
