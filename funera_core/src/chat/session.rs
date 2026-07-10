use std::{marker::PhantomData, sync::Arc};

use anyhow::Result;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::{
    chat::message::FuneraMessage,
    event_bus::env_state_bus::EnvStateEvent,
    middleware::{ErrorsEnabled, EventSenderFn, MiddlewareChain, MiddlewareEvent},
    re_act::{ReActLoop, ReActLoopConfig, ReActLoopHandle},
};
use tokio::sync::broadcast;

pub trait State {}
pub struct Idle;
pub struct Running;
impl State for Idle {}
impl State for Running {}

#[derive(Debug, Serialize, Deserialize)]
pub struct FuneraSession<SessionState: State> {
    id: Uuid,
    msgs: Arc<RwLock<Vec<FuneraMessage>>>,
    queued_msg: Arc<RwLock<Vec<FuneraMessage>>>,

    #[serde(skip)]
    current_loop: Option<ReActLoopHandle>,

    _state: PhantomData<fn() -> SessionState>,
}

impl<SessionState: State> FuneraSession<SessionState> {
    pub fn session_context(&self) -> Vec<JsonValue> {
        self.msgs
            .read()
            .iter()
            .map(|msg| msg.format_json())
            .collect::<Vec<_>>()
    }

    pub fn id(&self) -> Uuid {
        self.id
    }
}

impl FuneraSession<Idle> {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            msgs: Arc::new(RwLock::new(Vec::new())),
            queued_msg: Arc::new(RwLock::new(Vec::new())),
            current_loop: None,
            _state: PhantomData::<fn() -> Idle>,
        }
    }

    /// Push a message into the session's message history before it runs.
    /// Useful for injecting system prompts or pre-seeding the conversation.
    pub fn push_message(&self, msg: FuneraMessage) {
        self.msgs.write().push(msg);
    }

    pub fn run(self) -> FuneraSession<Running> {
        FuneraSession::<Running> {
            id: self.id,
            msgs: self.msgs,
            queued_msg: self.queued_msg,
            current_loop: None,
            _state: PhantomData::<fn() -> Running>,
        }
    }
}

impl FuneraSession<Running> {
    pub async fn react_loop<P: crate::provider::ChatProvider, E: MiddlewareEvent>(
        &mut self,
        init_msg: FuneraMessage,
        mut config: ReActLoopConfig,
        env_state_tx: broadcast::Sender<EnvStateEvent>,
        middleware: Option<Arc<MiddlewareChain<E, ErrorsEnabled>>>,
        event_sender: Option<EventSenderFn<E>>,
    ) -> Result<()> {
        let _ = env_state_tx.send(EnvStateEvent::SessionStart);

        {
            let mut msgs = self.msgs.write();
            msgs.push(init_msg.clone());
        }

        config.session_msgs = Some(self.msgs.clone());
        let react_loop = ReActLoop::<P>::from_config(config);
        let sender = react_loop.sender();
        sender.send(init_msg).await?;

        let loop_handle = react_loop.run::<E>(middleware, event_sender);
        self.current_loop = Some(loop_handle);

        if let Some(handle) = self.current_loop.take() {
            handle.task.await??;
        }

        let _ = env_state_tx.send(EnvStateEvent::SessionClosed);
        Ok(())
    }

    pub fn idle(self) -> FuneraSession<Idle> {
        if let Some(handle) = &self.current_loop {
            handle.cancel_token.cancel();
        }
        FuneraSession::<Idle> {
            id: self.id,
            msgs: self.msgs,
            queued_msg: self.queued_msg,
            current_loop: None,
            _state: PhantomData::<fn() -> Idle>,
        }
    }
}
