use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tokio::sync::broadcast;

use funera_core::env::FuneraEnv;
use funera_core::event_bus::env_state_bus::EnvStateEvent;
#[cfg(feature = "security")]
use funera_core::event_bus::react_bus::ReactEvent;
use funera_core::plugin::{Plugin, PluginConfig, PluginError};

use crate::event::{AgentEvent, RawAgentEvent};

pub type CallbackFn = Arc<dyn Fn(AgentEvent) + Send + Sync>;

static NEXT_CALLBACK_ID: AtomicU64 = AtomicU64::new(0);

/// A cloneable registry of named callbacks.
///
/// The interior lock lets a [`crate::CallbackPlugin`] add and remove callbacks
/// through the plugin lifecycle without taking `&mut self` on the registry.
#[derive(Clone)]
pub struct CallbackRegistry {
    callbacks: Arc<Mutex<Vec<(String, CallbackFn)>>>,
}

impl Default for CallbackRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CallbackRegistry {
    pub fn new() -> Self {
        Self {
            callbacks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add an anonymous callback with an auto-generated name.
    pub fn add(&self, f: CallbackFn) {
        let name = format!("auto:{}", NEXT_CALLBACK_ID.fetch_add(1, Ordering::Relaxed));
        self.add_named(name, f);
    }

    /// Add a callback under an explicit name.
    pub fn add_named(&self, name: impl Into<String>, f: CallbackFn) {
        self.callbacks.lock().push((name.into(), f));
    }

    /// Remove every callback registered under `name`.
    pub fn remove_named(&self, name: &str) {
        self.callbacks.lock().retain(|(n, _)| n != name);
    }

    pub fn is_empty(&self) -> bool {
        self.callbacks.lock().is_empty()
    }

    pub fn len(&self) -> usize {
        self.callbacks.lock().len()
    }

    pub fn dispatch(&self, event: AgentEvent) {
        let callbacks: Vec<CallbackFn> = self
            .callbacks
            .lock()
            .iter()
            .map(|(_, f)| Arc::clone(f))
            .collect();
        for f in callbacks {
            f(event.clone());
        }
    }

    pub fn combine(&self, other: &CallbackRegistry) {
        let other_callbacks = other.callbacks.lock().clone();
        self.callbacks.lock().extend(other_callbacks);
    }
}

/// Mounts a callback into an [`Agent`](crate::Agent)'s [`CallbackRegistry`].
///
/// The callback is installed when the plugin activates and removed when the
/// plugin is unmounted, so callbacks participate in the same lifecycle as
/// tools, skills, and middleware.
pub struct CallbackPlugin {
    pub name: String,
    pub registry: Arc<CallbackRegistry>,
    pub callback: CallbackFn,
}

impl CallbackPlugin {
    pub fn new(
        name: impl Into<String>,
        registry: Arc<CallbackRegistry>,
        callback: CallbackFn,
    ) -> Self {
        Self {
            name: name.into(),
            registry,
            callback,
        }
    }
}

#[async_trait::async_trait]
impl Plugin for CallbackPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn apply(
        &self,
        env: &FuneraEnv,
        _config: Option<&PluginConfig>,
    ) -> Result<(), PluginError> {
        self.registry
            .add_named(self.name.clone(), self.callback.clone());

        let registry = Arc::clone(&self.registry);
        let name = self.name.clone();
        env.effect(|| {
            Box::new(move || {
                registry.remove_named(&name);
            })
        });

        Ok(())
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
                Ok(EnvStateEvent::PerTurnBusReady { token_tx, react_tx }) => {
                    let _ = raw_event_tx.send(RawAgentEvent::EnvState(
                        EnvStateEvent::PerTurnBusReady {
                            token_tx,
                            react_tx: react_tx.clone(),
                        },
                    ));

                    #[cfg(all(feature = "tool", feature = "security"))]
                    {
                        let event_tx = event_tx.clone();
                        let mut react_rx = react_tx.subscribe();
                        tokio::spawn(async move {
                            while let Ok(event) = react_rx.recv().await {
                                if let ReactEvent::ToolApprovalRequired {
                                    call_id,
                                    tool_name,
                                    reason,
                                    ..
                                } = event
                                {
                                    let _ = event_tx.send(AgentEvent::ToolApprovalRequired {
                                        call_id: Arc::from(call_id.as_str()),
                                        tool_name,
                                        reason,
                                    });
                                }
                            }
                        });
                    }
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
        let r = CallbackRegistry::new();
        r.add(Arc::new(|_| {}));
        assert!(!r.is_empty());
    }

    #[test]
    fn registry_dispatch_calls_all() {
        let r = CallbackRegistry::new();
        let c1 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));
        {
            let c = c1.clone();
            r.add(Arc::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }
        {
            let c = c2.clone();
            r.add(Arc::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }
        r.dispatch(AgentEvent::Done);
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn registry_dispatch_with_arg() {
        let captured = Arc::new(AtomicUsize::new(0));
        let r = CallbackRegistry::new();
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
        let r = CallbackRegistry::new();
        r.add(Arc::new(|_| {}));
        let r2 = r.clone();
        assert!(!r.is_empty());
        assert!(!r2.is_empty());
    }

    #[test]
    fn registry_combine_merges() {
        let mut r1 = CallbackRegistry::new();
        let r2 = CallbackRegistry::new();
        let c = Arc::new(AtomicUsize::new(0));
        {
            let cnt = c.clone();
            r1.add(Arc::new(move |_| {
                cnt.fetch_add(1, Ordering::SeqCst);
            }));
        }
        {
            let cnt = c.clone();
            r2.add(Arc::new(move |_| {
                cnt.fetch_add(1, Ordering::SeqCst);
            }));
        }
        {
            let cnt = c.clone();
            r2.add(Arc::new(move |_| {
                cnt.fetch_add(1, Ordering::SeqCst);
            }));
        }
        r1.combine(&r2);
        r1.dispatch(AgentEvent::Done);
        assert_eq!(c.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn callback_plugin_adds_and_removes() {
        use funera_core::plugin::{PluginPhase, PluginRegistry};
        use std::sync::Arc;

        let registry = Arc::new(CallbackRegistry::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let callback_counter = Arc::clone(&counter);
        let plugin = CallbackPlugin::new(
            "count",
            Arc::clone(&registry),
            Arc::new(move |_| {
                callback_counter.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let (env, _watcher) =
            funera_core::env::FuneraEnv::new(async_openai::Client::new(), "test-model");
        let mut reg = PluginRegistry::new(env);
        let id = reg.mount(Arc::new(plugin));
        reg.refresh().await;

        assert_eq!(reg.phase(id), Some(PluginPhase::Active));
        registry.dispatch(AgentEvent::Done);
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        reg.unmount(id).await;
        registry.dispatch(AgentEvent::Done);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "callback should be removed"
        );
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
        assert!(matches!(
            raw,
            Ok(Ok(RawAgentEvent::EnvState(EnvStateEvent::SessionClosed)))
        ));
    }
}
