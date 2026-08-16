//! Adapter plugins for existing capability traits.

use std::sync::Arc;

use async_trait::async_trait;

use async_openai::config::OpenAIConfig;

use crate::env::FuneraEnv;
use crate::plugin::{Plugin, PluginConfig, PluginError};

#[cfg(feature = "tool")]
use crate::re_act::tool::Tool;

#[cfg(feature = "skill")]
use crate::re_act::skills::Skill;

use crate::middleware::{ErrorsEnabled, MiddlewareChain, MiddlewareLayer};

/// Spawns an async teardown action on the current tokio runtime.
///
/// Used by adapters whose underlying registry is async. If no runtime is
/// available the teardown is dropped (and `tracing::warn`ed).
fn spawn_teardown(task: impl std::future::Future<Output = ()> + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(task);
        }
        Err(_) => {
            tracing::warn!("cannot schedule async plugin teardown: no tokio runtime");
        }
    }
}

#[cfg(feature = "tool")]
#[async_trait]
impl Plugin for ToolPlugin {
    fn name(&self) -> &str {
        self.tool.name()
    }

    async fn apply(
        &self,
        env: &FuneraEnv,
        _config: Option<&PluginConfig>,
    ) -> Result<(), PluginError> {
        let tool_name = self.tool.name().to_string();
        env.add_tool(self.tool.clone()).await;

        let teardown_env = env.clone();
        env.effect(|| {
            Box::new(move || {
                let tool_name = tool_name.clone();
                spawn_teardown(async move {
                    let _ = teardown_env.remove_tool(&tool_name).await;
                });
            })
        });

        Ok(())
    }
}

/// Mounts a [`Tool`] into the shared tool registry as a plugin.
#[cfg(feature = "tool")]
pub struct ToolPlugin {
    pub tool: Arc<dyn Tool>,
}

#[cfg(feature = "tool")]
impl ToolPlugin {
    pub fn new(tool: Arc<dyn Tool>) -> Self {
        Self { tool }
    }
}

/// Mounts a [`Skill`] into the shared skill registry as a plugin.
#[cfg(feature = "skill")]
pub struct SkillPlugin {
    pub skill: Skill,
    pub active: bool,
}

#[cfg(feature = "skill")]
impl SkillPlugin {
    pub fn new(skill: Skill) -> Self {
        Self {
            skill,
            active: false,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

#[cfg(feature = "skill")]
#[async_trait]
impl Plugin for SkillPlugin {
    fn name(&self) -> &str {
        &self.skill.name
    }

    async fn apply(
        &self,
        env: &FuneraEnv,
        _config: Option<&PluginConfig>,
    ) -> Result<(), PluginError> {
        let skill_name = self.skill.name.clone();
        env.add_skill(self.skill.clone()).await;
        if self.active {
            env.activate_skill(&skill_name).await;
        }

        let teardown_env = env.clone();
        env.effect(|| {
            Box::new(move || {
                let skill_name = skill_name.clone();
                spawn_teardown(async move {
                    let _ = teardown_env.remove_skill(&skill_name).await;
                });
            })
        });

        Ok(())
    }
}

/// Mounts an LLM client/model pair as a provider plugin.
///
/// `apply` pushes the client and model into the env's hot-reload watch
/// channels; the previous values are restored on unload.
pub struct ProviderPlugin {
    pub name: String,
    pub client: async_openai::Client<OpenAIConfig>,
    pub model: String,
}

impl ProviderPlugin {
    pub fn new(
        name: impl Into<String>,
        client: async_openai::Client<OpenAIConfig>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            client,
            model: model.into(),
        }
    }
}

#[async_trait]
impl Plugin for ProviderPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    async fn apply(
        &self,
        env: &FuneraEnv,
        _config: Option<&PluginConfig>,
    ) -> Result<(), PluginError> {
        let old_model = env.model();
        let old_client = env.current_client();

        env.set_client(self.client.clone());
        env.set_model(self.model.clone());

        let revert_env = env.clone();
        env.effect(|| {
            Box::new(move || {
                revert_env.set_client(old_client);
                revert_env.set_model(old_model);
            })
        });

        Ok(())
    }
}

/// Mounts a middleware layer into a lock-protected [`MiddlewareChain`].
///
/// The ReAct loop reads from the same lock on every event, so middleware can
/// be added/removed at runtime through the plugin lifecycle.
pub struct MiddlewarePlugin<Evt: Clone + Send + 'static> {
    pub name: String,
    pub layer: MiddlewareLayer<Evt>,
    pub chain: Option<Arc<parking_lot::RwLock<MiddlewareChain<Evt, ErrorsEnabled>>>>,
}

impl<Evt: Clone + Send + 'static> MiddlewarePlugin<Evt> {
    pub fn new(
        name: impl Into<String>,
        layer: MiddlewareLayer<Evt>,
        chain: Arc<parking_lot::RwLock<MiddlewareChain<Evt, ErrorsEnabled>>>,
    ) -> Self {
        Self {
            name: name.into(),
            layer,
            chain: Some(chain),
        }
    }
}

#[async_trait]
impl<Evt: Clone + Send + 'static> Plugin for MiddlewarePlugin<Evt> {
    fn name(&self) -> &str {
        &self.name
    }

    async fn apply(
        &self,
        env: &FuneraEnv,
        _config: Option<&PluginConfig>,
    ) -> Result<(), PluginError> {
        if let Some(chain) = &self.chain {
            chain.write().add_layer(self.layer.clone());

            let teardown_chain = Arc::clone(chain);
            let plugin_name = self.name.clone();
            env.effect(|| {
                Box::new(move || {
                    teardown_chain.write().remove_plugin(&plugin_name);
                })
            });
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "tool"))]
mod tool_tests {
    use super::*;
    use crate::plugin::PluginRegistry;
    use crate::plugin::registry::PluginPhase;
    use crate::re_act::tool::ToolCallError;
    use serde_json::Value as JsonValue;
    use std::time::Duration;

    struct MockTool;

    impl Plugin for MockTool {
        fn name(&self) -> &str {
            "mock_tool"
        }
    }

    #[async_trait]
    impl Tool for MockTool {
        fn description(&self) -> &str {
            "mock tool"
        }

        fn schema(&self) -> JsonValue {
            serde_json::json!({"type": "function", "function": {"name": "mock_tool"}})
        }

        async fn execute(&self, _args: JsonValue) -> Result<String, ToolCallError> {
            Ok("ok".into())
        }
    }

    async fn wait_until<F: Fn() -> bool>(mut cond: F) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if cond() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("condition not met within timeout");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn tool_plugin_adds_and_removes_tool() {
        let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "test-model");
        let mut reg = PluginRegistry::new(env);
        let id = reg.mount(Arc::new(ToolPlugin::new(Arc::new(MockTool))));
        reg.refresh().await;

        assert_eq!(reg.phase(id), Some(PluginPhase::Active));
        assert!(
            reg.env()
                .tool_registry
                .read()
                .await
                .tool_exists("mock_tool")
        );

        reg.unmount(id).await;
        wait_until(|| {
            reg.env()
                .tool_registry
                .try_read()
                .map(|r| !r.tool_exists("mock_tool"))
                .unwrap_or(false)
        })
        .await;
    }
}

#[cfg(all(test, feature = "skill"))]
mod skill_tests {
    use super::*;
    use crate::plugin::PluginRegistry;
    use crate::plugin::registry::PluginPhase;
    use crate::re_act::skills::Skill;
    use std::time::Duration;

    async fn wait_until<F: Fn() -> bool>(mut cond: F) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if cond() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("condition not met within timeout");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn skill_plugin_adds_and_removes_skill() {
        let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "test-model");
        let mut reg = PluginRegistry::new(env);
        let skill = Skill::new("s1", "desc", "content");
        let id = reg.mount(Arc::new(SkillPlugin::new(skill).active(true)));
        reg.refresh().await;

        assert_eq!(reg.phase(id), Some(PluginPhase::Active));
        assert!(reg.env().skill_registry.read().await.contains("s1"));
        assert!(reg.env().skill_registry.read().await.is_active("s1"));

        reg.unmount(id).await;
        wait_until(|| {
            reg.env()
                .skill_registry
                .try_read()
                .map(|r| !r.contains("s1"))
                .unwrap_or(false)
        })
        .await;
    }
}

#[cfg(test)]
mod middleware_tests {
    use super::*;
    use crate::middleware::{InspectorError, MiddlewareLayer, MutatorAction, MutatorMiddleware};
    use crate::plugin::registry::PluginPhase;
    use crate::plugin::{Plugin, PluginError, PluginRegistry};
    use parking_lot::RwLock;

    struct AppendMutator;
    impl Plugin for AppendMutator {
        fn name(&self) -> &str {
            "append"
        }
    }
    impl MutatorMiddleware<String> for AppendMutator {
        fn process(&self, event: String) -> MutatorAction<String> {
            MutatorAction::Modify(format!("{event}!"))
        }
    }

    #[tokio::test]
    async fn middleware_plugin_adds_and_removes_layer() {
        let (env, _watcher) = FuneraEnv::new(async_openai::Client::new(), "test-model");
        let chain = MiddlewareChain::<String>::new().activate_error_channel().0;
        let chain = Arc::new(RwLock::new(chain));
        let layer = MiddlewareLayer::Mutator(vec![Arc::new(AppendMutator)]);
        let plugin = MiddlewarePlugin::new("append", layer, Arc::clone(&chain));

        let mut reg = PluginRegistry::new(env);
        let id = reg.mount(Arc::new(plugin));
        reg.refresh().await;

        assert_eq!(reg.phase(id), Some(PluginPhase::Active));
        assert_eq!(chain.read().process("hi".into()).unwrap(), "hi!");

        reg.unmount(id).await;
        assert_eq!(chain.read().process("hi".into()).unwrap(), "hi");
    }
}

#[cfg(test)]
mod provider_tests {
    use super::*;
    use crate::plugin::PluginRegistry;
    use crate::plugin::registry::PluginPhase;

    #[tokio::test]
    async fn provider_plugin_switches_and_reverts_model() {
        let (env, mut watcher) = FuneraEnv::new(async_openai::Client::new(), "old-model");
        let mut reg = PluginRegistry::new(env);

        let id = reg.mount(Arc::new(ProviderPlugin::new(
            "provider",
            async_openai::Client::new(),
            "new-model",
        )));
        reg.refresh().await;

        assert_eq!(reg.phase(id), Some(PluginPhase::Active));
        assert_eq!(reg.env().model(), "new-model");
        assert_eq!(watcher.watch_model(), "new-model");

        reg.unmount(id).await;
        assert_eq!(reg.env().model(), "old-model");
        assert_eq!(watcher.watch_model(), "old-model");
    }
}
