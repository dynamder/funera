use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value as JsonValue;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use funera_core::chat::session::SessionCmd;
use funera_core::event_bus::env_state_bus::EnvStateEvent;
use funera_core::event_bus::token_bus::TokenUsage;
use funera_core::provider::ChatProvider;
use tokio_util::sync::CancellationToken;

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
    pub(crate) runtime: Option<AgentRuntime<P, Acquired>>,
    pub(crate) handle: Option<JoinHandle<anyhow::Result<()>>>,
    pub(crate) event_rx: Option<broadcast::Receiver<AgentEvent>>,
    pub(crate) env_state_tx: broadcast::Sender<EnvStateEvent>,
    pub(crate) cancel: CancellationToken,
}

impl<P: ChatProvider> SendHandle<P> {
    /// Query session context while react_loop is running.
    pub async fn session_context(&self) -> Vec<JsonValue> {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .runtime
            .as_ref()
            .expect("runtime present")
            .session_tx
            .send(SessionCmd::FetchContext { respond });
        rx.await.unwrap_or_default()
    }

    /// Returns a cloneable [`ApprovalHandle`] for approving tool calls
    /// during react_loop execution.
    #[cfg(all(feature = "tool", feature = "security"))]
    pub fn approval_handle(&self) -> ApprovalHandle {
        ApprovalHandle::new(
            self.runtime
                .as_ref()
                .expect("runtime present")
                .env_cmd_tx
                .clone(),
        )
    }

    async fn wait(mut self) -> Result<(AgentRuntime<P, Idle>, ChatResponse), OrchestrateError> {
        // Wait for react_loop to complete (an aborted task surfaces as Cancelled).
        let handle = self.handle.take().expect("handle taken once");
        handle.await.map_err(|_| OrchestrateError::Cancelled)??;

        let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);

        if self.cancel.is_cancelled() {
            return Err(OrchestrateError::Cancelled);
        }

        // Aggregate filtered events into ChatResponse
        let event_rx = self.event_rx.take().expect("event rx taken once");
        let resp = aggregate_from_broadcast(event_rx).await?;

        let runtime = self.runtime.take().expect("runtime taken once");
        Ok((runtime.into_idle(), resp))
    }
}

impl<P: ChatProvider + 'static> IntoFuture for SendHandle<P> {
    type Output = Result<(AgentRuntime<P, Idle>, ChatResponse), OrchestrateError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.wait().await })
    }
}

impl<P: ChatProvider> Drop for SendHandle<P> {
    fn drop(&mut self) {
        drop_cancel(&self.cancel);
    }
}

// ═══════════════════════════════════════════════════════════════
// SendStreamHandle — stateful send + streaming
// ═══════════════════════════════════════════════════════════════

/// Handle returned by [`Agent::send_stream`](crate::Agent::send_stream).
///
/// Like [`SendHandle`] but also provides `recv()` for per-event streaming.
pub struct SendStreamHandle<P: ChatProvider> {
    pub(crate) runtime: Option<AgentRuntime<P, Acquired>>,
    pub(crate) handle: Option<JoinHandle<anyhow::Result<()>>>,
    pub(crate) event_rx: Option<broadcast::Receiver<AgentEvent>>,
    pub(crate) stream_rx: Option<mpsc::Receiver<AgentEvent>>,
    pub(crate) env_state_tx: broadcast::Sender<EnvStateEvent>,
    pub(crate) cancel: CancellationToken,
}

impl<P: ChatProvider> SendStreamHandle<P> {
    /// Receive the next streaming event.
    ///
    /// Returns `None` once the call is cancelled (or the stream is exhausted).
    pub async fn recv(&mut self) -> Option<AgentEvent> {
        if self.cancel.is_cancelled() {
            return None;
        }
        self.stream_rx.as_mut()?.recv().await
    }

    /// Query session context while streaming is in progress.
    pub async fn session_context(&self) -> Vec<JsonValue> {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .runtime
            .as_ref()
            .expect("runtime present")
            .session_tx
            .send(SessionCmd::FetchContext { respond });
        rx.await.unwrap_or_default()
    }

    /// Returns a cloneable [`ApprovalHandle`] for approving tool calls
    /// during react_loop execution.
    #[cfg(all(feature = "tool", feature = "security"))]
    pub fn approval_handle(&self) -> ApprovalHandle {
        ApprovalHandle::new(
            self.runtime
                .as_ref()
                .expect("runtime present")
                .env_cmd_tx
                .clone(),
        )
    }

    async fn wait(mut self) -> Result<(AgentRuntime<P, Idle>, ChatResponse), OrchestrateError> {
        let handle = self.handle.take().expect("handle taken once");
        handle.await.map_err(|_| OrchestrateError::Cancelled)??;
        let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);
        if self.cancel.is_cancelled() {
            return Err(OrchestrateError::Cancelled);
        }
        let event_rx = self.event_rx.take().expect("event rx taken once");
        let resp = aggregate_from_broadcast(event_rx).await?;
        let runtime = self.runtime.take().expect("runtime taken once");
        Ok((runtime.into_idle(), resp))
    }
}

impl<P: ChatProvider + 'static> IntoFuture for SendStreamHandle<P> {
    type Output = Result<(AgentRuntime<P, Idle>, ChatResponse), OrchestrateError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.wait().await })
    }
}

impl<P: ChatProvider> Drop for SendStreamHandle<P> {
    fn drop(&mut self) {
        drop_cancel(&self.cancel);
    }
}

// ═══════════════════════════════════════════════════════════════
// FireHandle — one-shot result handle (no per-event stream)
// ═══════════════════════════════════════════════════════════════

/// Handle returned by [`Agent::fire`](crate::Agent::fire).
///
/// One-shot (temporary session): `cancel()` interrupts the call and stops all
/// background work spawned for it, `await`/`wait()` yields the final
/// [`ChatResponse`]. Dropping the handle cancels automatically.
pub struct FireHandle {
    pub(crate) handle: Option<JoinHandle<anyhow::Result<()>>>,
    pub(crate) cancel: CancellationToken,
    pub(crate) event_rx: Option<broadcast::Receiver<AgentEvent>>,
    pub(crate) env_state_tx: broadcast::Sender<EnvStateEvent>,
}

impl FireHandle {
    /// Immediately interrupt this call: stop receiving output, abandon
    /// in-flight tool executions, and notify middleware via
    /// [`AgentEvent::Cancelled`].
    pub fn cancel(&self) {
        drop_cancel(&self.cancel);
    }

    async fn wait(mut self) -> Result<ChatResponse, OrchestrateError> {
        let handle = self.handle.take().expect("handle taken once");
        handle.await.map_err(|_| OrchestrateError::Cancelled)??;
        let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);
        if self.cancel.is_cancelled() {
            return Err(OrchestrateError::Cancelled);
        }
        let event_rx = self.event_rx.take().expect("event rx taken once");
        aggregate_from_broadcast(event_rx).await
    }
}

impl IntoFuture for FireHandle {
    type Output = Result<ChatResponse, OrchestrateError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.wait().await })
    }
}

impl Drop for FireHandle {
    fn drop(&mut self) {
        drop_cancel(&self.cancel);
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
    pub(crate) handle: Option<JoinHandle<anyhow::Result<()>>>,
    pub(crate) event_rx: Option<broadcast::Receiver<AgentEvent>>,
    pub(crate) stream_rx: Option<mpsc::Receiver<AgentEvent>>,
    pub(crate) env_state_tx: broadcast::Sender<EnvStateEvent>,
    pub(crate) cancel: CancellationToken,
}

impl FireStreamHandle {
    /// Receive the next streaming event.
    ///
    /// Returns `None` once the call is cancelled (or the stream is exhausted).
    pub async fn recv(&mut self) -> Option<AgentEvent> {
        if self.cancel.is_cancelled() {
            return None;
        }
        self.stream_rx.as_mut()?.recv().await
    }

    /// Immediately interrupt this call: stop receiving output, abandon
    /// in-flight tool executions, and notify middleware via
    /// [`AgentEvent::Cancelled`].
    pub fn cancel(&self) {
        drop_cancel(&self.cancel);
    }

    async fn wait(mut self) -> Result<ChatResponse, OrchestrateError> {
        let handle = self.handle.take().expect("handle taken once");
        handle.await.map_err(|_| OrchestrateError::Cancelled)??;
        let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);
        if self.cancel.is_cancelled() {
            return Err(OrchestrateError::Cancelled);
        }
        let event_rx = self.event_rx.take().expect("event rx taken once");
        aggregate_from_broadcast(event_rx).await
    }
}

impl IntoFuture for FireStreamHandle {
    type Output = Result<ChatResponse, OrchestrateError>;
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.wait().await })
    }
}

impl Drop for FireStreamHandle {
    fn drop(&mut self) {
        drop_cancel(&self.cancel);
    }
}

// ═══════════════════════════════════════════════════════════════
// Internal helpers
// ═══════════════════════════════════════════════════════════════

/// Cancel a call's token.
///
/// Cancellation is fully cooperative: every blocking await point of the ReAct
/// loop — stream consumption, the initial provider request, in-flight tool
/// executions — listens for the token via `select!`, so cancelling makes the
/// loop exit promptly and emit [`AgentEvent::Cancelled`] to middleware and
/// subscribers. No abort is needed, which keeps the notification reliable.
/// Used by every handle's `cancel()` and `Drop`.
fn drop_cancel(cancel: &CancellationToken) {
    cancel.cancel();
}

/// Aggregate filtered events from a broadcast receiver into a ChatResponse.
async fn aggregate_from_broadcast(
    mut event_rx: broadcast::Receiver<AgentEvent>,
) -> Result<ChatResponse, OrchestrateError> {
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    let mut iterations = 0usize;
    let mut finish_reason: Option<String> = None;
    let mut usage: Option<TokenUsage> = None;
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
            Ok(AgentEvent::TurnEnd {
                finish_reason: fr,
                usage: u,
            }) => {
                finish_reason = fr;
                usage = u;
            }
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
        usage,
    })
}

// ═══════════════════════════════════════════════════════════════
// ApprovalHandle — tool call approval for spawned tasks
// ═══════════════════════════════════════════════════════════════

/// A lightweight, cloneable handle for approving or rejecting tool calls
/// during agent execution.
///
/// Obtain one **before** calling [`send`](crate::Agent::send) /
/// [`send_stream`](crate::Agent::send_stream) via
/// [`AgentRuntime::approval_handle`](crate::AgentRuntime::approval_handle),
/// or afterwards from
/// [`SendHandle::approval_handle`] / [`SendStreamHandle::approval_handle`].
///
/// Because `ApprovalHandle` implements [`Clone`], it can be freely moved
/// into spawned tasks for background approval while the main task awaits
/// the send handle's result.
#[cfg(all(feature = "tool", feature = "security"))]
#[derive(Clone)]
pub struct ApprovalHandle {
    env_cmd_tx: mpsc::UnboundedSender<funera_core::env_actor::EnvCmd>,
}

#[cfg(all(feature = "tool", feature = "security"))]
impl ApprovalHandle {
    pub(crate) fn new(env_cmd_tx: mpsc::UnboundedSender<funera_core::env_actor::EnvCmd>) -> Self {
        Self { env_cmd_tx }
    }

    /// Approve or reject a pending tool call identified by `call_id`.
    ///
    /// `call_id` is typically obtained from the
    /// [`on_approval_required`](crate::AgentRuntimeBuilder::on_approval_required)
    /// callback. Returns `Ok(())` if the approval was processed, or
    /// `Err(msg)` if the call_id was not found or the env actor has died.
    pub async fn approve_tool_call(&self, call_id: &str, approved: bool) -> Result<(), String> {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self
            .env_cmd_tx
            .send(funera_core::env_actor::EnvCmd::ApproveToolCall {
                call_id: call_id.to_string(),
                approved,
                respond,
            });
        rx.await.unwrap_or(Err("env actor died".into()))
    }
}
