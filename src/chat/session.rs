use std::{marker::PhantomData, sync::Arc};

use anyhow::Result;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task::Id};
use uuid::Uuid;

use crate::chat::message::FuneraMessage;
use serde_json::{Value as JsonValue, json};

//States
trait State {}
struct Idle; //no user prompt action happens.
struct Running; //user prompted something that need call llms.
impl State for Idle {}
impl State for Running {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuneraSession<SessionState: State> {
    id: Uuid,
    msgs: Arc<RwLock<Vec<FuneraMessage>>>,
    queued_msg: Arc<RwLock<Vec<FuneraMessage>>>,
    _state: PhantomData<fn() -> SessionState>,
}

impl<SessionState: State> FuneraSession<SessionState> {
    pub fn session_context(&self) -> JsonValue {
        self.msgs
            .read()
            .iter()
            .map(|msg| msg.format_json())
            .collect::<Vec<_>>()
            .into()
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
            _state: PhantomData::<fn() -> Idle>,
        }
    }

    pub fn run(self) -> FuneraSession<Running> {
        FuneraSession::<Running> {
            id: self.id,
            msgs: self.msgs,
            queued_msg: self.queued_msg,
            _state: PhantomData::<fn() -> Running>,
        }
    }
}

impl FuneraSession<Running> {
    pub fn react_loop(&self, init_msg: FuneraMessage) -> Result<()> {
        todo!()
    }
    pub fn idle(self) -> FuneraSession<Idle> {
        FuneraSession::<Idle> {
            id: self.id,
            msgs: self.msgs,
            queued_msg: self.queued_msg,
            _state: PhantomData::<fn() -> Idle>,
        }
    }
}
