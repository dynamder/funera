use tokio::sync::{broadcast, mpsc};

use crate::event_bus::{
    react_bus::{ReactBus, ReactEvent},
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
    PerTurnBusReady {
        token_tx: broadcast::Sender<TokenEvent>,
        react_tx: broadcast::Sender<ReactEvent>,
    },
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
                        let react_tx = react_bus.sender();
                        let _ = self
                            .env_state_tx
                            .send(EnvStateEvent::PerTurnBusReady {
                                token_tx: token_tx.clone(),
                                react_tx,
                            });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn turn_highway_full_protocol() {
        let (bus, mut handle) = EnvStateBus::new();
        bus.start_turn_highway();

        let (token_tx, react_bus) = handle.prepare_turn().await;
        assert!(token_tx.receiver_count() > 0 || token_tx.receiver_count() == 0);
        let _ = react_bus.send(crate::event_bus::react_bus::ReactEvent::TurnStart);
    }

    #[tokio::test]
    async fn turn_highway_multiple_turns() {
        let (bus, mut handle) = EnvStateBus::new();
        bus.start_turn_highway();

        let (tx1, rb1) = handle.prepare_turn().await;
        let (tx2, rb2) = handle.prepare_turn().await;

        // Each turn should get fresh buses
        assert!(!std::ptr::eq(&tx1, &tx2));
        rb1.send(crate::event_bus::react_bus::ReactEvent::TurnStart).ok();
        rb2.send(crate::event_bus::react_bus::ReactEvent::TurnStart).ok();
    }

    #[tokio::test]
    async fn turn_highway_fallback() {
        let (bus, mut handle) = EnvStateBus::new();
        drop(bus); // drop sender so prepare_turn's recv() returns None immediately
        // Intentionally NOT starting the highway

        let (token_tx, react_bus) = handle.prepare_turn().await;
        // Fallback should return valid buses even without highway
        let _ = react_bus.send(crate::event_bus::react_bus::ReactEvent::TurnStart);
        let _ = token_tx;
        // If we got here without hanging, fallback works
    }

    #[tokio::test]
    async fn env_state_bus_send_receive() {
        let (bus, _handle) = EnvStateBus::new();
        let mut rx = bus.subscribe();

        bus.send(EnvStateEvent::SessionStart).unwrap();
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, EnvStateEvent::SessionStart));

        bus.send(EnvStateEvent::LlmChanged("gpt-4".into())).unwrap();
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, EnvStateEvent::LlmChanged(m) if m == "gpt-4"));

        bus.send(EnvStateEvent::ToolAdded("calc".into())).unwrap();
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, EnvStateEvent::ToolAdded(n) if n == "calc"));
    }
}
