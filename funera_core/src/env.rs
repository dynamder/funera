use std::sync::Arc;

use async_openai::config::OpenAIConfig;
use parking_lot::Mutex;

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
    /// Accumulator of registered reversible effects, drained in reverse (LIFO)
    /// order by [`FuneraEnv::dispose`].
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

    /// The currently configured sandbox policy.
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
    pub(crate) async fn add_tool(&mut self, tool: Arc<dyn Tool>) {
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

    /// Remove the tool only if the registered entry is the same `Arc` value.
    ///
    /// This is the safe inverse of [`add_tool`](Self::add_tool) for a
    /// [`Disposer`]: a stale teardown cannot delete a replacement tool that
    /// reuses the same name.
    ///
    /// Exercised by the reversible-effects tests; kept on the env so callers
    /// inside the crate can pair registrations with their exact inverse.
    #[cfg(feature = "tool")]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn remove_tool_if_same(&mut self, name: &str, tool: &Arc<dyn Tool>) -> bool {
        let mut registry = self.tool_registry.write().await;
        let removed = registry.remove_tool_if_same(name, tool);
        if removed {
            let _ = self.tool_tx.send(registry.available_tools_json());
        }
        removed
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

    // ── Reversible effects (LIFO teardown) ─────────────────────

    /// Register a reversible effect.
    ///
    /// `body` runs now (setup) and returns a [`Disposer`] that undoes it. The
    /// disposer is pushed onto the env's accumulator and run in reverse (LIFO)
    /// order by [`dispose`](Self::dispose) when the env is torn down, so later
    /// registrations — which may depend on earlier ones — are undone first.
    ///
    /// ```rust,ignore
    /// env.effect(|| {
    ///     let resource = acquire();             // setup: the effect
    ///     Box::new(move || release(resource))   // teardown: its inverse
    /// });
    /// ```
    pub fn effect(&self, body: impl FnOnce() -> Disposer) {
        let undo = body();
        self.disposers.lock().push(undo);
    }

    /// Run every registered disposer in reverse (LIFO) order.
    ///
    /// This undoes each effect in the reverse of the order it was registered,
    /// preventing memory / service leaks from registrations that were never
    /// explicitly reverted. Disposal is idempotent (the accumulator is drained)
    /// and panic-isolated: a panicking disposer is caught and logged while the
    /// remaining disposers still run.
    pub fn dispose(&self) {
        let disposers = std::mem::take(&mut *self.disposers.lock());
        for undo in disposers.into_iter().rev() {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(undo));
            if result.is_err() {
                tracing::warn!("a disposer panicked during FuneraEnv::dispose; continuing");
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_env() -> (FuneraEnv, FuneraEnvWatcher) {
        FuneraEnv::new(async_openai::Client::new(), "test-model")
    }

    // ── reversible effects (LIFO teardown) ─────────────────────

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
        let ran = Arc::new(AtomicUsize::new(0));
        let r = Arc::clone(&ran);
        env.effect(|| {
            Box::new(move || {
                r.fetch_add(1, Ordering::Relaxed);
            })
        });
        env.dispose();
        // Second dispose is a no-op: the accumulator was drained.
        env.dispose();
        assert_eq!(ran.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dispose_isolates_panicking_disposer() {
        let (env, _watcher) = test_env();
        let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

        let l1 = Arc::clone(&log);
        env.effect(|| Box::new(move || l1.lock().push("first")));
        env.effect(|| Box::new(move || panic!("boom")));
        let l3 = Arc::clone(&log);
        env.effect(|| Box::new(move || l3.lock().push("third")));

        // Must not propagate the panic; remaining disposers still run (LIFO).
        env.dispose();
        assert_eq!(*log.lock(), vec!["third", "first"]);
    }

    // ── model / client hot-reload ──────────────────────────────

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

        // Blocks while no change is pending.
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
        use crate::re_act::tool::{Tool, ToolCallError, ToolRegistry};
        use serde_json::json;

        struct MockTool;
        #[async_trait::async_trait]
        impl Tool for MockTool {
            fn name(&self) -> &str {
                "mock"
            }
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
            reg.add_tool(Arc::new(MockTool));
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
            env.add_tool(Arc::new(MockTool)).await;
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
            env.add_tool(Arc::new(MockTool)).await;
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
            reg.add_tool(Arc::new(MockTool));
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
                env2.add_tool(Arc::new(MockTool)).await;
            });
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(5), watcher.tool_changed())
                    .await;
            assert!(result.is_ok(), "tool_changed should resolve after add_tool");
            handle.await.unwrap();
        }

        #[tokio::test]
        async fn env_remove_tool_if_same_removes_only_matching_arc() {
            let (mut env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            let original: Arc<dyn Tool> = Arc::new(MockTool);
            env.add_tool(Arc::clone(&original)).await;
            // A replacement reuses the same name: the registered Arc is now the
            // second tool.
            env.add_tool(Arc::new(MockTool)).await;

            // A different Arc must not remove anything.
            let other: Arc<dyn Tool> = Arc::new(MockTool);
            assert!(!env.remove_tool_if_same("mock", &other).await);
            assert!(env.tool_registry.read().await.tool_exists("mock"));

            // The stale (original) Arc must not remove the replacement either.
            assert!(!env.remove_tool_if_same("mock", &original).await);
            assert!(env.tool_registry.read().await.tool_exists("mock"));

            // Removing the currently registered Arc succeeds and notifies.
            let current = env
                .tool_registry
                .read()
                .await
                .get_tool("mock")
                .unwrap()
                .tool
                .clone();
            assert!(env.remove_tool_if_same("mock", &current).await);
            assert!(!env.tool_registry.read().await.tool_exists("mock"));
            assert!(
                watcher
                    .watch_tool()
                    .as_array()
                    .is_some_and(|a| a.is_empty())
            );
        }

        #[tokio::test]
        async fn disposer_can_revert_tool_registration() {
            let (mut env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            let tool: Arc<dyn Tool> = Arc::new(MockTool);
            env.add_tool(Arc::clone(&tool)).await;
            assert!(
                watcher
                    .watch_tool()
                    .as_array()
                    .is_some_and(|a| a.len() == 1)
            );

            // Register the inverse of the registration as a disposer, so
            // dispose() removes exactly this tool — the leak-safe pattern.
            let registry = env.tool_registry.clone();
            env.effect(move || {
                let registry = Arc::clone(&registry);
                let tool = Arc::clone(&tool);
                Box::new(move || {
                    // Best-effort, non-blocking undo over the async registry.
                    if let Ok(mut guard) = registry.try_write() {
                        guard.remove_tool_if_same("mock", &tool);
                    }
                })
            });

            env.dispose();
            assert!(!env.tool_registry.read().await.tool_exists("mock"));
        }

        #[tokio::test]
        async fn stale_disposer_does_not_remove_replacement_tool() {
            let (mut env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "m");
            let original: Arc<dyn Tool> = Arc::new(MockTool);
            env.add_tool(Arc::clone(&original)).await;

            let registry = env.tool_registry.clone();
            env.effect(move || {
                let registry = Arc::clone(&registry);
                let original = Arc::clone(&original);
                Box::new(move || {
                    if let Ok(mut guard) = registry.try_write() {
                        guard.remove_tool_if_same("mock", &original);
                    }
                })
            });

            // A replacement tool reuses the same name before disposal.
            env.add_tool(Arc::new(MockTool)).await;

            // The stale disposer must NOT remove the replacement.
            env.dispose();
            assert!(env.tool_registry.read().await.tool_exists("mock"));
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
