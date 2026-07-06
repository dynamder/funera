use tokio::sync::{broadcast, mpsc};

use crate::event_bus::{
    react_bus::ReactBus,
    token_bus::TokenEvent,
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
        token_tx: broadcast::Sender<TokenEvent>,
        react_bus: ReactBus,
    },
}

pub struct TurnHighWayHandle {
    pub turn_high_way_tx: mpsc::Sender<TurnHighWayEvent>,
    pub turn_high_way_rx: mpsc::Receiver<TurnHighWayEvent>,
}

impl TurnHighWayHandle {
    pub async fn prepare_turn(&mut self) -> (broadcast::Sender<TokenEvent>, ReactBus) {
        let _ = self
            .turn_high_way_tx
            .send(TurnHighWayEvent::TurnPrepareRequest)
            .await;

        match self.turn_high_way_rx.recv().await {
            Some(TurnHighWayEvent::TurnPrepareResponse { token_tx, react_bus }) => {
                (token_tx, react_bus)
            }
            _ => {
                let (token_tx, _) = broadcast::channel(50);
                (token_tx, ReactBus::new())
            }
        }
    }
}

pub struct EnvStateBus {
    pub env_state_tx: broadcast::Sender<EnvStateEvent>,
    turn_high_way_handle: TurnHighWayHandle,
}

impl EnvStateBus {
    pub fn new() -> (Self, TurnHighWayHandle) {
        let (tx1, rx1) = mpsc::channel(5);
        let (tx2, rx2) = mpsc::channel(5);

        let handle_out = TurnHighWayHandle {
            turn_high_way_tx: tx2,
            turn_high_way_rx: rx1,
        };
        let handle_self = TurnHighWayHandle {
            turn_high_way_tx: tx1,
            turn_high_way_rx: rx2,
        };
        (
            Self {
                env_state_tx: broadcast::channel(20).0,
                turn_high_way_handle: handle_self,
            },
            handle_out,
        )
    }

    pub fn start_turn_highway(self) {
        tokio::spawn(async move {
            let mut rx = self.turn_high_way_handle.turn_high_way_rx;
            let tx = self.turn_high_way_handle.turn_high_way_tx;
            while let Some(event) = rx.recv().await {
                match event {
                    TurnHighWayEvent::TurnPrepareRequest => {
                        let (token_tx, _) = broadcast::channel(50);
                        let react_bus = ReactBus::new();
                        let _ = tx
                            .send(TurnHighWayEvent::TurnPrepareResponse {
                                token_tx,
                                react_bus,
                            })
                            .await;
                    }
                    TurnHighWayEvent::TurnPrepareResponse { .. } => {}
                }
            }
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EnvStateEvent> {
        self.env_state_tx.subscribe()
    }

    pub fn send(&self, event: EnvStateEvent) -> anyhow::Result<usize> {
        self.env_state_tx.send(event).map_err(|e| e.into())
    }
}
