use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use funera_core::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};
use funera_core::chat::session::FuneraSession;
use funera_core::event_bus::env_state_bus::{EnvStateBus, EnvStateEvent};
use funera_core::middleware::EventSenderFn;
#[cfg(feature = "middleware")]
use funera_core::middleware::{ErrorsEnabled, MiddlewareChain};
use funera_core::provider::ChatProvider;
use funera_core::re_act::ReActLoopConfig;

use crate::dispatcher::{CallbackDispatcher, CallbackRegistry};
use crate::error::OrchestrateError;
use crate::event::{AgentEvent, RawAgentEvent};
use crate::response::{ChatResponse, ToolCallInfo};
use crate::runtime::{AgentRuntime, Idle};
use crate::send_handle::{FireStreamHandle, SendHandle, SendStreamHandle};

// ---------------------------------------------------------------------------
// AgentBuilder
// ---------------------------------------------------------------------------

/// Builds an [`Agent`].
///
/// An `Agent` is lightweight configuration — no infrastructure, no session.
/// All runtime concerns are injected at call time via `&AgentRuntime` or
/// `&mut AgentRuntime`.
///
/// # Example
///
/// ```rust,no_run
/// # use funera_orchestrate::Agent;
/// let agent = Agent::builder()
///     .system_prompt("You are a helpful assistant.")
///     .on_token(|t| print!("{t}"))
///     .build();
/// ```
pub struct AgentBuilder {
    system_prompt: Option<String>,
    callbacks: CallbackRegistry,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            system_prompt: None,
            callbacks: CallbackRegistry::new(),
        }
    }

    /// A system-level prompt that prefixes every interaction.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Fired for each text token streamed from the LLM.
    pub fn on_token<F>(mut self, f: F) -> Self
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if let AgentEvent::Text(t) = event {
                f(t);
            }
        }));
        self
    }

    /// Fired when a tool call is detected (before execution).
    pub fn on_tool_call<F>(mut self, f: F) -> Self
    where
        F: Fn(String, serde_json::Value) + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if let AgentEvent::ToolCallRequest { name, args, .. } = event {
                f(name, args);
            }
        }));
        self
    }

    /// Fired when a tool execution completes.
    pub fn on_tool_result<F>(mut self, f: F) -> Self
    where
        F: Fn(String, Result<String, String>) + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if let AgentEvent::ToolCallResult { name, result, .. } = event {
                f(name, result);
            }
        }));
        self
    }

    /// Fired at the start of each ReAct turn.
    pub fn on_turn_start<F>(mut self, f: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if matches!(event, AgentEvent::TurnStart) {
                f();
            }
        }));
        self
    }

    /// Fired at the end of each ReAct turn.
    pub fn on_turn_end<F>(mut self, f: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if matches!(event, AgentEvent::TurnEnd { .. }) {
                f();
            }
        }));
        self
    }

    /// Fired for every [`AgentEvent`] (catch-all).
    pub fn on_event<F>(mut self, f: F) -> Self
    where
        F: Fn(AgentEvent) + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(f));
        self
    }

    /// Build the [`Agent`].
    pub fn build(self) -> Agent {
        let (event_tx, _) = broadcast::channel(256);
        let (raw_event_tx, _) = broadcast::channel(256);
        Agent {
            system_prompt: self.system_prompt,
            callbacks: Arc::new(self.callbacks),
            event_tx,
            raw_event_tx,
        }
    }
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// A lightweight agent configuration.
///
/// `Agent` holds only behavioural configuration (system prompt, callbacks).
/// All runtime and session state lives in [`AgentRuntime`], which is injected
/// at call time.
///
/// # Fire-and-forget (one-shot)
///
/// ```rust,no_run
/// # use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let runtime = AgentRuntime::<DeepSeekProvider>::builder()
///     .api_key(std::env::var("DEEPSEEK_API_KEY")?)
///     .model("deepseek-v4-flash")
///     .build()?;
///
/// let agent = Agent::builder()
///     .system_prompt("You are helpful.")
///     .build();
///
/// // fire uses a temporary session — runtime is &, no state mutated
/// let resp = agent.fire("Hello!", &runtime).await?;
/// # Ok(())
/// # }
/// ```
///
/// # Multi-turn conversation
///
/// ```rust,no_run
/// # use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let runtime = AgentRuntime::<DeepSeekProvider>::builder()
///     .api_key(std::env::var("DEEPSEEK_API_KEY")?)
///     .model("deepseek-v4-flash")
///     .build()?;
///
/// let agent = Agent::builder().build();
///
/// // send consumes runtime, must unwrap via IntoFuture
/// let (runtime, _) = agent.send("My name is Alice.", runtime).await?.await?;
/// let (_runtime, _) = agent.send("What is my name?", runtime).await?.await?;
/// // → "Alice"
/// # Ok(())
/// # }
/// ```
pub struct Agent {
    pub(crate) system_prompt: Option<String>,
    pub(crate) callbacks: Arc<CallbackRegistry>,
    pub(crate) event_tx: broadcast::Sender<AgentEvent>,
    pub(crate) raw_event_tx: broadcast::Sender<RawAgentEvent>,
}

impl Agent {
    /// Create a new [`AgentBuilder`].
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// Subscribe to all [`AgentEvent`]s from subsequent calls.
    ///
    /// The returned receiver gets a clone of every event (tokens, tool calls,
    /// turn boundaries) dispatched during `fire`/`send`.
    pub fn subscribe_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    /// Subscribe to raw underlying events from the core event buses.
    ///
    /// The returned receiver yields [`RawAgentEvent`] variants that directly
    /// wrap `funera_core`'s [`TokenEvent`](funera_core::event_bus::token_bus::TokenEvent),
    /// [`ReactEvent`](funera_core::event_bus::react_bus::ReactEvent), and
    /// [`EnvStateEvent`](funera_core::event_bus::env_state_bus::EnvStateEvent).
    ///
    /// Unlike [`subscribe_events`](Self::subscribe_events) which returns a
    /// curated/translated [`AgentEvent`], this stream provides the original
    /// events including [`TokenEvent::ToolDelta`], [`ReactEvent::MessageQueued`],
    /// and all [`EnvStateEvent`] variants.
    pub fn subscribe_raw_events(&self) -> broadcast::Receiver<RawAgentEvent> {
        self.raw_event_tx.subscribe()
    }

    // ── fire (one-shot, no session) ────────────────────────────────

    /// One-shot query. Creates a temporary session, runs a single ReAct loop,
    /// and discards the session.
    pub async fn fire<P: ChatProvider, S>(
        &self,
        msg: impl Into<String>,
        runtime: &AgentRuntime<P, S>,
    ) -> Result<ChatResponse, OrchestrateError> {
        let text = msg.into();
        let mut event_rx = self.subscribe_events();

        let (env_state_bus, turn_highway_handle) = EnvStateBus::new();
        let env_state_tx = env_state_bus.env_state_tx.clone();
        let env_state_rx = env_state_bus.subscribe();
        env_state_bus.start_turn_highway();

        let _dispatcher = CallbackDispatcher::new(
            env_state_rx,
            self.event_tx.clone(),
            self.raw_event_tx.clone(),
        );

        let _ = env_state_tx.send(EnvStateEvent::SessionStart);

        // Temporary actor for one-shot — dropped after react_loop completes
        let session_tx = funera_core::chat::session::spawn_session_actor();
        let session = FuneraSession::new(session_tx);
        if let Some(ref sys) = self.system_prompt {
            session.push_message(FuneraMessage::new(
                Role::System,
                MsgVariant::Text(TextMessage {
                    text: sys.clone().into(),
                    reasoning_content: None,
                }),
            ));
        }

        let init_msg = FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage {
                text: text.into(),
                reasoning_content: None,
            }),
        );

        let mut config = ReActLoopConfig::new(
            runtime.channel_buffer(),
            runtime.max_iterations(),
            runtime.env_watcher(),
            env_state_tx.clone(),
            turn_highway_handle,
        );
        #[cfg(feature = "tool")]
        {
            config = config.with_tool_bus(runtime.tool_bus.clone());
        }

        let event_sender = build_event_sender(self.callbacks.clone(), self.event_tx.clone());

        let result = session
            .react_loop::<P, AgentEvent>(
                init_msg,
                config,
                env_state_tx.clone(),
                middleware_opt(runtime),
                Some(event_sender),
            )
            .await;

        let _ = env_state_tx.send(EnvStateEvent::SessionClosed);
        aggregate_response(&mut event_rx, result).await
    }

    /// Streaming variant of [`fire`](Self::fire).
    ///
    /// Returns a [`FireStreamHandle`] that provides `recv()` for per-event
    /// streaming and `IntoFuture` / `wait()` for the final [`ChatResponse`].
    pub async fn fire_stream<P: ChatProvider, S>(
        &self,
        msg: impl Into<String>,
        runtime: &AgentRuntime<P, S>,
    ) -> Result<FireStreamHandle, OrchestrateError> {
        let text = msg.into();
        let event_rx = self.subscribe_events();

        // Spawn relay: broadcast → mpsc
        let (relay_tx, stream_rx) = mpsc::channel(256);
        let relay_event_rx = self.subscribe_events();
        tokio::spawn(async move {
            relay_broadcast_to_mpsc(relay_event_rx, relay_tx).await;
        });

        let (env_state_bus, turn_highway_handle) = EnvStateBus::new();
        let env_state_tx = env_state_bus.env_state_tx.clone();
        let env_state_rx = env_state_bus.subscribe();
        env_state_bus.start_turn_highway();

        let _dispatcher = CallbackDispatcher::new(
            env_state_rx,
            self.event_tx.clone(),
            self.raw_event_tx.clone(),
        );
        let _ = env_state_tx.send(EnvStateEvent::SessionStart);

        let session_tx = funera_core::chat::session::spawn_session_actor();
        let session = FuneraSession::new(session_tx);
        if let Some(ref sys) = self.system_prompt {
            session.push_message(FuneraMessage::new(
                Role::System,
                MsgVariant::Text(TextMessage {
                    text: sys.clone().into(),
                    reasoning_content: None,
                }),
            ));
        }
        let init_msg = FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage {
                text: text.into(),
                reasoning_content: None,
            }),
        );
        let mut config = ReActLoopConfig::new(
            runtime.channel_buffer(),
            runtime.max_iterations(),
            runtime.env_watcher(),
            env_state_tx.clone(),
            turn_highway_handle,
        );
        #[cfg(feature = "tool")]
        {
            config = config.with_tool_bus(runtime.tool_bus.clone());
        }
        let event_sender = build_event_sender(self.callbacks.clone(), self.event_tx.clone());

        // Spawn react_loop as background task
        let mw = middleware_opt(runtime);
        let env_tx = env_state_tx.clone();
        let handle = tokio::spawn(async move {
            session
                .react_loop::<P, AgentEvent>(init_msg, config, env_tx, mw, Some(event_sender))
                .await
        });

        Ok(FireStreamHandle {
            handle,
            event_rx,
            stream_rx,
            env_state_tx,
        })
    }

    // ── send (multi-turn, persistent session) ──────────────────────

    /// Multi-turn message. Consumes `AgentRuntime<P, Idle>` and returns a
    /// [`SendHandle`] that yields `(AgentRuntime<P, Idle>, ChatResponse)` on
    /// completion. The react_loop runs in a background task — you can query
    /// session context via the handle while it is in progress.
    pub async fn send<P: ChatProvider>(
        &self,
        msg: impl Into<String>,
        runtime: AgentRuntime<P, Idle>,
    ) -> Result<SendHandle<P>, OrchestrateError> {
        let text = msg.into();
        let event_rx = self.subscribe_events();

        let (env_state_bus, turn_highway_handle) = EnvStateBus::new();
        let env_state_tx = env_state_bus.env_state_tx.clone();
        let env_state_rx = env_state_bus.subscribe();
        env_state_bus.start_turn_highway();

        let _dispatcher = CallbackDispatcher::new(
            env_state_rx,
            self.event_tx.clone(),
            self.raw_event_tx.clone(),
        );
        let _ = env_state_tx.send(EnvStateEvent::SessionStart);

        let session = FuneraSession::new(runtime.session_tx());
        if let Some(ref sys) = self.system_prompt {
            let msgs = session.session_context().await;
            if msgs.is_empty() {
                session.push_message(FuneraMessage::new(
                    Role::System,
                    MsgVariant::Text(TextMessage {
                        text: sys.clone().into(),
                        reasoning_content: None,
                    }),
                ));
            }
        }
        let init_msg = FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage {
                text: text.into(),
                reasoning_content: None,
            }),
        );
        let mut config = ReActLoopConfig::new(
            runtime.channel_buffer(),
            runtime.max_iterations(),
            runtime.env_watcher(),
            env_state_tx.clone(),
            turn_highway_handle,
        );
        #[cfg(feature = "tool")]
        {
            config = config.with_tool_bus(runtime.tool_bus.clone());
        }
        let event_sender = build_event_sender(self.callbacks.clone(), self.event_tx.clone());

        let env_tx = env_state_tx.clone();
        let mw = middleware_opt(&runtime);
        let handle = tokio::spawn(async move {
            session
                .react_loop::<P, AgentEvent>(init_msg, config, env_tx, mw, Some(event_sender))
                .await
        });

        Ok(SendHandle {
            runtime: runtime.into_acquired(),
            handle,
            event_rx,
            env_state_tx,
        })
    }

    /// Streaming variant of [`send`](Self::send).
    pub async fn send_stream<P: ChatProvider>(
        &self,
        msg: impl Into<String>,
        runtime: AgentRuntime<P, Idle>,
    ) -> Result<SendStreamHandle<P>, OrchestrateError> {
        let text = msg.into();
        let event_rx = self.subscribe_events();

        // Spawn relay: broadcast → mpsc
        let (relay_tx, stream_rx) = mpsc::channel(256);
        let relay_event_rx = self.subscribe_events();
        tokio::spawn(async move {
            relay_broadcast_to_mpsc(relay_event_rx, relay_tx).await;
        });

        let (env_state_bus, turn_highway_handle) = EnvStateBus::new();
        let env_state_tx = env_state_bus.env_state_tx.clone();
        let env_state_rx = env_state_bus.subscribe();
        env_state_bus.start_turn_highway();

        let _dispatcher = CallbackDispatcher::new(
            env_state_rx,
            self.event_tx.clone(),
            self.raw_event_tx.clone(),
        );
        let _ = env_state_tx.send(EnvStateEvent::SessionStart);

        let session = FuneraSession::new(runtime.session_tx());
        if let Some(ref sys) = self.system_prompt {
            let msgs = session.session_context().await;
            if msgs.is_empty() {
                session.push_message(FuneraMessage::new(
                    Role::System,
                    MsgVariant::Text(TextMessage {
                        text: sys.clone().into(),
                        reasoning_content: None,
                    }),
                ));
            }
        }
        let init_msg = FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage {
                text: text.into(),
                reasoning_content: None,
            }),
        );
        let mut config = ReActLoopConfig::new(
            runtime.channel_buffer(),
            runtime.max_iterations(),
            runtime.env_watcher(),
            env_state_tx.clone(),
            turn_highway_handle,
        );
        #[cfg(feature = "tool")]
        {
            config = config.with_tool_bus(runtime.tool_bus.clone());
        }
        let event_sender = build_event_sender(self.callbacks.clone(), self.event_tx.clone());

        let env_tx = env_state_tx.clone();
        let mw = middleware_opt(&runtime);
        let handle = tokio::spawn(async move {
            session
                .react_loop::<P, AgentEvent>(init_msg, config, env_tx, mw, Some(event_sender))
                .await
        });

        Ok(SendStreamHandle {
            runtime: runtime.into_acquired(),
            handle,
            event_rx,
            stream_rx,
            env_state_tx,
        })
    }
}

// ═══════════════════════════════════════════════════════════
// Free helper functions
// ═══════════════════════════════════════════════════════════

/// Build an event sender closure that dispatches to callbacks and broadcasts to event_tx.
fn build_event_sender(
    callbacks: Arc<CallbackRegistry>,
    event_tx: broadcast::Sender<AgentEvent>,
) -> EventSenderFn<AgentEvent> {
    Box::new(move |event: AgentEvent| {
        callbacks.dispatch(event.clone());
        let _ = event_tx.send(event);
    })
}

/// Return the middleware chain from runtime, or None.
#[cfg(feature = "middleware")]
fn middleware_opt<P: ChatProvider, S>(
    runtime: &AgentRuntime<P, S>,
) -> Option<Arc<MiddlewareChain<AgentEvent, ErrorsEnabled>>> {
    Some(runtime.middleware_chain())
}

#[cfg(not(feature = "middleware"))]
fn middleware_opt<P: ChatProvider, S>(
    _runtime: &AgentRuntime<P, S>,
) -> Option<
    Arc<
        funera_core::middleware::MiddlewareChain<
            AgentEvent,
            funera_core::middleware::ErrorsEnabled,
        >,
    >,
> {
    None
}

/// Relay events from a broadcast receiver to an mpsc sender.
async fn relay_broadcast_to_mpsc(
    mut event_rx: broadcast::Receiver<AgentEvent>,
    relay_tx: mpsc::Sender<AgentEvent>,
) {
    while let Ok(event) = event_rx.recv().await {
        let is_done = matches!(event, AgentEvent::Done);
        if relay_tx.send(event).await.is_err() {
            break;
        }
        if is_done {
            break;
        }
    }
}

/// Aggregate middleware-filtered events from the event stream into a ChatResponse.
async fn aggregate_response(
    event_rx: &mut broadcast::Receiver<AgentEvent>,
    react_result: Result<(), anyhow::Error>,
) -> Result<ChatResponse, OrchestrateError> {
    react_result.map_err(OrchestrateError::Session)?;

    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut iterations = 0usize;
    let mut finish_reason: Option<String> = None;

    // Track pending tool call requests to match with results
    let mut pending_requests: Vec<(Arc<str>, String, serde_json::Value)> = Vec::new();

    loop {
        match event_rx.recv().await {
            Ok(AgentEvent::Text(t)) => {
                content = t;
            }
            Ok(AgentEvent::ToolCallRequest {
                call_id,
                name,
                args,
                ..
            }) => {
                pending_requests.push((call_id, name, args));
            }
            Ok(AgentEvent::ToolCallResult {
                call_id,
                name: _,
                result,
            }) => {
                if let Some(pos) = pending_requests
                    .iter()
                    .position(|(id, _, _)| *id == call_id)
                {
                    let (_, name, args) = pending_requests.remove(pos);
                    tool_calls.push(ToolCallInfo { name, args, result });
                }
            }
            Ok(AgentEvent::TurnStart) => iterations += 1,
            Ok(AgentEvent::TurnEnd { finish_reason: fr }) => finish_reason = fr,
            Ok(AgentEvent::Done) => break,
            Err(broadcast::error::RecvError::Closed) => break,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            _ => {}
        }
    }

    Ok(ChatResponse {
        content,
        tool_calls,
        iterations,
        finish_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── builder ────────────────────────────────────────────────────

    #[test]
    fn builder_minimal_build_succeeds() {
        let agent = AgentBuilder::new().build();
        assert!(agent.system_prompt.is_none());
        assert!(agent.callbacks.is_empty());
    }

    #[test]
    fn builder_system_prompt() {
        let agent = AgentBuilder::new()
            .system_prompt("You are helpful.")
            .build();
        assert_eq!(agent.system_prompt, Some("You are helpful.".into()));
    }

    #[test]
    fn builder_multiple_builds_independent() {
        let a1 = AgentBuilder::new().system_prompt("P1").build();
        let a2 = AgentBuilder::new().system_prompt("P2").build();
        assert_eq!(a1.system_prompt, Some("P1".into()));
        assert_eq!(a2.system_prompt, Some("P2".into()));
    }

    // ── callback registration ──────────────────────────────────────

    #[test]
    fn builder_on_token_registers() {
        let agent = AgentBuilder::new().on_token(|_| {}).build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_tool_call_registers() {
        let agent = AgentBuilder::new().on_tool_call(|_, _| {}).build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_tool_result_registers() {
        let agent = AgentBuilder::new().on_tool_result(|_, _| {}).build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_turn_start_registers() {
        let agent = AgentBuilder::new().on_turn_start(|| {}).build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_turn_end_registers() {
        let agent = AgentBuilder::new().on_turn_end(|| {}).build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_event_registers() {
        let agent = AgentBuilder::new().on_event(|_| {}).build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_all_callbacks_stacked() {
        let agent = AgentBuilder::new()
            .on_token(|_| {})
            .on_tool_call(|_, _| {})
            .on_event(|_| {})
            .build();
        // Each registered callback is one call to add()
        assert!(agent.callbacks.len() >= 3);
    }

    // ── subscribe_events ──────────────────────────────────────────

    #[tokio::test]
    async fn subscribe_events_receives_token() {
        let agent = AgentBuilder::new().build();
        let mut rx = agent.subscribe_events();
        agent
            .event_tx
            .send(AgentEvent::Text("hello".into()))
            .unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await;
        assert!(matches!(got, Ok(Ok(AgentEvent::Text(t))) if t == "hello"));
    }

    #[tokio::test]
    async fn subscribe_events_receives_tool_call() {
        let agent = AgentBuilder::new().build();
        let mut rx = agent.subscribe_events();
        agent
            .event_tx
            .send(AgentEvent::ToolCallRequest {
                index: 0,
                call_id: "call_abc".into(),
                name: "test".into(),
                args: serde_json::json!({}),
            })
            .unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await;
        assert!(matches!(got, Ok(Ok(AgentEvent::ToolCallRequest { .. }))));
    }

    #[tokio::test]
    async fn subscribe_events_multiple_receivers() {
        let agent = AgentBuilder::new().build();
        let mut rx1 = agent.subscribe_events();
        let mut rx2 = agent.subscribe_events();
        agent.event_tx.send(AgentEvent::Done).unwrap();

        let r1 = tokio::time::timeout(std::time::Duration::from_secs(1), rx1.recv()).await;
        let r2 = tokio::time::timeout(std::time::Duration::from_secs(1), rx2.recv()).await;
        assert!(r1.is_ok());
        assert!(r2.is_ok());
    }

    #[tokio::test]
    async fn subscribe_raw_events_receives_raw_token() {
        use funera_core::event_bus::token_bus::TokenEvent;
        let agent = AgentBuilder::new().build();
        let mut rx = agent.subscribe_raw_events();
        agent
            .raw_event_tx
            .send(RawAgentEvent::Token(TokenEvent::Text("raw".into())))
            .unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await;
        assert!(matches!(
            got,
            Ok(Ok(RawAgentEvent::Token(TokenEvent::Text(t)))) if t == "raw"
        ));
    }

    // ── callback firing via dispatch ───────────────────────────────

    #[test]
    fn callbacks_fire_on_dispatch() {
        let counter = Arc::new(AtomicUsize::new(0));
        let agent = AgentBuilder::new()
            .on_event({
                let c = counter.clone();
                move |_| {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
            .build();
        agent.callbacks.dispatch(AgentEvent::Done);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn callbacks_only_fire_matching_event() {
        let token_hits = Arc::new(AtomicUsize::new(0));
        let tool_hits = Arc::new(AtomicUsize::new(0));

        let agent = AgentBuilder::new()
            .on_token({
                let c = token_hits.clone();
                move |_| {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
            .on_tool_call({
                let c = tool_hits.clone();
                move |_, _| {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
            .build();

        agent.callbacks.dispatch(AgentEvent::Text("x".into()));
        assert_eq!(token_hits.load(Ordering::SeqCst), 1);
        assert_eq!(tool_hits.load(Ordering::SeqCst), 0);
    }
}
