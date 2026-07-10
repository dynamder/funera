use std::sync::Arc;

use tokio::sync::broadcast;

use funera_core::event_bus::env_state_bus::EnvStateEvent;

use crate::event::{AgentEvent, RawAgentEvent};

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

    pub fn len(&self) -> usize {
        self.callbacks.len()
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

/// Minimal dispatcher: listens to env-state events, forwards to raw channel, emits Done on close.
///
/// Data events (Text, ToolCall, etc.) are now emitted by `react_loop` via the event sender
/// and sent directly to `event_tx` + callbacks. This dispatcher only handles session lifecycle.
pub struct CallbackDispatcher {
    _event_tx: broadcast::Sender<AgentEvent>,
    _handles: Vec<tokio::task::JoinHandle<()>>,
}

impl CallbackDispatcher {
    pub fn new(
        env_state_rx: broadcast::Receiver<EnvStateEvent>,
        event_tx: broadcast::Sender<AgentEvent>,
        raw_event_tx: broadcast::Sender<RawAgentEvent>,
    ) -> Self {
        let tx = event_tx.clone();
        let raw_tx = raw_event_tx.clone();
        let handle = tokio::spawn(async move {
            Self::listen_env_state(env_state_rx, tx, raw_tx).await;
        });

        Self {
            _event_tx: event_tx,
            _handles: vec![handle],
        }
    }

    async fn listen_env_state(
        mut rx: broadcast::Receiver<EnvStateEvent>,
        event_tx: broadcast::Sender<AgentEvent>,
        raw_event_tx: broadcast::Sender<RawAgentEvent>,
    ) {
        loop {
            match rx.recv().await {
                Ok(EnvStateEvent::PerTurnBusReady {
                    token_tx,
                    react_tx,
                }) => {
                    let _ = raw_event_tx.send(RawAgentEvent::EnvState(
                        EnvStateEvent::PerTurnBusReady {
                            token_tx,
                            react_tx,
                        },
                    ));
                }
                Ok(EnvStateEvent::SessionClosed) => {
                    let _ =
                        raw_event_tx.send(RawAgentEvent::EnvState(EnvStateEvent::SessionClosed));
                    let _ = event_tx.send(AgentEvent::Done);
                    break;
                }
                Ok(other) => {
                    let _ = raw_event_tx.send(RawAgentEvent::EnvState(other));
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    }

    pub fn event_tx(&self) -> broadcast::Sender<AgentEvent> {
        self._event_tx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{AgentEvent, RawAgentEvent};
    use funera_core::event_bus::env_state_bus::{EnvStateBus, EnvStateEvent};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_dispatcher(
        rx: broadcast::Receiver<EnvStateEvent>,
        event_tx: broadcast::Sender<AgentEvent>,
        raw_event_tx: broadcast::Sender<RawAgentEvent>,
    ) -> CallbackDispatcher {
        CallbackDispatcher::new(rx, event_tx, raw_event_tx)
    }

    // ── CallbackRegistry ───────────────────────────────────────────

    #[test]
    fn registry_new_is_empty() {
        let r = CallbackRegistry::new();
        assert!(r.is_empty());
    }

    #[test]
    fn registry_add_not_empty() {
        let mut r = CallbackRegistry::new();
        r.add(Arc::new(|_| {}));
        assert!(!r.is_empty());
    }

    #[test]
    fn registry_dispatch_calls_all() {
        let mut r = CallbackRegistry::new();
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        {
            let c = c1.clone();
            r.add(Arc::new(move |_| { c.fetch_add(1, Ordering::SeqCst); }));
        }
        {
            let c = c2.clone();
            r.add(Arc::new(move |_| { c.fetch_add(1, Ordering::SeqCst); }));
        }
        r.dispatch(AgentEvent::Done);
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn registry_dispatch_with_arg() {
        let captured = Arc::new(AtomicUsize::new(0));
        let mut r = CallbackRegistry::new();
        {
            let c = captured.clone();
            r.add(Arc::new(move |event| {
                if let AgentEvent::Text(t) = event {
                    assert_eq!(t, "hello");
                    c.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        r.dispatch(AgentEvent::Text("hello".into()));
        assert_eq!(captured.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn registry_clone_independent() {
        let mut r = CallbackRegistry::new();
        r.add(Arc::new(|_| {}));
        let r2 = r.clone();
        assert!(!r.is_empty());
        assert!(!r2.is_empty());
    }

    #[test]
    fn registry_combine_merges() {
        let mut r1 = CallbackRegistry::new();
        let mut r2 = CallbackRegistry::new();
        let c = Arc::new(AtomicUsize::new(0));
        {
            let cnt = c.clone();
            r1.add(Arc::new(move |_| { cnt.fetch_add(1, Ordering::SeqCst); }));
        }
        {
            let cnt = c.clone();
            r2.add(Arc::new(move |_| { cnt.fetch_add(1, Ordering::SeqCst); }));
        }
        {
            let cnt = c.clone();
            r2.add(Arc::new(move |_| { cnt.fetch_add(1, Ordering::SeqCst); }));
        }
        r1.combine(r2);
        r1.dispatch(AgentEvent::Done);
        assert_eq!(c.load(Ordering::SeqCst), 3);
    }

    // ── CallbackDispatcher ─────────────────────────────────────────

    #[tokio::test]
    async fn dispatcher_session_closed_stops_listener() {
        let (event_tx, mut event_rx) = broadcast::channel(32);
        let (raw_event_tx, mut raw_rx) = broadcast::channel(32);

        let bus = EnvStateBus::new();
        let rx = bus.0.subscribe();
        let _disp = make_dispatcher(rx, event_tx.clone(), raw_event_tx);

        let _ = bus.0.env_state_tx.send(EnvStateEvent::SessionClosed);

        let got = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv()).await;
        assert!(matches!(got, Ok(Ok(AgentEvent::Done))));

        let raw = tokio::time::timeout(std::time::Duration::from_secs(1), raw_rx.recv()).await;
        assert!(matches!(raw, Ok(Ok(RawAgentEvent::EnvState(EnvStateEvent::SessionClosed)))));
    }
}
