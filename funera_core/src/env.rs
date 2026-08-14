use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock as StdRwLock};

use async_openai::config::OpenAIConfig;

#[cfg(feature = "skill")]
use crate::re_act::skills::{Skill, SkillRegistry};
#[cfg(feature = "tool")]
use crate::re_act::tool::{Tool, ToolRegistry};
#[cfg(feature = "sandbox")]
use crate::security::sandbox::SandboxPolicy;
use serde_json::Value as JsonValue;
use tokio::sync::{
    RwLock,
    watch::{self, error::RecvError},
};

/// A teardown action that undoes one effect.
///
/// Returned by [`FuneraEnv::effect`] bodies and run in reverse registration
/// order when [`FuneraEnv::dispose`] is called (LIFO recovery).
pub type Disposer = Box<dyn FnOnce() + Send + 'static>;

/// A callback fired when a service is provided (`true`) or removed (`false`).
///
/// Subscribed via [`FuneraEnv::on_service_change`]; `provide` and the disposers
/// registered by `provide` invoke it on the changed key. This is the reactive
/// notification primitive that drives dependent-plugin refresh.
pub type ServiceObserver = Arc<dyn Fn(TypeId, bool) + Send + Sync>;

/// Shared state behind a [`FuneraEnv`]'s capability layer.
///
/// `services` is the coeffect context (a typed service table keyed by
/// [`TypeId`]), shared between a derived env and its children. Each [`FuneraEnv`]
/// keeps its own effect accumulator (`disposers`) so a child can be torn down
/// independently of its parent.
struct EnvShared {
    services: StdRwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    observers: Mutex<Vec<ServiceObserver>>,
}

impl EnvShared {
    fn new() -> Self {
        Self {
            services: StdRwLock::new(HashMap::new()),
            observers: Mutex::new(Vec::new()),
        }
    }

    fn insert(&self, key: TypeId, value: Arc<dyn Any + Send + Sync>) {
        self.services.write().unwrap().insert(key, value);
        self.notify(key, true);
    }

    fn remove(&self, key: TypeId) {
        self.services.write().unwrap().remove(&key);
        self.notify(key, false);
    }

    fn notify(&self, key: TypeId, present: bool) {
        let observers = self.observers.lock().unwrap();
        for observer in observers.iter() {
            observer(key, present);
        }
    }

    fn subscribe(&self, observer: ServiceObserver) {
        self.observers.lock().unwrap().push(observer);
    }
}

#[derive(Clone)]
pub struct FuneraEnv {
    #[cfg(feature = "tool")]
    pub(crate) tool_registry: Arc<RwLock<ToolRegistry>>,
    #[cfg(feature = "skill")]
    pub(crate) skill_registry: Arc<RwLock<SkillRegistry>>,
    llm_client: async_openai::Client<OpenAIConfig>,
    model: String,
    #[cfg(feature = "tool")]
    tool_tx: watch::Sender<JsonValue>,
    client_tx: watch::Sender<async_openai::Client<OpenAIConfig>>,
    model_tx: watch::Sender<String>,
    #[cfg(feature = "skill")]
    skill_tx: watch::Sender<String>,
    #[cfg(feature = "sandbox")]
    sandbox_policy: SandboxPolicy,
    /// Shared service table (coeffect context) and observers.
    shared: Arc<EnvShared>,
    /// This env's own effect accumulator (LIFO inverse stack).
    disposers: Arc<Mutex<Vec<Disposer>>>,
}

impl FuneraEnv {
    pub fn new(
        llm_client: async_openai::Client<OpenAIConfig>,
        model: impl Into<String>,
    ) -> (Self, FuneraEnvWatcher) {
        let model = model.into();
        let (client_tx, client_rx) = watch::channel(llm_client.clone());
        let (model_tx, model_rx) = watch::channel(model.clone());

        #[cfg(feature = "tool")]
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        #[cfg(feature = "tool")]
        let (tool_tx, tool_rx) = watch::channel(JsonValue::Array(Vec::new()));

        #[cfg(feature = "skill")]
        let skill_registry = Arc::new(RwLock::new(SkillRegistry::new()));
        #[cfg(feature = "skill")]
        let (skill_tx, skill_rx) = watch::channel(String::new());

        (
            Self {
                #[cfg(feature = "tool")]
                tool_registry,
                #[cfg(feature = "skill")]
                skill_registry,
                llm_client,
                model,
                #[cfg(feature = "tool")]
                tool_tx,
                client_tx,
                model_tx,
                #[cfg(feature = "skill")]
                skill_tx,
                #[cfg(feature = "sandbox")]
                sandbox_policy: SandboxPolicy::default(),
                shared: Arc::new(EnvShared::new()),
                disposers: Arc::new(Mutex::new(Vec::new())),
            },
            FuneraEnvWatcher {
                #[cfg(feature = "tool")]
                tool_rx,
                client_rx,
                model_rx,
                #[cfg(feature = "skill")]
                skill_rx,
            },
        )
    }

    /// Set a custom sandbox policy.
    #[cfg(feature = "sandbox")]
    pub fn with_sandbox_policy(mut self, policy: SandboxPolicy) -> Self {
        self.sandbox_policy = policy;
        self
    }

    #[cfg(feature = "tool")]
    pub fn with_tool_registry(self, tool_registry: ToolRegistry) -> Self {
        let snapshot = tool_registry.available_tools_json();
        let _ = self.tool_tx.send(snapshot);
        Self {
            tool_registry: Arc::new(RwLock::new(tool_registry)),
            ..self
        }
    }

    #[cfg(feature = "skill")]
    pub fn with_skill_registry(self, skill_registry: SkillRegistry) -> Self {
        let prompt = skill_registry.get_active_skills_prompt();
        let _ = self.skill_tx.send(prompt);
        Self {
            skill_registry: Arc::new(RwLock::new(skill_registry)),
            ..self
        }
    }

    #[cfg(feature = "tool")]
    pub(crate) async fn add_tool(&mut self, tool: Box<dyn Tool>) {
        let mut registry = self.tool_registry.write().await;
        registry.add_tool(tool);
        let _ = self.tool_tx.send(registry.available_tools_json());
    }

    #[cfg(feature = "tool")]
    pub(crate) async fn remove_tool(&mut self, name: &str) {
        let mut registry = self.tool_registry.write().await;
        registry.remove_tool(name);
        let _ = self.tool_tx.send(registry.available_tools_json());
    }

    #[cfg(feature = "tool")]
    pub(crate) async fn set_tool_availability(&mut self, _name: &str, _available: bool) {
        let registry = self.tool_registry.read().await;
        let _ = self.tool_tx.send(registry.available_tools_json());
    }

    pub(crate) fn set_client(&mut self, client: async_openai::Client<OpenAIConfig>) {
        self.llm_client = client.clone();
        let _ = self.client_tx.send(client);
    }

    pub(crate) fn set_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.model = model.clone();
        let _ = self.model_tx.send(model);
    }

    #[cfg(feature = "skill")]
    pub(crate) async fn add_skill(&mut self, skill: Skill) {
        let mut registry = self.skill_registry.write().await;
        registry.add(skill);
        let _ = self.skill_tx.send(registry.get_active_skills_prompt());
    }

    #[cfg(feature = "skill")]
    pub(crate) async fn remove_skill(&mut self, name: &str) {
        let mut registry = self.skill_registry.write().await;
        registry.remove(name);
        let _ = self.skill_tx.send(registry.get_active_skills_prompt());
    }

    #[cfg(feature = "skill")]
    pub(crate) async fn activate_skill(&mut self, name: &str) -> bool {
        let mut registry = self.skill_registry.write().await;
        let ok = registry.activate(name);
        if ok {
            let _ = self.skill_tx.send(registry.get_active_skills_prompt());
        }
        ok
    }

    #[cfg(feature = "skill")]
    pub(crate) async fn deactivate_skill(&mut self, name: &str) -> bool {
        let mut registry = self.skill_registry.write().await;
        let ok = registry.deactivate(name);
        if ok {
            let _ = self.skill_tx.send(registry.get_active_skills_prompt());
        }
        ok
    }

    #[cfg(feature = "skill")]
    pub(crate) fn skill_prompt_now(&self) -> String {
        self.skill_tx.borrow().clone()
    }

    #[cfg(feature = "skill")]
    pub(crate) fn set_skill_prompt(&mut self, prompt: String) {
        let _ = self.skill_tx.send(prompt);
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Clone)]
pub struct FuneraEnvWatcher {
    #[cfg(feature = "tool")]
    tool_rx: watch::Receiver<JsonValue>,
    client_rx: watch::Receiver<async_openai::Client<OpenAIConfig>>,
    model_rx: watch::Receiver<String>,
    #[cfg(feature = "skill")]
    skill_rx: watch::Receiver<String>,
}

impl FuneraEnvWatcher {
    #[cfg(feature = "tool")]
    pub fn watch_tool(&mut self) -> JsonValue {
        self.tool_rx.borrow_and_update().clone()
    }

    pub fn watch_client(&mut self) -> async_openai::Client<OpenAIConfig> {
        self.client_rx.borrow_and_update().clone()
    }

    pub fn watch_model(&mut self) -> String {
        self.model_rx.borrow_and_update().clone()
    }

    #[cfg(feature = "skill")]
    pub fn watch_skill(&mut self) -> String {
        self.skill_rx.borrow_and_update().clone()
    }

    #[cfg(feature = "tool")]
    pub fn has_tool_changed(&self) -> bool {
        self.tool_rx.has_changed().unwrap_or(false)
    }

    pub fn has_client_changed(&self) -> bool {
        self.client_rx.has_changed().unwrap_or(false)
    }

    pub fn has_model_changed(&self) -> bool {
        self.model_rx.has_changed().unwrap_or(false)
    }

    #[cfg(feature = "skill")]
    pub fn has_skill_changed(&self) -> bool {
        self.skill_rx.has_changed().unwrap_or(false)
    }

    pub fn use_client(&mut self) -> async_openai::Client<OpenAIConfig> {
        self.watch_client()
    }

    #[cfg(feature = "tool")]
    pub async fn tool_changed(&mut self) -> Result<(), RecvError> {
        self.tool_rx.changed().await
    }

    pub async fn client_changed(&mut self) -> Result<(), RecvError> {
        self.client_rx.changed().await
    }

    pub async fn model_changed(&mut self) -> Result<(), RecvError> {
        self.model_rx.changed().await
    }

    #[cfg(feature = "skill")]
    pub async fn skill_changed(&mut self) -> Result<(), RecvError> {
        self.skill_rx.changed().await
    }
}

// ═══════════════════════════════════════════════════════════════
// Capability layer — reversible effects + typed services
// ═══════════════════════════════════════════════════════════════

impl FuneraEnv {
    /// Derive a child env that shares this env's service table but owns a fresh
    /// effect accumulator.
    ///
    /// A plugin instance runs its `apply` against a derived env, so unloading
    /// that instance ([`dispose`](Self::dispose)) reverts only the effects it
    /// registered, while the services it provided remain visible to siblings.
    pub fn derive(&self) -> FuneraEnv {
        let mut child = self.clone();
        child.disposers = Arc::new(Mutex::new(Vec::new()));
        child
    }

    /// Subscribe to service provision/removal events.
    ///
    /// The observer is called with `(TypeId, true)` when a service is provided
    /// and `(TypeId, false)` when it is removed.
    pub fn on_service_change(&self, observer: ServiceObserver) {
        self.shared.subscribe(observer);
    }

    /// Register a reversible effect.
    ///
    /// `body` runs now (setup) and returns a [`Disposer`] that undoes it. The
    /// disposer is pushed onto the env's accumulator and run in reverse (LIFO)
    /// order by [`dispose`](Self::dispose) when the env is torn down.
    ///
    /// ```rust,ignore
    /// env.effect(|| {
    ///     let resource = acquire();       // setup: the effect
    ///     Box::new(move || release(resource))  // teardown: its inverse
    /// });
    /// ```
    pub fn effect(&self, body: impl FnOnce() -> Disposer) {
        let undo = body();
        self.disposers.lock().unwrap().push(undo);
    }

    /// Provide a typed service to this env.
    ///
    /// The service is stored under its [`TypeId`] and automatically removed on
    /// [`dispose`](Self::dispose). `provide` is itself an effect: its inverse is
    /// "remove the service", registered in this env's own accumulator.
    ///
    /// ```rust,ignore
    /// #[derive(Clone)]
    /// struct Config { url: String }
    /// env.provide(Config { url: "db://…".into() });
    /// let cfg: std::sync::Arc<Config> = env.get::<Config>().unwrap();
    /// ```
    pub fn provide<T: Send + Sync + 'static>(&self, value: T) {
        let key = TypeId::of::<T>();
        let shared = Arc::clone(&self.shared);
        self.effect(move || {
            shared.insert(key, Arc::new(value));
            let teardown_shared = Arc::clone(&shared);
            Box::new(move || {
                teardown_shared.remove(key);
            })
        });
    }

    /// Resolve a typed service previously provided to this env (or a parent).
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.shared
            .services
            .read()
            .unwrap()
            .get(&TypeId::of::<T>())
            .and_then(|v| v.clone().downcast::<T>().ok())
    }

    /// Whether a service of type `T` is currently provided.
    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.contains_typeid(TypeId::of::<T>())
    }

    /// Whether a service with the given [`TypeId`] is currently provided.
    ///
    /// The non-generic counterpart of [`contains`](Self::contains), used by the
    /// plugin registry to check `inject` requirements (which are [`TypeId`]s).
    pub fn contains_typeid(&self, key: TypeId) -> bool {
        self.shared.services.read().unwrap().contains_key(&key)
    }

    /// Number of services currently provided.
    pub fn service_count(&self) -> usize {
        self.shared.services.read().unwrap().len()
    }

    /// Run every registered disposer in reverse (LIFO) order.
    ///
    /// This tears the env's capability layer down: each effect is undone in the
    /// reverse of the order it was registered. Only this env's own effects are
    /// reverted; services provided by sibling envs are untouched.
    pub fn dispose(&self) {
        let disposers = std::mem::take(&mut *self.disposers.lock().unwrap());
        for undo in disposers.into_iter().rev() {
            undo();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_env() -> (FuneraEnv, FuneraEnvWatcher) {
        FuneraEnv::new(async_openai::Client::new(), "test-model")
    }

    #[test]
    fn provide_get_roundtrip() {
        let (env, _watcher) = test_env();
        #[derive(Debug, PartialEq)]
        struct Config(u32);

        env.provide(Config(42));

        assert!(env.contains::<Config>());
        assert_eq!(env.service_count(), 1);
        assert_eq!(env.get::<Config>().unwrap().0, 42);
    }

    #[test]
    fn get_missing_returns_none() {
        let (env, _watcher) = test_env();
        assert!(env.get::<String>().is_none());
    }

    #[test]
    fn provide_removed_on_dispose() {
        let (env, _watcher) = test_env();
        env.provide(String::from("hello"));
        assert!(env.contains::<String>());

        env.dispose();
        assert!(!env.contains::<String>());
        assert_eq!(env.service_count(), 0);
    }

    #[test]
    fn effect_runs_in_lifo_order() {
        let (env, _watcher) = test_env();
        let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

        let l1 = Arc::clone(&log);
        env.effect(|| Box::new(move || l1.lock().unwrap().push("first")));
        let l2 = Arc::clone(&log);
        env.effect(|| Box::new(move || l2.lock().unwrap().push("second")));

        env.dispose();
        assert_eq!(*log.lock().unwrap(), vec!["second", "first"]);
    }

    #[test]
    fn dispose_idempotent() {
        let (env, _watcher) = test_env();
        env.effect(|| Box::new(|| {}));
        env.dispose();
        // Second dispose is a no-op: the accumulator was drained.
        env.dispose();
        assert_eq!(env.service_count(), 0);
    }
}
