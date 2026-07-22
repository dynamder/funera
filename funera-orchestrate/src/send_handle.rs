use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value as JsonValue;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use funera_core::chat::session::SessionCmd;
use funera_core::event_bus::env_state_bus::EnvStateEvent;
use funera_core::provider::ChatProvider;

use crate::error::OrchestrateError;
use crate::event::AgentEvent;
use crate::response::{ChatResponse, ToolCallInfo};
use crate::runtime::{Acquired, AgentRuntime, Idle};

// ═══════════════════════════════════════════════════════════════
// SendHandle — stateful send (consumes Idle, returns Idle on wait)
// ═══════════════════════════════════════════════════════════════

/// Handle returned by [`Agent::send`](crate::Agent::send).
///
/// Holds an `AgentRuntime<Acquired>` — cannot send again until `wait()` completes.
/// The react_loop runs in a background task; you can query session context via
/// [`session_context`](Self::session_context) while it is in progress.
///
/// # Awaiting
///
/// ```rust,no_run
/// # use funera_orchestrate::{Agent, AgentRuntime, DeepSeekProvider};
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let agent = Agent::builder().build();
/// # let rt = AgentRuntime::<DeepSeekProvider>::builder()
/// #     .api_key(std::env::var("DEEPSEEK_API_KEY")?).build()?;
/// let handle = agent.send("Hello", rt).await?;
/// let (_rt, resp) = handle.await?;    // IntoFuture → (Idle, ChatResponse)
/// # Ok(())
/// # }
/// ```
pub struct SendHandle<P: ChatProvider> {
    pub(crate) runtime: AgentRuntime<P, Acquired>,
    pub(crate) handle: JoinHandle<anyhow::Result<()>>,
    pub(crate) event_rx: broadcast::Receiver<AgentEvent>,
    pub(crate) env_state_tx: broadcast::Sender<EnvStateEvent>,
}

impl<P: ChatProvider> SendHandle<P> {
    /// Query session context while react_loop is running.
    pub async fn session_context(&self) -> Vec<JsonValue> {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .runtime
            .session_tx
            .send(SessionCmd::FetchContext { respond });
        rx.await.unwrap_or_default()
    }

    async fn wait(self) -> Result<(AgentRuntime<P, Idle>, ChatResponse), OrchestrateError> {
        // Wait for react_loop to complete
        self.handle
            .await
            .map_err(|e| OrchestrateError::Session(e.into()))??;

        let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);

        // Aggregate filtered events into ChatResponse
        let resp = aggregate_from_broadcast(self.event_rx).await?;

        Ok((self.runtime.into_idle(), resp))
    }
}

impl<P: ChatProvider + 'static> IntoFuture for SendHandle<P> {
    type Output = Result<(AgentRuntime<P, Idle>, ChatResponse), OrchestrateError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.wait().await })
    }
}

// ═══════════════════════════════════════════════════════════════
// SendStreamHandle — stateful send + streaming
// ═══════════════════════════════════════════════════════════════

/// Handle returned by [`Agent::send_stream`](crate::Agent::send_stream).
///
/// Like [`SendHandle`] but also provides `recv()` for per-event streaming.
pub struct SendStreamHandle<P: ChatProvider> {
    pub(crate) runtime: AgentRuntime<P, Acquired>,
    pub(crate) handle: JoinHandle<anyhow::Result<()>>,
    pub(crate) event_rx: broadcast::Receiver<AgentEvent>,
    pub(crate) stream_rx: mpsc::Receiver<AgentEvent>,
    pub(crate) env_state_tx: broadcast::Sender<EnvStateEvent>,
}

impl<P: ChatProvider> SendStreamHandle<P> {
    /// Receive the next streaming event.
    pub async fn recv(&mut self) -> Option<AgentEvent> {
        self.stream_rx.recv().await
    }

    /// Query session context while streaming is in progress.
    pub async fn session_context(&self) -> Vec<JsonValue> {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .runtime
            .session_tx
            .send(SessionCmd::FetchContext { respond });
        rx.await.unwrap_or_default()
    }

    async fn wait(self) -> Result<(AgentRuntime<P, Idle>, ChatResponse), OrchestrateError> {
        self.handle
            .await
            .map_err(|e| OrchestrateError::Session(e.into()))??;
        let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);
        let resp = aggregate_from_broadcast(self.event_rx).await?;
        Ok((self.runtime.into_idle(), resp))
    }
}

impl<P: ChatProvider + 'static> IntoFuture for SendStreamHandle<P> {
    type Output = Result<(AgentRuntime<P, Idle>, ChatResponse), OrchestrateError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.wait().await })
    }
}

// ═══════════════════════════════════════════════════════════════
// FireStreamHandle — stateless stream (no runtime binding)
// ═══════════════════════════════════════════════════════════════

/// Handle returned by [`Agent::fire_stream`](crate::Agent::fire_stream).
///
/// Unlike `SendHandle`, this does NOT hold an `AgentRuntime` — the session is
/// temporary. Provides `recv()` for streaming and `wait()` (`IntoFuture`) for
/// the final [`ChatResponse`].
pub struct FireStreamHandle {
    pub(crate) handle: JoinHandle<anyhow::Result<()>>,
    pub(crate) event_rx: broadcast::Receiver<AgentEvent>,
    pub(crate) stream_rx: mpsc::Receiver<AgentEvent>,
    pub(crate) env_state_tx: broadcast::Sender<EnvStateEvent>,
}

impl FireStreamHandle {
    /// Receive the next streaming event.
    pub async fn recv(&mut self) -> Option<AgentEvent> {
        self.stream_rx.recv().await
    }

    async fn wait(self) -> Result<ChatResponse, OrchestrateError> {
        self.handle
            .await
            .map_err(|e| OrchestrateError::Session(e.into()))??;
        let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);
        aggregate_from_broadcast(self.event_rx).await
    }
}

impl IntoFuture for FireStreamHandle {
    type Output = Result<ChatResponse, OrchestrateError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.wait().await })
    }
}

// ═══════════════════════════════════════════════════════════════
// Internal helpers
// ═══════════════════════════════════════════════════════════════

/// Aggregate filtered events from a broadcast receiver into a ChatResponse.
async fn aggregate_from_broadcast(
    mut event_rx: broadcast::Receiver<AgentEvent>,
) -> Result<ChatResponse, OrchestrateError> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut iterations = 0usize;
    let mut finish_reason: Option<String> = None;
    let mut pending: Vec<(Arc<str>, String, serde_json::Value)> = Vec::new();

    loop {
        match event_rx.recv().await {
            Ok(AgentEvent::Text(t)) => content = t,
            Ok(AgentEvent::ToolCallRequest {
                call_id,
                name,
                args,
                ..
            }) => {
                pending.push((call_id, name, args));
            }
            Ok(AgentEvent::ToolCallResult {
                call_id,
                name: _,
                result,
            }) => {
                if let Some(pos) = pending.iter().position(|(id, _, _)| *id == call_id) {
                    let (_, name, args) = pending.remove(pos);
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
