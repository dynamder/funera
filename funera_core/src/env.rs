use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use async_openai::config::OpenAIConfig;
use parking_lot::{Mutex, RwLock as StdRwLock};

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

pub mod key;

pub use key::{Generation, ProviderId, ServiceBinding, ServiceKey};

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
/// `services` is the coeffect context: a typed service table keyed by
/// [`ServiceKey`] (the primary key for `T` is its [`TypeId`]). The table is
/// shared between a derived env and its children. Each [`FuneraEnv`] keeps its
/// own effect accumulator (`disposers`) so a child can be torn down
/// independently of its parent.
struct EnvShared {
    services: StdRwLock<HashMap<ServiceKey, Arc<dyn Any + Send + Sync>>>,
    observers: Mutex<Vec<ServiceObserver>>,
}

impl EnvShared {
    fn new() -> Self {
        Self {
            services: StdRwLock::new(HashMap::new()),
            observers: Mutex::new(Vec::new()),
        }
    }

    fn insert(&self, key: ServiceKey, value: Arc<dyn Any + Send + Sync>) {
        self.services.write().insert(key.clone(), value);
        self.notify(key.type_id, true);
    }

    fn remove(&self, key: &ServiceKey) {
        self.services.write().remove(key);
        self.notify(key.type_id, false);
    }

    fn notify(&self, key: TypeId, present: bool) {
        let observers = self.observers.lock();
        for observer in observers.iter() {
            observer(key, present);
        }
    }

    fn subscribe(&self, observer: ServiceObserver) {
        self.observers.lock().push(observer);
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

    /// The configured sandbox policy.
    #[cfg(feature = "sandbox")]
    pub fn sandbox_policy(&self) -> &SandboxPolicy {
        &self.sandbox_policy
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
        self.disposers.lock().push(undo);
    }

    /// Provide a typed service to this env.
    ///
    /// The service is stored under the primary [`ServiceKey`] for `T` (the
    /// unnamed key) and automatically removed on [`dispose`](Self::dispose).
    /// `provide` is itself an effect: its inverse is "remove the service",
    /// registered in this env's own accumulator.
    ///
    /// ```rust,ignore
    /// #[derive(Clone)]
    /// struct Config { url: String }
    /// env.provide(Config { url: "db://…".into() });
    /// let cfg: std::sync::Arc<Config> = env.get::<Config>().unwrap();
    /// ```
    pub fn provide<T: Send + Sync + 'static>(&self, value: T) {
        self.provide_keyed(ServiceKey::primary::<T>(), value);
    }

    /// Provide a named typed service.
    ///
    /// Named keys permit multiple services of the same Rust type to coexist.
    /// Use [`get_named`](Self::get_named) to resolve them.
    pub fn provide_named<T: Send + Sync + 'static>(&self, name: impl Into<Arc<str>>, value: T) {
        self.provide_keyed(ServiceKey::named::<T>(name), value);
    }

    fn provide_keyed<T: Send + Sync + 'static>(&self, key: ServiceKey, value: T) {
        let shared = Arc::clone(&self.shared);
        self.effect(move || {
            shared.insert(key.clone(), Arc::new(value));
            let teardown_shared = Arc::clone(&shared);
            Box::new(move || {
                teardown_shared.remove(&key);
            })
        });
    }

    /// Resolve a typed service previously provided to this env (or a parent).
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.get_keyed(&ServiceKey::primary::<T>())
    }

    /// Resolve a named typed service previously provided via
    /// [`provide_named`](Self::provide_named).
    pub fn get_named<T: Send + Sync + 'static>(&self, name: &str) -> Option<Arc<T>> {
        self.get_keyed(&ServiceKey::named::<T>(name.to_string()))
    }

    fn get_keyed<T: Send + Sync + 'static>(&self, key: &ServiceKey) -> Option<Arc<T>> {
        self.shared
            .services
            .read()
            .get(key)
            .and_then(|v| v.clone().downcast::<T>().ok())
    }

    /// Whether a service of type `T` is currently provided.
    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.contains_typeid(TypeId::of::<T>())
    }

    /// Whether a named service of type `T` is currently provided.
    pub fn contains_named<T: Send + Sync + 'static>(&self, name: &str) -> bool {
        self.shared
            .services
            .read()
            .contains_key(&ServiceKey::named::<T>(name.to_string()))
    }

    /// Whether a service with the given [`TypeId`] is currently provided.
    ///
    /// The non-generic counterpart of [`contains`](Self::contains), used by the
    /// plugin registry to check `inject` requirements (which are [`TypeId`]s).
    pub fn contains_typeid(&self, key: TypeId) -> bool {
        self.shared.services.read().contains_key(&ServiceKey {
            type_id: key,
            name: None,
        })
    }

    /// Number of services currently provided.
    pub fn service_count(&self) -> usize {
        self.shared.services.read().len()
    }

    /// Run every registered disposer in reverse (LIFO) order.
    ///
    /// This tears the env's capability layer down: each effect is undone in the
    /// reverse of the order it was registered. Only this env's own effects are
    /// reverted; services provided by sibling envs are untouched.
    ///
    /// The disposal is idempotent and panic-isolated: a panicking disposer is
    /// caught and logged, and the remaining disposers still run.
    pub fn dispose(&self) {
        let disposers = std::mem::take(&mut *self.disposers.lock());
        for undo in disposers.into_iter().rev() {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| undo()));
            if result.is_err() {
                tracing::warn!("a disposer panicked during FuneraEnv::dispose; continuing");
            }
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
    fn provide_named_get_roundtrip() {
        let (env, _watcher) = test_env();
        #[derive(Debug, PartialEq)]
        struct Config(u32);

        env.provide_named("a", Config(1));
        env.provide_named("b", Config(2));
        env.provide(Config(3));

        assert_eq!(env.service_count(), 3);
        assert_eq!(env.get_named::<Config>("a").unwrap().0, 1);
        assert_eq!(env.get_named::<Config>("b").unwrap().0, 2);
        assert_eq!(env.get::<Config>().unwrap().0, 3);
        assert!(env.contains_named::<Config>("a"));
        assert!(!env.contains_named::<Config>("missing"));
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
        env.effect(|| Box::new(move || l1.lock().push("first")));
        let l2 = Arc::clone(&log);
        env.effect(|| Box::new(move || l2.lock().push("second")));

        env.dispose();
        assert_eq!(*log.lock(), vec!["second", "first"]);
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

    // ── model / client hot-reload (pre-existing methods) ──────

    #[test]
    fn set_model_updates_model_and_watcher() {
        let (mut env, mut watcher) = test_env();
        assert_eq!(env.model(), "test-model");
        env.set_model("m2");
        assert_eq!(env.model(), "m2");
        assert_eq!(watcher.watch_model(), "m2");
    }

    #[test]
    fn has_model_changed_reflects_changes() {
        let (mut env, mut watcher) = test_env();
        assert!(!watcher.has_model_changed());
        env.set_model("m2");
        assert!(watcher.has_model_changed());
        let _ = watcher.watch_model();
        assert!(!watcher.has_model_changed());
    }

    #[test]
    fn set_client_marks_client_changed() {
        let (mut env, mut watcher) = test_env();
        assert!(!watcher.has_client_changed());
        env.set_client(async_openai::Client::new());
        assert!(watcher.has_client_changed());
        let _ = watcher.watch_client();
        assert!(!watcher.has_client_changed());
    }

    #[tokio::test]
    async fn model_changed_blocks_then_resolves() {
        let (env, mut watcher) = test_env();

        // Blocks while no change is pending (kills a "return Ok(())" mutation).
        let early = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            watcher.model_changed(),
        )
        .await;
        assert!(
            early.is_err(),
            "model_changed should block with no pending change"
        );

        // Resolves once a change is sent.
        let mut env2 = env.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            env2.set_model("m2");
        });
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), watcher.model_changed()).await;
        assert!(
            result.is_ok(),
            "model_changed should resolve after set_model"
        );
        assert_eq!(watcher.watch_model(), "m2");
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn client_changed_blocks_then_resolves() {
        let (env, mut watcher) = test_env();

        let early = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            watcher.client_changed(),
        )
        .await;
        assert!(
            early.is_err(),
            "client_changed should block with no pending change"
        );

        let mut env2 = env.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            env2.set_client(async_openai::Client::new());
        });
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), watcher.client_changed()).await;
        assert!(
            result.is_ok(),
            "client_changed should resolve after set_client"
        );
        handle.await.unwrap();
    }

    #[cfg(feature = "sandbox")]
    #[test]
    fn with_sandbox_policy_roundtrips() {
        let (env, _watcher) = test_env();
        let env = env.with_sandbox_policy(SandboxPolicy::disabled());
        assert!(!env.sandbox_policy().enabled);
    }

    #[cfg(feature = "tool")]
    mod tool_tests {
        use super::*;
        use crate::plugin::Plugin;
        use crate::re_act::tool::{Tool, ToolCallError, ToolRegistry};
        use serde_json::json;

        struct MockTool;
        impl Plugin for MockTool {
            fn name(&self) -> &str {
                "mock"
            }
        }
        #[async_trait::async_trait]
        impl Tool for MockTool {
            fn description(&self) -> &str {
                "mock tool"
            }
            fn schema(&self) -> JsonValue {
                json!({"type": "function", "function": {"name": "mock"}})
            }
            async fn execute(&self, _args: JsonValue) -> Result<String, ToolCallError> {
                Ok("ok".into())
            }
        }

        #[test]
        fn with_tool_registry_updates_watcher_and_registry() {
            let (env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            let mut reg = ToolRegistry::new();
            reg.add_tool(Box::new(MockTool));
            let env = env.with_tool_registry(reg);
            assert!(
                watcher
                    .watch_tool()
                    .as_array()
                    .is_some_and(|a| a.len() == 1)
            );
            assert!(env.tool_registry.blocking_read().tool_exists("mock"));
        }

        #[tokio::test]
        async fn add_then_remove_tool_updates_watcher() {
            let (mut env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            env.add_tool(Box::new(MockTool)).await;
            assert!(
                watcher
                    .watch_tool()
                    .as_array()
                    .is_some_and(|a| a.len() == 1)
            );
            env.remove_tool("mock").await;
            assert!(
                watcher
                    .watch_tool()
                    .as_array()
                    .is_some_and(|a| a.is_empty())
            );
        }

        #[tokio::test]
        async fn set_tool_availability_rebroadcasts_snapshot() {
            let (mut env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            env.add_tool(Box::new(MockTool)).await;
            let _ = watcher.watch_tool();
            assert!(!watcher.has_tool_changed());
            env.set_tool_availability("mock", false).await;
            assert!(watcher.has_tool_changed());
        }

        #[test]
        fn has_tool_changed_reflects_changes() {
            let (env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            assert!(!watcher.has_tool_changed());
            let mut reg = ToolRegistry::new();
            reg.add_tool(Box::new(MockTool));
            let _env = env.with_tool_registry(reg);
            assert!(watcher.has_tool_changed());
            let _ = watcher.watch_tool();
            assert!(!watcher.has_tool_changed());
        }

        #[tokio::test]
        async fn tool_changed_blocks_then_resolves() {
            let (env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");

            let early =
                tokio::time::timeout(std::time::Duration::from_millis(50), watcher.tool_changed())
                    .await;
            assert!(
                early.is_err(),
                "tool_changed should block with no pending change"
            );

            let mut env2 = env.clone();
            let handle = tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                env2.add_tool(Box::new(MockTool)).await;
            });
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(5), watcher.tool_changed())
                    .await;
            assert!(result.is_ok(), "tool_changed should resolve after add_tool");
            handle.await.unwrap();
        }
    }

    #[cfg(feature = "skill")]
    mod skill_tests {
        use super::*;
        use crate::re_act::skills::{Skill, SkillRegistry};

        fn skill(name: &str, content: &str) -> Skill {
            Skill::new(name, "", content)
        }

        #[tokio::test]
        async fn add_and_activate_skill_updates_prompt() {
            let (mut env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            env.add_skill(skill("s1", "part one")).await;
            assert!(env.activate_skill("s1").await);
            assert_eq!(watcher.watch_skill(), "part one");
            assert_eq!(env.skill_prompt_now(), "part one");
        }

        #[tokio::test]
        async fn activate_deactivate_skill_returns_bool() {
            let (mut env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            env.add_skill(skill("s1", "content")).await;
            assert!(!env.activate_skill("ghost").await);
            assert!(env.activate_skill("s1").await);
            assert_eq!(watcher.watch_skill(), "content");
            assert!(env.deactivate_skill("s1").await);
            assert_eq!(watcher.watch_skill(), "");
            assert!(!env.deactivate_skill("s1").await);
        }

        #[tokio::test]
        async fn remove_skill_clears_prompt() {
            let (mut env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            env.add_skill(skill("s1", "content")).await;
            env.activate_skill("s1").await;
            env.remove_skill("s1").await;
            assert_eq!(watcher.watch_skill(), "");
        }

        #[test]
        fn with_skill_registry_updates_watcher_and_registry() {
            let (env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            let mut reg = SkillRegistry::new();
            reg.add(skill("s1", "content"));
            reg.activate("s1");
            let env = env.with_skill_registry(reg);
            assert_eq!(watcher.watch_skill(), "content");
            assert!(env.skill_registry.blocking_read().contains("s1"));
        }

        #[test]
        fn set_skill_prompt_and_has_changed() {
            let (mut env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            assert!(!watcher.has_skill_changed());
            env.set_skill_prompt("hello".into());
            assert!(watcher.has_skill_changed());
            assert_eq!(watcher.watch_skill(), "hello");
        }

        #[tokio::test]
        async fn skill_changed_blocks_then_resolves() {
            let (env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");

            let early = tokio::time::timeout(
                std::time::Duration::from_millis(50),
                watcher.skill_changed(),
            )
            .await;
            assert!(
                early.is_err(),
                "skill_changed should block with no pending change"
            );

            let mut env2 = env.clone();
            let handle = tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                env2.add_skill(skill("s1", "content")).await;
            });
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(5), watcher.skill_changed())
                    .await;
            assert!(
                result.is_ok(),
                "skill_changed should resolve after add_skill"
            );
            handle.await.unwrap();
        }
    }
}
