use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use funera_core::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};
use funera_core::chat::session::{FuneraSession, Idle};
use funera_core::event_bus::env_state_bus::{EnvStateBus, EnvStateEvent};
use funera_core::provider::ChatProvider;
use funera_core::re_act::ReActLoopConfig;

use crate::dispatcher::{CallbackDispatcher, CallbackRegistry};
use crate::error::OrchestrateError;
use crate::event::AgentEvent;
use crate::response::ChatResponse;
use crate::runtime::AgentRuntime;

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
            if let AgentEvent::Token(t) = event {
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
            if let AgentEvent::ToolCallStart { name, args, .. } = event {
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
            if matches!(event, AgentEvent::TurnEnd) {
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
        Agent {
            system_prompt: self.system_prompt,
            callbacks: Arc::new(self.callbacks),
            event_tx,
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
///     .api_key(std::env::var("OPENAI_API_KEY")?)
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
/// let mut runtime = AgentRuntime::<DeepSeekProvider>::builder()
///     .api_key(std::env::var("OPENAI_API_KEY")?)
///     .build()?;
///
/// let agent = Agent::builder().build();
///
/// // send uses the runtime's session — runtime needs &mut
/// agent.send("My name is Alice.", &mut runtime).await?;
/// agent.send("What is my name?", &mut runtime).await?;
/// // → "Alice"
/// # Ok(())
/// # }
/// ```
pub struct Agent {
    pub(crate) system_prompt: Option<String>,
    pub(crate) callbacks: Arc<CallbackRegistry>,
    pub(crate) event_tx: broadcast::Sender<AgentEvent>,
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

    // ── fire (one-shot, no session) ────────────────────────────────

    /// One-shot query. Creates a temporary session, runs a single ReAct loop,
    /// and discards the session.  The runtime is accessed via a shared `&`
    /// reference — no session state is mutated.
    ///
    /// Use [`send`](Self::send) instead if you need multi-turn conversation.
    pub async fn fire<P: ChatProvider>(
        &self,
        msg: impl Into<String>,
        runtime: &AgentRuntime<P>,
    ) -> Result<ChatResponse, OrchestrateError> {
        let text = msg.into();

        // Create per-call plumbing
        let (env_state_bus, turn_highway_handle) = EnvStateBus::new();
        let env_state_tx = env_state_bus.env_state_tx.clone();
        let env_state_rx = env_state_bus.subscribe();
        env_state_bus.start_turn_highway();

        // Per-call dispatcher
        let _dispatcher =
            CallbackDispatcher::new(env_state_rx, self.callbacks.clone(), self.event_tx.clone());

        let _ = env_state_tx.send(EnvStateEvent::SessionStart);

        // Fresh temp session — never stored back
        let session = FuneraSession::<Idle>::new();

        // Inject system prompt
        if let Some(ref sys) = self.system_prompt {
            let sys_msg = FuneraMessage::new(
                Role::System,
                MsgVariant::Text(TextMessage {
                    text: sys.clone().into(),
                    reasoning_content: None,
                }),
            );
            session.push_message(sys_msg);
        }

        let mut running = session.run();

        let init_msg = FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage {
                text: text.clone().into(),
                reasoning_content: None,
            }),
        );

        let config = ReActLoopConfig::new(
            runtime.channel_buffer(),
            runtime.max_iterations(),
            runtime.env_watcher(),
            runtime.tool_bus.clone(),
            env_state_tx.clone(),
            turn_highway_handle,
        );

        let result = running
            .react_loop::<P>(init_msg, config, env_state_tx.clone())
            .await;
        let idle_session = running.idle();
        let _ = env_state_tx.send(EnvStateEvent::SessionClosed);

        Self::extract_response(result, idle_session)
    }

    /// Streaming variant of [`fire`](Self::fire).
    ///
    /// Returns a channel receiver that yields [`AgentEvent`] items as they
    /// happen (tokens, tool calls, turn boundaries), ending with `Done`.
    pub async fn fire_stream<P: ChatProvider>(
        &self,
        msg: impl Into<String>,
        runtime: &AgentRuntime<P>,
    ) -> Result<mpsc::Receiver<AgentEvent>, OrchestrateError> {
        let (relay_tx, rx) = mpsc::channel(256);
        let tx = relay_tx.clone();
        let event_rx = self.event_tx.subscribe();

        tokio::spawn(async move {
            let mut rx = event_rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if tx.send(event.clone()).await.is_err() {
                            break;
                        }
                        if matches!(event, AgentEvent::Done) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let _ = self.fire(msg, runtime).await?;
        let _ = relay_tx.send(AgentEvent::Done).await;
        Ok(rx)
    }

    // ── send (multi-turn, persistent session) ──────────────────────

    /// Multi-turn message. Uses the runtime's persistent session, storing
    /// the updated session back into the runtime on completion.
    ///
    /// The runtime requires `&mut` because the session state is mutated.
    pub async fn send<P: ChatProvider>(
        &self,
        msg: impl Into<String>,
        runtime: &mut AgentRuntime<P>,
    ) -> Result<ChatResponse, OrchestrateError> {
        let text = msg.into();

        // Per-call plumbing
        let (env_state_bus, turn_highway_handle) = EnvStateBus::new();
        let env_state_tx = env_state_bus.env_state_tx.clone();
        let env_state_rx = env_state_bus.subscribe();
        env_state_bus.start_turn_highway();

        // Per-call dispatcher
        let _dispatcher =
            CallbackDispatcher::new(env_state_rx, self.callbacks.clone(), self.event_tx.clone());

        let _ = env_state_tx.send(EnvStateEvent::SessionStart);

        // Take or create session
        let session = runtime.take_session();

        // Inject system prompt on first interaction (session is fresh)
        if let Some(ref sys) = self.system_prompt {
            let msgs = session.session_context();
            if msgs.is_empty() {
                let sys_msg = FuneraMessage::new(
                    Role::System,
                    MsgVariant::Text(TextMessage {
                        text: sys.clone().into(),
                        reasoning_content: None,
                    }),
                );
                session.push_message(sys_msg);
            }
        }

        let mut running = session.run();

        let init_msg = FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage { text: text.into(), reasoning_content: None }),
        );

        let config = ReActLoopConfig::new(
            runtime.channel_buffer(),
            runtime.max_iterations(),
            runtime.env_watcher(),
            runtime.tool_bus.clone(),
            env_state_tx.clone(),
            turn_highway_handle,
        );

        let result = running
            .react_loop::<P>(init_msg, config, env_state_tx.clone())
            .await;
        let idle_session = running.idle();
        let _ = env_state_tx.send(EnvStateEvent::SessionClosed);

        // Extract context before consuming the session
        let ctx = idle_session.session_context();

        let response = match result {
            Ok(()) => {
                let assistant_msgs: Vec<&serde_json::Value> =
                    ctx.iter().filter(|m| m["role"] == "assistant").collect();

                let mut content = String::new();
                for m in &assistant_msgs {
                    if let Some(c) = m["content"].as_str() {
                        if !c.is_empty() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(c);
                        }
                    }
                }

                let finish_reason = assistant_msgs
                    .last()
                    .and_then(|m| m["finish_reason"].as_str())
                    .map(|s| s.to_string());

                Ok(ChatResponse {
                    content,
                    tool_calls: Vec::new(),
                    iterations: assistant_msgs.len(),
                    finish_reason,
                })
            }
            Err(e) => Err(OrchestrateError::Session(e)),
        };

        // Store session back for multi-turn
        if response.is_ok() {
            runtime.store_session(idle_session);
        }

        response
    }

    /// Streaming variant of [`send`](Self::send).
    pub async fn send_stream<P: ChatProvider>(
        &self,
        msg: impl Into<String>,
        runtime: &mut AgentRuntime<P>,
    ) -> Result<mpsc::Receiver<AgentEvent>, OrchestrateError> {
        let (relay_tx, rx) = mpsc::channel(256);
        let tx = relay_tx.clone();
        let event_rx = self.event_tx.subscribe();

        tokio::spawn(async move {
            let mut rx = event_rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if tx.send(event.clone()).await.is_err() {
                            break;
                        }
                        if matches!(event, AgentEvent::Done) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let _ = self.send(msg, runtime).await?;
        let _ = relay_tx.send(AgentEvent::Done).await;
        Ok(rx)
    }

    // ── internal helpers ──────────────────────────────────────────

    fn extract_response(
        result: Result<(), anyhow::Error>,
        session: FuneraSession<Idle>,
    ) -> Result<ChatResponse, OrchestrateError> {
        match result {
            Ok(()) => {
                let ctx = session.session_context();
                let assistant_msgs: Vec<&serde_json::Value> =
                    ctx.iter().filter(|m| m["role"] == "assistant").collect();

                let mut content = String::new();
                for m in &assistant_msgs {
                    if let Some(c) = m["content"].as_str() {
                        if !c.is_empty() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(c);
                        }
                    }
                }

                let finish_reason = assistant_msgs
                    .last()
                    .and_then(|m| m["finish_reason"].as_str())
                    .map(|s| s.to_string());

                Ok(ChatResponse {
                    content,
                    tool_calls: Vec::new(),
                    iterations: assistant_msgs.len(),
                    finish_reason,
                })
            }
            Err(e) => Err(OrchestrateError::Session(e)),
        }
    }
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
        let agent = AgentBuilder::new()
            .on_token(|_| {})
            .build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_tool_call_registers() {
        let agent = AgentBuilder::new()
            .on_tool_call(|_, _| {})
            .build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_tool_result_registers() {
        let agent = AgentBuilder::new()
            .on_tool_result(|_, _| {})
            .build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_turn_start_registers() {
        let agent = AgentBuilder::new()
            .on_turn_start(|| {})
            .build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_turn_end_registers() {
        let agent = AgentBuilder::new()
            .on_turn_end(|| {})
            .build();
        assert!(!agent.callbacks.is_empty());
    }

    #[test]
    fn builder_on_event_registers() {
        let agent = AgentBuilder::new()
            .on_event(|_| {})
            .build();
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
            .send(AgentEvent::Token("hello".into()))
            .unwrap();
        let got = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            rx.recv(),
        )
        .await;
        assert!(matches!(got, Ok(Ok(AgentEvent::Token(t))) if t == "hello"));
    }

    #[tokio::test]
    async fn subscribe_events_receives_tool_call() {
        let agent = AgentBuilder::new().build();
        let mut rx = agent.subscribe_events();
        agent
            .event_tx
            .send(AgentEvent::ToolCallStart {
                index: 0,
                call_id: uuid::Uuid::new_v4(),
                name: "test".into(),
                args: serde_json::json!({}),
            })
            .unwrap();
        let got = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            rx.recv(),
        )
        .await;
        assert!(matches!(got, Ok(Ok(AgentEvent::ToolCallStart { .. }))));
    }

    #[tokio::test]
    async fn subscribe_events_multiple_receivers() {
        let agent = AgentBuilder::new().build();
        let mut rx1 = agent.subscribe_events();
        let mut rx2 = agent.subscribe_events();
        agent
            .event_tx
            .send(AgentEvent::Done)
            .unwrap();

        let r1 = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            rx1.recv(),
        )
        .await;
        let r2 = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            rx2.recv(),
        )
        .await;
        assert!(r1.is_ok());
        assert!(r2.is_ok());
    }

    // ── callback firing via dispatch ───────────────────────────────

    #[test]
    fn callbacks_fire_on_dispatch() {
        let counter = Arc::new(AtomicUsize::new(0));
        let agent = AgentBuilder::new()
            .on_event({
                let c = counter.clone();
                move |_| { c.fetch_add(1, Ordering::SeqCst); }
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
                move |_| { c.fetch_add(1, Ordering::SeqCst); }
            })
            .on_tool_call({
                let c = tool_hits.clone();
                move |_, _| { c.fetch_add(1, Ordering::SeqCst); }
            })
            .build();

        agent.callbacks.dispatch(AgentEvent::Token("x".into()));
        assert_eq!(token_hits.load(Ordering::SeqCst), 1);
        assert_eq!(tool_hits.load(Ordering::SeqCst), 0);
    }
}
