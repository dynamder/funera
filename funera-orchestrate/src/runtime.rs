use std::sync::Arc;

use async_openai::config::OpenAIConfig;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use funera_core::chat::session::{FuneraSession, Idle};
use funera_core::env::{FuneraEnv, FuneraEnvWatcher};
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::{Tool, ToolRegistry};
use funera_core::re_act::tool_executor::ToolExecutor;

use crate::error::OrchestrateError;

/// Builds an [`AgentRuntime`].
///
/// ```rust,no_run
/// # use funera_orchestrate::AgentRuntime;
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut runtime = AgentRuntime::builder()
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

    /// Build the runtime.
    ///
    /// Spawns a background `ToolExecutor` task that lives for the runtime's
    /// lifetime and processes tool calls from the ReAct loop.
    pub fn build(self) -> Result<AgentRuntime, OrchestrateError> {
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

        let (env, env_watcher) = FuneraEnv::new(registry, client, &model);
        let (tool_bus, exec_rx) = ToolBus::new();
        let reg = env.tool_registry.clone();
        let handle = tokio::spawn(async move {
            ToolExecutor::new(reg, exec_rx).run().await;
        });

        Ok(AgentRuntime {
            env,
            env_watcher,
            tool_bus,
            model,
            max_iterations: self.max_iterations,
            channel_buffer: self.channel_buffer,
            _executor_handle: handle,
            session: None,
        })
    }
}

/// A runtime context for executing agent interactions.
///
/// `AgentRuntime` owns the shared infrastructure (LLM client, tool registry,
/// tool executor) and an optional persistent conversation session.
///
/// Pass `&mut AgentRuntime` to [`Agent::send`](crate::Agent::send) /
/// [`Agent::send_stream`](crate::Agent::send_stream) for multi-turn
/// conversations, or `&AgentRuntime` to [`Agent::fire`](crate::Agent::fire) /
/// [`Agent::fire_stream`](crate::Agent::fire_stream) for one-shot queries.
pub struct AgentRuntime {
    env: FuneraEnv,
    pub(crate) env_watcher: FuneraEnvWatcher,
    pub(crate) tool_bus: ToolBus,
    pub(crate) model: String,
    pub(crate) max_iterations: usize,
    pub(crate) channel_buffer: usize,
    _executor_handle: JoinHandle<()>,
    session: Option<FuneraSession<Idle>>,
}

impl AgentRuntime {
    /// Create a new builder.
    pub fn builder() -> AgentRuntimeBuilder {
        AgentRuntimeBuilder::new()
    }

    /// Reset the conversation session (clear message history).
    pub fn reset(&mut self) {
        self.session = None;
    }

    /// Take the current session, or create a fresh one.
    pub(crate) fn take_session(&mut self) -> FuneraSession<Idle> {
        self.session.take().unwrap_or_else(FuneraSession::<Idle>::new)
    }

    /// Store a session back.
    pub(crate) fn store_session(&mut self, session: FuneraSession<Idle>) {
        self.session = Some(session);
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

    /// Clone the env watcher for a session.
    pub(crate) fn env_watcher(&self) -> FuneraEnvWatcher {
        self.env_watcher.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn session_take_empty_creates_new() {
        let mut rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let s1 = rt.take_session();
        let s2 = rt.take_session();
        // Two different sessions
        assert_ne!(s1.id(), s2.id());
    }

    #[tokio::test]
    async fn session_store_and_retrieve() {
        let mut rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let s = rt.take_session();
        let id = s.id();
        rt.store_session(s);
        let s2 = rt.take_session();
        assert_eq!(s2.id(), id, "should retrieve the same session");
    }

    #[tokio::test]
    async fn session_reset_clears() {
        let mut rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let s = rt.take_session();
        let id = s.id();
        rt.store_session(s);
        rt.reset();
        let s2 = rt.take_session();
        assert_ne!(s2.id(), id, "reset should create a new session");
    }

    #[tokio::test]
    async fn session_multiple_store_take() {
        let mut rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let s1 = rt.take_session();
        let id1 = s1.id();
        rt.store_session(s1);

        let s2 = rt.take_session();
        assert_eq!(s2.id(), id1);

        let id2 = s2.id();
        rt.store_session(s2);
        rt.reset();

        let s3 = rt.take_session();
        assert_ne!(s3.id(), id2);
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
}
