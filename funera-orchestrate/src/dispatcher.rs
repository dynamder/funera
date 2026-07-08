use std::sync::Arc;

use tokio::sync::broadcast;

use funera_core::event_bus::env_state_bus::EnvStateEvent;
use funera_core::event_bus::react_bus::ReactEvent;
use funera_core::event_bus::token_bus::TokenEvent;

use crate::event::AgentEvent;

type CallbackFn = Arc<dyn Fn(AgentEvent) + Send + Sync>;

#[derive(Default, Clone)]
pub struct CallbackRegistry {
    callbacks: Vec<CallbackFn>,
}

impl CallbackRegistry {
    pub fn new() -> Self {
        Self {
            callbacks: Vec::new(),
        }
    }

    pub fn add(&mut self, f: CallbackFn) {
        self.callbacks.push(f);
    }

    pub fn is_empty(&self) -> bool {
        self.callbacks.is_empty()
    }

    pub fn dispatch(&self, event: AgentEvent) {
        for f in &self.callbacks {
            f(event.clone());
        }
    }

    pub fn combine(&mut self, other: CallbackRegistry) {
        self.callbacks.extend(other.callbacks);
    }
}

pub struct CallbackDispatcher {
    _event_tx: broadcast::Sender<AgentEvent>,
    _handles: Vec<tokio::task::JoinHandle<()>>,
}

impl CallbackDispatcher {
    pub fn new(
        env_state_rx: broadcast::Receiver<EnvStateEvent>,
        registry: Arc<CallbackRegistry>,
        event_tx: broadcast::Sender<AgentEvent>,
    ) -> Self {
        let mut handles = Vec::new();

        let reg = registry.clone();
        let tx = event_tx.clone();
        let handle = tokio::spawn(async move {
            Self::listen_env_state(env_state_rx, reg, tx).await;
        });
        handles.push(handle);

        Self {
            _event_tx: event_tx,
            _handles: handles,
        }
    }

    async fn listen_env_state(
        mut rx: broadcast::Receiver<EnvStateEvent>,
        registry: Arc<CallbackRegistry>,
        event_tx: broadcast::Sender<AgentEvent>,
    ) {
        loop {
            match rx.recv().await {
                Ok(EnvStateEvent::PerTurnBusReady {
                    token_tx,
                    react_tx,
                }) => {
                    let token_rx = token_tx.subscribe();
                    let react_rx = react_tx.subscribe();

                    let reg = registry.clone();
                    let tx = event_tx.clone();
                    tokio::spawn(async move {
                        Self::listen_token_bus(token_rx, reg.clone(), tx.clone()).await;
                    });

                    let reg = registry.clone();
                    let tx = event_tx.clone();
                    tokio::spawn(async move {
                        Self::listen_react_bus(react_rx, reg.clone(), tx.clone()).await;
                    });
                }
                Ok(EnvStateEvent::SessionClosed) => {
                    let _ = event_tx.send(AgentEvent::Done);
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                _ => {}
            }
        }
    }

    async fn listen_token_bus(
        mut rx: broadcast::Receiver<TokenEvent>,
        registry: Arc<CallbackRegistry>,
        event_tx: broadcast::Sender<AgentEvent>,
    ) {
        loop {
            match rx.recv().await {
                Ok(TokenEvent::Text(t)) => {
                    let event = AgentEvent::Token(t);
                    registry.dispatch(event.clone());
                    let _ = event_tx.send(event);
                }
                Ok(TokenEvent::ToolDelta {
                    index,
                    call_id,
                    name,
                    args_chunk,
                }) => {
                    if let (Some(name), Some(args_str)) = (name, args_chunk) {
                        let args: serde_json::Value =
                            serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Null);
                        let call_id = uuid::Uuid::parse_str(
                            &call_id.unwrap_or_default(),
                        )
                        .unwrap_or_default();
                        let event = AgentEvent::ToolCallStart {
                            index,
                            call_id,
                            name,
                            args,
                        };
                        registry.dispatch(event.clone());
                        let _ = event_tx.send(event);
                    }
                }
                Ok(TokenEvent::Finish(_)) => {
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    }

    async fn listen_react_bus(
        mut rx: broadcast::Receiver<ReactEvent>,
        registry: Arc<CallbackRegistry>,
        event_tx: broadcast::Sender<AgentEvent>,
    ) {
        loop {
            match rx.recv().await {
                Ok(ReactEvent::TurnStart) => {
                    let event = AgentEvent::TurnStart;
                    registry.dispatch(event.clone());
                    let _ = event_tx.send(event);
                }
                Ok(ReactEvent::TurnEnd) => {
                    let event = AgentEvent::TurnEnd;
                    registry.dispatch(event.clone());
                    let _ = event_tx.send(event);
                }
                Ok(ReactEvent::ToolExecRequest(req)) => {
                    let call_id = uuid::Uuid::parse_str(&req.call_id).unwrap_or_default();
                    let event = AgentEvent::ToolCallStart {
                        index: req.index,
                        call_id,
                        name: req.name,
                        args: req.args,
                    };
                    registry.dispatch(event.clone());
                    let _ = event_tx.send(event);
                }
                Ok(ReactEvent::ToolExecResponse(res)) => {
                    let (call_id, name, result) = match res {
                        Ok(response) => {
                            let call_id =
                                uuid::Uuid::parse_str(&response.call_id).unwrap_or_default();
                            (call_id, String::new(), Ok(response.result))
                        }
                        Err(e) => {
                            (uuid::Uuid::nil(), String::new(), Err(e))
                        }
                    };
                    let event = AgentEvent::ToolCallResult {
                        call_id,
                        name,
                        result,
                    };
                    registry.dispatch(event.clone());
                    let _ = event_tx.send(event);
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                _ => {}
            }
        }
    }

    pub fn event_tx(&self) -> broadcast::Sender<AgentEvent> {
        self._event_tx.clone()
    }
}
