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
#[cfg(feature = "tool")]
use crate::event_bus::tool_bus::ToolBus;
use crate::middleware::{ErrorsEnabled, EventSenderFn, MiddlewareChain, MiddlewareEvent};
use crate::provider::ChatProvider;

#[cfg(feature = "skill")]
pub mod skills;
#[cfg(feature = "tool")]
pub mod tool;
#[cfg(feature = "tool")]
pub mod tool_executor;

pub struct ReActLoopConfig {
    pub buffer: usize,
    pub max_iteration: usize,
    pub env_watcher: FuneraEnvWatcher,
    #[cfg(feature = "tool")]
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
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        turn_highway_handle: TurnHighWayHandle,
    ) -> Self {
        Self {
            buffer,
            max_iteration,
            env_watcher,
            #[cfg(feature = "tool")]
            tool_bus: {
                let (tool_bus, _) = ToolBus::new();
                tool_bus
            },
            env_state_tx,
            turn_highway_handle,
            session_tx: None,
        }
    }

    #[cfg(feature = "tool")]
    pub fn with_tool_bus(mut self, tool_bus: ToolBus) -> Self {
        // Placeholder: tool_bus is already set via the field; this allows
        // extending the builder-style construction if needed.
        self.tool_bus = tool_bus;
        self
    }
}

pub struct ReActLoop<P: ChatProvider> {
    session_tx: Option<mpsc::UnboundedSender<SessionCmd>>,
    max_iteration: usize,
    env_watcher: FuneraEnvWatcher,
    #[cfg(feature = "tool")]
    tool_bus: ToolBus,
    #[allow(dead_code)]
    env_state_tx: broadcast::Sender<EnvStateEvent>,
    turn_highway_handle: TurnHighWayHandle,
    _phantom: PhantomData<P>,
}

impl<P: ChatProvider> ReActLoop<P> {
    pub fn new(
        max_iteration: usize,
        session_tx: Option<mpsc::UnboundedSender<SessionCmd>>,
        env_watcher: FuneraEnvWatcher,
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        turn_highway_handle: TurnHighWayHandle,
    ) -> Self {
        Self {
            session_tx,
            max_iteration,
            env_watcher,
            #[cfg(feature = "tool")]
            tool_bus: {
                let (tool_bus, _) = ToolBus::new();
                tool_bus
            },
            env_state_tx,
            turn_highway_handle,
            _phantom: PhantomData,
        }
    }

    #[cfg(feature = "tool")]
    pub fn with_tool_bus(mut self, tool_bus: ToolBus) -> Self {
        self.tool_bus = tool_bus;
        self
    }

    pub fn from_config(config: ReActLoopConfig) -> Self {
        Self {
            session_tx: config.session_tx,
            max_iteration: config.max_iteration,
            env_watcher: config.env_watcher,
            #[cfg(feature = "tool")]
            tool_bus: config.tool_bus,
            env_state_tx: config.env_state_tx,
            turn_highway_handle: config.turn_highway_handle,
            _phantom: PhantomData,
        }
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
                let client = env_watcher.watch_client();
                #[cfg(feature = "tool")]
                let tools_json = env_watcher.watch_tool();
                #[cfg(not(feature = "tool"))]
                let tools_json = JsonValue::Array(Vec::new());
                let model = env_watcher.watch_model();
                #[cfg(feature = "skill")]
                let skill_content = env_watcher.watch_skill();
                #[cfg(not(feature = "skill"))]
                let skill_content = String::new();

                let history_json = if let Some(ref tx) = self.session_tx {
                    Self::build_history_from(tx).await
                } else {
                    Vec::new()
                };

                let (token_tx, react_bus) =
                    self.turn_highway_handle.prepare_turn().await;

                let request_json = P::build_request_json(
                    &model, &history_json, &skill_content, &tools_json,
                );

                react_bus.send(ReactEvent::TurnStart).ok();

                emit_event(&event_sender, E::turn_start());

                let stream = P::create_stream(&client, request_json).await?;

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

                filter_and_store(
                    turn_events,
                    &middleware,
                    &event_sender,
                    &self.session_tx,
                );

                let (should_continue, tool_results) = handle_turn_finish(
                    turn_finish_reason.as_ref(),
                    &tool_call_accums,
                    &react_bus,
                    #[cfg(feature = "tool")]
                    &self.tool_bus,
                )
                .await?;

                #[cfg(feature = "tool")]
                {
                    let mut result_events: Vec<E> = Vec::new();
                    for r in tool_results {
                        let result = match r.result {
                            Ok(s) => Ok(s),
                            Err(e) => Err(e.into()),
                        };
                        result_events.push(E::tool_response(r.call_id.into(), r.name, result));
                    }
                    filter_and_store(
                        result_events,
                        &middleware,
                        &event_sender,
                        &self.session_tx,
                    );
                }

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

struct ToolExecResult {
    call_id: String,
    name: String,
    result: Result<String, String>,
}

fn emit_event<E: MiddlewareEvent>(sender: &Option<EventSenderFn<E>>, event: E) {
    if let Some(s) = sender {
        s(event);
    }
}

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
        if let Some((role, variant)) = filtered.clone().into_session_message()
            && let Some(tx) = session_tx
        {
            let _ = tx.send(SessionCmd::PushMessages {
                msgs: vec![FuneraMessage::new(role, variant)],
            });
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

#[cfg(feature = "tool")]
async fn handle_turn_finish(
    finish_reason: Option<&FinishReason>,
    tool_call_accums: &HashMap<usize, ToolCallAccumulator>,
    react_bus: &ReactBus,
    tool_bus: &ToolBus,
) -> Result<(bool, Vec<ToolExecResult>)> {
    match finish_reason {
        None | Some(FinishReason::Stop) => Ok((false, Vec::new())),

        Some(FinishReason::ToolCalls) | Some(FinishReason::Length) => {
            let mut accums: Vec<_> = tool_call_accums.values().collect();
            accums.sort_by_key(|a| a.index);

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

            use futures::future::join_all;
            let results: Vec<Result<String, crate::re_act::tool::ToolCallError>> = join_all(accums.iter().map(|acc| {
                let args: JsonValue = serde_json::from_str(&acc.args).unwrap_or(JsonValue::Null);
                tool_bus.execute(acc.call_id.clone(), acc.name.clone(), args)
            }))
            .await;

            let mut tool_results = Vec::new();
            for (acc, result) in accums.iter().zip(results) {
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
                            result: Err(e.to_string()),
                        });
                    }
                }
            }

            Ok((true, tool_results))
        }

        _ => Ok((false, Vec::new())),
    }
}

#[cfg(not(feature = "tool"))]
async fn handle_turn_finish(
    finish_reason: Option<&FinishReason>,
    tool_call_accums: &HashMap<usize, ToolCallAccumulator>,
    _react_bus: &ReactBus,
) -> Result<(bool, Vec<ToolExecResult>)> {
    match finish_reason {
        Some(FinishReason::ToolCalls) | Some(FinishReason::Length)
            if !tool_call_accums.is_empty() =>
        {
            eprintln!("warn: LLM requested tool calls but 'tool' feature is disabled");
            Ok((false, Vec::new()))
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
    use tokio::sync::mpsc;

    use crate::event_bus::env_state_bus::TurnHighWayEvent;
    use crate::event_bus::react_bus::ReactBus;
    use crate::test_helpers;

    use super::*;

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

    #[cfg(feature = "tool")]
    #[tokio::test]
    async fn handle_finish_stop_with_content() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = crate::event_bus::tool_bus::ToolBus::new();

        let (should_continue, results) = handle_turn_finish(
            Some(&FinishReason::Stop),
            &HashMap::new(),
            &react_bus,
            &tool_bus,
        )
        .await
        .unwrap();

        assert!(!should_continue);
        assert!(results.is_empty());
    }

    #[cfg(feature = "tool")]
    #[tokio::test]
    async fn handle_finish_none() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = crate::event_bus::tool_bus::ToolBus::new();

        let (should_continue, results) = handle_turn_finish(
            None,
            &HashMap::new(),
            &react_bus,
            &tool_bus,
        )
        .await
        .unwrap();

        assert!(!should_continue);
        assert!(results.is_empty());
    }

    #[cfg(feature = "tool")]
    #[tokio::test]
    async fn handle_finish_stop_with_reasoning() {
        let react_bus = ReactBus::new();
        let (tool_bus, _rx) = crate::event_bus::tool_bus::ToolBus::new();

        let (should_continue, results) = handle_turn_finish(
            Some(&FinishReason::Stop),
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

    #[cfg(feature = "tool")]
    struct MockProvider;

    #[cfg(feature = "tool")]
    impl ChatProvider for MockProvider {
        type Chunk = async_openai::types::chat::CreateChatCompletionStreamResponse;

        fn build_request_json(
            _model: &str,
            messages: &[JsonValue],
            _skill_content: &str,
            _tools_json: &JsonValue,
        ) -> JsonValue {
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

    #[cfg(feature = "tool")]
    #[tokio::test]
    async fn react_loop_mock_provider_completes() {
        let (env_state_tx, _env_state_rx) = tokio::sync::broadcast::channel(20);
        let (tool_bus, _exec_rx) = crate::event_bus::tool_bus::ToolBus::new();

        let (loop_tx, mut rx_from_loop) = tokio::sync::mpsc::channel(5);
        let (tx_to_loop, loop_rx) = tokio::sync::mpsc::channel(5);

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
        let (_env, _env_watcher) = crate::env::FuneraEnv::new(
            async_openai::Client::new(),
            "mock-model",
        );
        let env_watcher = _env_watcher;

        let loop_instance = ReActLoop::<MockProvider>::new(
            1, Some(session_tx.clone()),
            env_watcher,
            env_state_tx,
            handle,
        ).with_tool_bus(tool_bus);

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
