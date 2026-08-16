//! Replaceable agent loop abstraction.

use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use funera_core::chat::message::FuneraMessage;
use funera_core::chat::session::FuneraSession;
use funera_core::event_bus::env_state_bus::EnvStateEvent;
use funera_core::middleware::{EventSenderFn, MiddlewareProcessor};
use funera_core::provider::ChatProvider;
use funera_core::re_act::ReActLoopConfig;

use crate::event::AgentEvent;

/// The execution strategy that runs one agent turn.
///
/// The default implementation is the built-in ReAct loop. Users can provide a
/// custom implementation to replace the core loop entirely while keeping the
/// same session, middleware, event, and environment plumbing.
///
/// # Bus contract
///
/// A valid loop implementation must drive the three runtime buses:
///
/// - **Token bus**: call `config.turn_highway_handle.prepare_turn().await` to
///   obtain `(token_tx, react_bus)`, then publish [`TokenEvent`]s on `token_tx`.
/// - **React bus**: publish [`ReactEvent::TurnStart`] before processing and
///   [`ReactEvent::TurnEnd`] after processing. Tool requests/responses must be
///   published with [`ReactEvent::ToolExecRequest`] /
///   [`ReactEvent::ToolExecResponse`].
/// - **Env-state bus**: broadcast session/turn lifecycle events through
///   `env_state_tx` (the surrounding `Agent` already sends `SessionStart` /
///   `SessionClosed`, but a loop should not invent duplicate lifecycle events).
///
/// In addition, the loop must feed the orchestration layer through
/// `event_sender` (including a final [`AgentEvent::Done`]) and keep the session
/// history consistent by pushing assistant/tool messages to `session`.
#[async_trait]
pub trait AgentLoop: Send + Sync + 'static {
    /// Run one turn for `session` with the given init message and config.
    async fn run(
        &self,
        session: &FuneraSession,
        init_msg: FuneraMessage,
        config: ReActLoopConfig,
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        middleware: Option<Arc<dyn MiddlewareProcessor<AgentEvent>>>,
        event_sender: Option<EventSenderFn<AgentEvent>>,
    ) -> anyhow::Result<()>;
}

/// The built-in ReAct loop as an [`AgentLoop`].
pub struct DefaultAgentLoop<P> {
    _phantom: PhantomData<fn() -> P>,
}

impl<P> Default for DefaultAgentLoop<P> {
    fn default() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

#[async_trait]
impl<P: ChatProvider + 'static> AgentLoop for DefaultAgentLoop<P> {
    async fn run(
        &self,
        session: &FuneraSession,
        init_msg: FuneraMessage,
        config: ReActLoopConfig,
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        middleware: Option<Arc<dyn MiddlewareProcessor<AgentEvent>>>,
        event_sender: Option<EventSenderFn<AgentEvent>>,
    ) -> anyhow::Result<()> {
        session
            .react_loop::<P, AgentEvent>(init_msg, config, env_state_tx, middleware, event_sender)
            .await
    }
}
