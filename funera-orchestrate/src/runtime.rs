use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;

use async_openai::config::OpenAIConfig;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::task::JoinHandle;

use funera_core::chat::session::{spawn_session_actor, FuneraSession, SessionCmd};
#[cfg(feature = "deepseek")]
use funera_core::provider::deepseek::DeepSeekProvider;
use funera_core::env::{FuneraEnv, FuneraEnvWatcher};
use funera_core::event_bus::env_state_bus::EnvStateEvent;
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::provider::ChatProvider;
use funera_core::re_act::skills::{Skill, SkillRegistry};
use funera_core::re_act::tool::{Tool, ToolRegistry};
use funera_core::re_act::tool_executor::ToolExecutor;

#[cfg(feature = "middleware")]
use crate::event::AgentEvent;
#[cfg(feature = "middleware")]
use crate::middleware_bundle::MiddlewareBundle;
#[cfg(feature = "middleware")]
use funera_core::middleware::{ErrorsEnabled, MiddlewareChain};

use crate::error::OrchestrateError;

/// Builds an [`AgentRuntime`].
///
/// ```rust,no_run
/// # use funera_orchestrate::{AgentRuntime, DeepSeekProvider};
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut runtime = AgentRuntime::<DeepSeekProvider>::builder()
///     .api_key(std::env::var("OPENAI_API_KEY")?)
///     .model("gpt-4o")
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct AgentRuntimeBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
    client: Option<async_openai::Client<OpenAIConfig>>,
    model: Option<String>,
    max_iterations: usize,
    channel_buffer: usize,
    tools: Vec<Box<dyn Tool>>,
    skills: Vec<Skill>,
    skill_names_to_activate: Vec<String>,
    load_default_skills: bool,
    #[cfg(feature = "middleware")]
    middleware_bundle: Option<MiddlewareBundle<AgentEvent>>,
}

impl Default for AgentRuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRuntimeBuilder {
    pub fn new() -> Self {
        Self {
            api_key: None,
            base_url: None,
            client: None,
            model: None,
            max_iterations: 10,
            channel_buffer: 32,
            tools: Vec::new(),
            skills: Vec::new(),
            skill_names_to_activate: Vec::new(),
            load_default_skills: false,
            #[cfg(feature = "middleware")]
            middleware_bundle: None,
        }
    }

    /// OpenAI API key. Falls back to `OPENAI_API_KEY` env var.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Custom base URL (proxy, compatible API, etc.).
    /// Pass e.g. `std::env::var("OPENAI_BASE_URL").ok()`.
    pub fn base_url(mut self, url: Option<String>) -> Self {
        if let Some(u) = url {
            self.base_url = Some(u);
        }
        self
    }

    /// LLM model name. Falls back to `OPENAI_MODEL` env var, then `"gpt-4o"`.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Directly provide an OpenAI client (overrides api_key + base_url).
    pub fn client(mut self, client: async_openai::Client<OpenAIConfig>) -> Self {
        self.client = Some(client);
        self
    }

    /// Maximum number of ReAct iterations per call (default 10).
    pub fn max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    /// Internal channel buffer size (default 32).
    pub fn channel_buffer(mut self, n: usize) -> Self {
        self.channel_buffer = n;
        self
    }

    /// Load a skill from a SKILL.md file.
    pub fn with_skill_file(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match Skill::from_file(&path) {
            Ok(skill) => {
                self.skills.push(skill);
            }
            Err(e) => {
                eprintln!("warn: failed to load skill from {:?}: {}", path, e);
            }
        }
        self
    }

    /// Load all SKILL.md files from a directory.
    pub fn with_skills_dir(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match Skill::from_dir(&path) {
            Ok(skills) => self.skills.extend(skills),
            Err(e) => {
                eprintln!("warn: failed to load skills from {:?}: {}", path, e);
            }
        }
        self
    }

    /// Register an inline skill definition.
    pub fn with_skill(
        mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        self.skills.push(Skill::new(name, description, content));
        self
    }

    /// Activate a previously loaded skill by name.
    /// If the skill does not exist, the call is silently ignored.
    pub fn with_skill_active(mut self, name: impl Into<String>) -> Self {
        self.skill_names_to_activate.push(name.into());
        self
    }

    /// Auto-discover and load skills from default paths
    /// (`$SKILLS_HOME` or `~/.agents/skills/`), then activate them.
    pub fn with_skills_default_path(mut self) -> Self {
        self.load_default_skills = true;
        self
    }

    /// Register a tool by its type (requires `Tool + Default`).
    pub fn with_tool<T: Tool + 'static>(mut self) -> Self
    where
        T: Default,
    {
        self.tools.push(Box::new(T::default()));
        self
    }

    /// Register a pre-constructed tool.
    pub fn with_tool_instance(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Attach a middleware chain with error channel.
    ///
    /// The runtime will spawn a task to consume inspector errors via `tracing::warn`.
    #[cfg(feature = "middleware")]
    pub fn with_middleware_bundle(mut self, bundle: MiddlewareBundle<AgentEvent>) -> Self {
        self.middleware_bundle = Some(bundle);
        self
    }

    /// Register all builtin tools (Read, Write, Edit, Shell).
    /// Requires the `builtin-tools` feature.
    #[cfg(feature = "builtin-tools")]
    pub fn with_builtin_tools(mut self) -> Self {
        use builtin_tools::{EditTool, ReadTool, ShellTool, WriteTool};
        self.tools.push(Box::new(ReadTool));
        self.tools.push(Box::new(WriteTool));
        self.tools.push(Box::new(EditTool));
        self.tools.push(Box::new(ShellTool));
        self
    }

    /// Build the runtime with the default DeepSeek provider.
    ///
    /// Spawns a background `ToolExecutor` task that lives for the runtime's
    /// lifetime and processes tool calls from the ReAct loop.
    #[cfg(feature = "deepseek")]
    pub fn build(self) -> Result<AgentRuntime<DeepSeekProvider>, OrchestrateError> {
        self.build_with::<DeepSeekProvider>()
    }

    /// Build the runtime with a custom LLM provider.
    ///
    /// ```rust,no_run
    /// # use funera_orchestrate::{AgentRuntime, DeepSeekProvider};
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let rt = AgentRuntime::<DeepSeekProvider>::builder().api_key("sk-key").model("gpt-4o").build_with::<DeepSeekProvider>()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn build_with<P: ChatProvider>(mut self) -> Result<AgentRuntime<P>, OrchestrateError> {
        let api_key = self.api_key.or_else(|| std::env::var("OPENAI_API_KEY").ok());
        let model = self
            .model
            .or_else(|| std::env::var("OPENAI_MODEL").ok())
            .unwrap_or_else(|| "gpt-4o".into());

        let client = match self.client {
            Some(c) => c,
            None => {
                let key = api_key.ok_or_else(|| {
                    OrchestrateError::Config(
                        "no API key; set OPENAI_API_KEY or call .api_key()".into(),
                    )
                })?;
                let mut cfg = OpenAIConfig::default().with_api_key(key);
                if let Some(url) = &self.base_url {
                    cfg = cfg.with_api_base(url);
                }
                async_openai::Client::with_config(cfg)
            }
        };

        let mut registry = ToolRegistry::new();
        for t in self.tools {
            registry.add_tool(t);
        }

        // Pre-build SkillRegistry synchronously (no locks needed yet)
        let mut skill_registry = funera_core::re_act::skills::SkillRegistry::new();

        // Load default skills if requested (auto-discover from $SKILLS_HOME / ~/.agents/skills/)
        if self.load_default_skills {
            let default_skills = Skill::from_default_path();
            for skill in default_skills {
                let name = skill.name.clone();
                skill_registry.add(skill);
                self.skill_names_to_activate.push(name);
            }
        }

        // Add all explicitly registered skills
        for skill in self.skills {
            skill_registry.add(skill);
        }

        // Activate specified skills
        for name in &self.skill_names_to_activate {
            skill_registry.activate(&name);
        }

        // Create env with pre-built skill registry (prompt is computed internally)
        let (env, env_watcher) = FuneraEnv::with_skills(registry, client, &model, skill_registry);

        // Runtime-level env state event channel (persistent, not per-call)
        let (env_state_tx, _) = broadcast::channel(32);

        // Emit runtime-level events for pre-registered tools
        if let Ok(guard) = env.tool_registry.try_read() {
            let tools = guard.get_all_tools();
            for name in tools.keys() {
                let _ = env_state_tx.send(EnvStateEvent::ToolAdded(name.clone()));
            }
        }

        // Emit runtime-level events for pre-registered skills
        if let Ok(guard) = env.skill_registry.try_read() {
            let skills = guard.all_skills();
            for name in skills.keys() {
                let _ = env_state_tx.send(EnvStateEvent::SkillAdded(name.clone()));
            }
        }

        #[cfg(feature = "middleware")]
        let middleware_chain = if let Some(bundle) = self.middleware_bundle.take() {
            let MiddlewareBundle { chain, error_rx } = bundle;
            tokio::spawn(async move {
                let mut rx = error_rx;
                while let Some((name, err)) = rx.recv().await {
                    tracing::warn!("[middleware:{name}] inspector error: {err}");
                }
            });
            Arc::new(chain)
        } else {
            let (chain, error_rx) = MiddlewareChain::<AgentEvent>::new().activate_error_channel();
            tokio::spawn(async move {
                let mut rx = error_rx;
                while let Some((name, err)) = rx.recv().await {
                    tracing::warn!("[middleware:{name}] inspector error: {err}");
                }
            });
            Arc::new(chain)
        };

        let (tool_bus, exec_rx) = ToolBus::new();
        let reg = env.tool_registry.clone();
        let handle = tokio::spawn(async move {
            ToolExecutor::new(reg, exec_rx).run().await;
        });

        let session_tx = spawn_session_actor();

        Ok(AgentRuntime::<P> {
            env,
            env_watcher,
            tool_bus,
            model,
            max_iterations: self.max_iterations,
            channel_buffer: self.channel_buffer,
            env_state_tx,
            _executor_handle: handle,
            session_tx,
            _state: PhantomData,
            _phantom: PhantomData,
            #[cfg(feature = "middleware")]
            middleware_chain,
        })
    }
}

/// A runtime context for executing agent interactions.
///
pub struct Idle;
pub struct Acquired;

/// `AgentRuntime` owns the shared infrastructure (LLM client, tool registry,
/// tool executor) and a persistent session (backed by a session actor).
///
/// The generic parameter `S` is a type-state marker — [`Idle`] means
/// no `send`/`send_stream` is in progress, [`Acquired``] means one is active.
/// Send operations consume `AgentRuntime<P, Idle>` and return a handle that
/// eventually yields back `AgentRuntime<P, Idle>`.
pub struct AgentRuntime<P: ChatProvider, S = Idle> {
    env: FuneraEnv,
    pub(crate) env_watcher: FuneraEnvWatcher,
    pub(crate) tool_bus: ToolBus,
    pub(crate) model: String,
    pub(crate) max_iterations: usize,
    pub(crate) channel_buffer: usize,
    env_state_tx: broadcast::Sender<EnvStateEvent>,
    _executor_handle: JoinHandle<()>,
    pub(crate) session_tx: mpsc::UnboundedSender<SessionCmd>,
    _state: PhantomData<S>,
    _phantom: PhantomData<fn() -> P>,
    #[cfg(feature = "middleware")]
    middleware_chain: Arc<MiddlewareChain<AgentEvent, ErrorsEnabled>>,
}

// ── All state markers share these methods ─────────────────────

impl<P: ChatProvider, S> AgentRuntime<P, S> {
    /// Create a new builder.
    pub fn builder() -> AgentRuntimeBuilder {
        AgentRuntimeBuilder::new()
    }

    /// Reset the conversation session (clear message history).
    pub fn reset(&self) {
        let _ = self.session_tx.send(SessionCmd::Clear);
    }

    /// Access the session control channel.
    pub fn session_tx(&self) -> mpsc::UnboundedSender<SessionCmd> {
        self.session_tx.clone()
    }

    /// The LLM model name configured for this runtime.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Maximum ReAct iterations per call.
    pub fn max_iterations(&self) -> usize {
        self.max_iterations
    }

    /// Channel buffer size.
    pub fn channel_buffer(&self) -> usize {
        self.channel_buffer
    }

    /// The tool registry (for dynamic tool management).
    pub fn tool_registry(&self) -> Arc<RwLock<ToolRegistry>> {
        self.env.tool_registry.clone()
    }

    /// The skill registry (for dynamic skill management).
    pub fn skill_registry(&self) -> Arc<RwLock<SkillRegistry>> {
        self.env.skill_registry.clone()
    }

    /// Clone the env watcher for a session.
    pub(crate) fn env_watcher(&self) -> FuneraEnvWatcher {
        self.env_watcher.clone()
    }

    /// Subscribe to runtime-level environment state events.
    ///
    /// The returned receiver yields [`EnvStateEvent`] notifications about
    /// tool/skill registration changes, LLM model changes, etc. that occur
    /// during the runtime's lifetime.
    ///
    /// Unlike [`Agent::subscribe_raw_events`](crate::Agent::subscribe_raw_events)
    /// which only delivers events during a `fire`/`send` call, this subscription
    /// is persistent and independent of any agent call.
    pub fn subscribe_env_state(&self) -> broadcast::Receiver<EnvStateEvent> {
        self.env_state_tx.subscribe()
    }

    /// Access the middleware chain for event filtering.
    #[cfg(feature = "middleware")]
    pub fn middleware_chain(&self) -> Arc<MiddlewareChain<AgentEvent, ErrorsEnabled>> {
        self.middleware_chain.clone()
    }

    /// Transform the runtime into `Acquired` state (internal use).
    pub(crate) fn into_acquired(self) -> AgentRuntime<P, Acquired> {
        AgentRuntime::<P, Acquired> {
            env: self.env,
            env_watcher: self.env_watcher,
            tool_bus: self.tool_bus,
            model: self.model,
            max_iterations: self.max_iterations,
            channel_buffer: self.channel_buffer,
            env_state_tx: self.env_state_tx,
            _executor_handle: self._executor_handle,
            session_tx: self.session_tx,
            _state: PhantomData,
            _phantom: PhantomData,
            #[cfg(feature = "middleware")]
            middleware_chain: self.middleware_chain,
        }
    }
}

// ── Acquired → Idle ─────────────────────────────────────────

impl<P: ChatProvider> AgentRuntime<P, Acquired> {
    pub(crate) fn into_idle(self) -> AgentRuntime<P, Idle> {
        AgentRuntime::<P, Idle> {
            env: self.env,
            env_watcher: self.env_watcher,
            tool_bus: self.tool_bus,
            model: self.model,
            max_iterations: self.max_iterations,
            channel_buffer: self.channel_buffer,
            env_state_tx: self.env_state_tx,
            _executor_handle: self._executor_handle,
            session_tx: self.session_tx,
            _state: PhantomData,
            _phantom: PhantomData,
            #[cfg(feature = "middleware")]
            middleware_chain: self.middleware_chain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use funera_core::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};
    use funera_core::re_act::tool::ToolCallError;

    // ── inline mock tool ───────────────────────────────────────────

    #[derive(Default)]
    struct MockTool;

    #[async_trait::async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            "mock_tool"
        }
        fn description(&self) -> &str {
            "A mock tool for testing"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        async fn execute(&self, _args: serde_json::Value) -> Result<String, ToolCallError> {
            Ok("ok".into())
        }
    }

    // ── builder defaults ───────────────────────────────────────────

    #[test]
    fn builder_defaults() {
        let b = AgentRuntimeBuilder::new();
        assert_eq!(b.max_iterations, 10);
        assert_eq!(b.channel_buffer, 32);
        assert!(b.api_key.is_none());
        assert!(b.model.is_none());
        assert!(b.tools.is_empty());
    }

    #[test]
    fn builder_set_max_iterations() {
        let b = AgentRuntimeBuilder::new().max_iterations(20);
        assert_eq!(b.max_iterations, 20);
    }

    #[test]
    fn builder_set_channel_buffer() {
        let b = AgentRuntimeBuilder::new().channel_buffer(64);
        assert_eq!(b.channel_buffer, 64);
    }

    #[test]
    fn builder_set_model() {
        let b = AgentRuntimeBuilder::new().model("test-model");
        assert_eq!(b.model, Some("test-model".into()));
    }

    #[test]
    fn builder_set_api_key() {
        let b = AgentRuntimeBuilder::new().api_key("sk-test");
        assert_eq!(b.api_key, Some("sk-test".into()));
    }

    #[test]
    fn builder_set_base_url() {
        let b = AgentRuntimeBuilder::new().base_url(Some("https://example.com".into()));
        assert_eq!(b.base_url, Some("https://example.com".into()));
    }

    #[test]
    fn builder_set_base_url_none_noop() {
        let b = AgentRuntimeBuilder::new().base_url(None);
        assert!(b.base_url.is_none());
    }

    #[test]
    fn builder_set_client() {
        let cfg = async_openai::config::OpenAIConfig::default();
        let client = async_openai::Client::with_config(cfg);
        let b = AgentRuntimeBuilder::new().client(client);
        assert!(b.client.is_some());
    }

    #[test]
    fn builder_with_tool_instance() {
        let b = AgentRuntimeBuilder::new()
            .with_tool_instance(Box::new(MockTool));
        assert_eq!(b.tools.len(), 1);
    }

    // ── skill builder methods ───────────────────────────────────────

    #[test]
    fn builder_with_skill_inline() {
        let b = AgentRuntimeBuilder::new()
            .with_skill("s1", "desc", "content");
        assert_eq!(b.skills.len(), 1);
        assert_eq!(b.skills[0].name, "s1");
        assert_eq!(b.skills[0].description, "desc");
        assert_eq!(b.skills[0].content, "content");
    }

    #[test]
    fn builder_with_skill_active_adds_to_list() {
        let b = AgentRuntimeBuilder::new()
            .with_skill_active("s1")
            .with_skill_active("s2");
        assert_eq!(b.skill_names_to_activate, vec!["s1", "s2"]);
    }

    #[test]
    fn builder_with_skills_default_path_sets_flag() {
        let b = AgentRuntimeBuilder::new().with_skills_default_path();
        assert!(b.load_default_skills);
    }

    #[test]
    fn builder_skills_combined() {
        let b = AgentRuntimeBuilder::new()
            .with_skill("a", "", "aaa")
            .with_skill("b", "", "bbb")
            .with_skill_active("a");
        assert_eq!(b.skills.len(), 2);
        assert_eq!(b.skill_names_to_activate, vec!["a"]);
    }

    // ── build ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn build_with_explicit_key() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("test-model")
            .build()
            .expect("build should succeed with api_key");
        assert_eq!(rt.model(), "test-model");
        assert_eq!(rt.max_iterations(), 10);
        assert_eq!(rt.channel_buffer(), 32);
    }

    #[tokio::test]
    async fn build_custom_params() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("my-model")
            .max_iterations(15)
            .channel_buffer(8)
            .build()
            .unwrap();
        assert_eq!(rt.model(), "my-model");
        assert_eq!(rt.max_iterations(), 15);
        assert_eq!(rt.channel_buffer(), 8);
    }

    #[tokio::test]
    async fn build_fails_without_key() {
        let has_key = std::env::var("OPENAI_API_KEY").is_ok();
        if has_key {
            // Can't test failure when key is present in env
            return;
        }
        let result = AgentRuntimeBuilder::new().model("x").build();
        assert!(matches!(result, Err(OrchestrateError::Config(_))));
    }

    #[tokio::test]
    async fn build_with_tool_adds_to_registry() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .with_tool::<MockTool>()
            .build()
            .unwrap();
        let registry = rt.tool_registry();
        let guard = registry.read().await;
        let tools = guard.get_all_tools();
        assert!(tools.contains_key("mock_tool"));
    }

    #[tokio::test]
    async fn build_model_fallback_default() {
        let has_model = std::env::var("OPENAI_MODEL").is_ok();
        if has_model {
            return;
        }
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .build()
            .unwrap();
        assert_eq!(rt.model(), "gpt-4o");
    }

    // ── session management ─────────────────────────────────────────

    #[tokio::test]
    async fn session_actor_is_alive() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let tx = rt.session_tx();
        assert!(tx.send(SessionCmd::Clear).is_ok());
    }

    #[tokio::test]
    async fn session_context_works_immediately() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let ctx = FuneraSession::new(rt.session_tx())
            .session_context()
            .await;
        assert!(ctx.is_empty());
    }

    #[tokio::test]
    async fn reset_clears_messages() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let session = FuneraSession::new(rt.session_tx());
        session.push_message(FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage { text: "hi".into(), reasoning_content: None }),
        ));
        let ctx_before = session.session_context().await;
        assert_eq!(ctx_before.len(), 1);

        rt.reset();

        let ctx_after = session.session_context().await;
        assert_eq!(ctx_after.len(), 0);
    }

    // ── accessors ──────────────────────────────────────────────────

    #[tokio::test]
    async fn tool_registry_accessor() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let reg = rt.tool_registry();
        let guard = reg.read().await;
        let tools = guard.get_all_tools();
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn subscribe_env_state_works() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let mut rx = rt.subscribe_env_state();
        // Send an event after subscribing to verify the channel works
        rt.env_state_tx
            .send(EnvStateEvent::LlmChanged("new-model".into()))
            .unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await;
        assert!(matches!(
            got,
            Ok(Ok(EnvStateEvent::LlmChanged(m))) if m == "new-model"
        ));
    }
}
