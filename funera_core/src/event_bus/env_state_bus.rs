use tokio::sync::{broadcast, mpsc};

use crate::event_bus::{
    react_bus::{ReactBus, ReactEvent},
    token_bus::{TokenBus, TokenEvent},
};

#[derive(Debug, Clone)]
pub enum EnvStateEvent {
    SessionStart,
    SessionClosed,
    LlmChanged(String),
    ToolAdded(String),
    ToolRemoved(String),
    ToolAvailability(String, bool),
}

pub enum TurnHighWayEvent {
    TurnPrepareRequest,
    TurnPrepareResponse {
        token_bus: TokenBus,
        react_bus: ReactBus,
    },
}

pub struct TurnHighWayHandle {
    pub turn_high_way_tx: mpsc::Sender<TurnHighWayEvent>,
    pub turn_high_way_rx: mpsc::Receiver<TurnHighWayEvent>,
}

pub struct EnvStateBus {
    env_state_tx: broadcast::Sender<EnvStateEvent>,
    turn_high_way_handle: TurnHighWayHandle,
}

impl EnvStateBus {
    pub fn new() -> (Self, TurnHighWayHandle) {
        let (turn_high_way_tx1, turn_high_way_rx1) = mpsc::channel(5);
        let (turn_high_way_tx2, turn_high_way_rx2) = mpsc::channel(5);

        let turn_high_way_handle_out = TurnHighWayHandle {
            turn_high_way_tx: turn_high_way_tx2,
            turn_high_way_rx: turn_high_way_rx1,
        };
        let turn_high_way_handle_self = TurnHighWayHandle {
            turn_high_way_tx: turn_high_way_tx1,
            turn_high_way_rx: turn_high_way_rx2,
        };
        (
            Self {
                env_state_tx: broadcast::channel(20).0,
                turn_high_way_handle: turn_high_way_handle_self,
            },
            turn_high_way_handle_out,
        )
    }
}
