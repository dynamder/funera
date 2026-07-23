use std::marker::PhantomData;
#[cfg(feature = "skill")]
use std::path::PathBuf;
#[cfg(any(feature = "middleware", feature = "security"))]
use std::sync::Arc;

use async_openai::config::OpenAIConfig;
use tokio::sync::{broadcast, mpsc};

#[cfg(test)]
use funera_core::chat::session::FuneraSession;
use funera_core::chat::session::{SessionCmd, spawn_session_actor};
use funera_core::env::FuneraEnv;
use funera_core::env_actor::{EnvCmd, ReActConfig, spawn_env_actor};
use funera_core::event_bus::env_state_bus::EnvStateEvent;
#[cfg(feature = "tool")]
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::provider::ChatProvider;
#[cfg(feature = "deepseek")]
use funera_core::provider::deepseek::DeepSeekProvider;
#[cfg(feature = "skill")]
use funera_core::re_act::skills::{Skill, SkillRegistry};
#[cfg(feature = "tool")]
use funera_core::re_act::tool::{Tool, ToolRegistry};
#[cfg(feature = "security")]
use funera_core::security::audit::{AuditBus, AuditEvent};
#[cfg(all(feature = "sandbox", feature = "security"))]
use funera_core::security::path_guard::PathGuard;
#[cfg(feature = "security")]
use funera_core::security::policy::ToolPolicy;
#[cfg(feature = "security")]
use funera_core::security::registry::ApprovalCallback;
#[cfg(feature = "sandbox")]
use funera_core::security::sandbox::SandboxPolicy;
#[cfg(feature = "security")]
use funera_core::security::secret::SecureApiKey;

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
/// let runtime = AgentRuntime::<DeepSeekProvider>::builder()
///     .api_key(std::env::var("DEEPSEEK_API_KEY")?)
///     .model("deepseek-v4-flash")
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
    #[cfg(feature = "tool")]
    tools: Vec<Box<dyn Tool>>,
    #[cfg(feature = "skill")]
    skills: Vec<Skill>,
    #[cfg(feature = "skill")]
    skill_names_to_activate: Vec<String>,
    #[cfg(feature = "skill")]
    load_default_skills: bool,
    #[cfg(feature = "sandbox")]
    sandbox_policy: Option<SandboxPolicy>,
    #[cfg(feature = "security")]
    tool_policy: Option<ToolPolicy>,
    #[cfg(feature = "security")]
    secure_api_key: Option<SecureApiKey>,
    #[cfg(feature = "security")]
    approval_callback: Option<ApprovalCallback>,
    #[cfg(feature = "security")]
    approval_timeout: Option<std::time::Duration>,
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
            #[cfg(feature = "tool")]
            tools: Vec::new(),
            #[cfg(feature = "skill")]
            skills: Vec::new(),
            #[cfg(feature = "skill")]
            skill_names_to_activate: Vec::new(),
            #[cfg(feature = "skill")]
            load_default_skills: false,
            #[cfg(feature = "sandbox")]
            sandbox_policy: None,
            #[cfg(feature = "security")]
            tool_policy: None,
            #[cfg(feature = "security")]
            secure_api_key: None,
            #[cfg(feature = "security")]
            approval_callback: None,
            #[cfg(feature = "security")]
            approval_timeout: None,
            #[cfg(feature = "middleware")]
            middleware_bundle: None,
        }
    }

    /// OpenAI API key. Falls back to `OPENAI_API_KEY` env var.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        let key = key.into();
        #[cfg(feature = "security")]
        {
            self.secure_api_key = Some(SecureApiKey::new(key.clone()));
        }
        self.api_key = Some(key);
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
    #[cfg(feature = "skill")]
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
    #[cfg(feature = "skill")]
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
    #[cfg(feature = "skill")]
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
    #[cfg(feature = "skill")]
    pub fn with_skill_active(mut self, name: impl Into<String>) -> Self {
        self.skill_names_to_activate.push(name.into());
        self
    }

    /// Auto-discover and load skills from default paths
    /// (`$SKILLS_HOME` or `~/.agents/skills/`), then activate them.
    #[cfg(feature = "skill")]
    pub fn with_skills_default_path(mut self) -> Self {
        self.load_default_skills = true;
        self
    }

    /// Register a tool by its type (requires `Tool + Default`).
    #[cfg(feature = "tool")]
    pub fn with_tool<T: Tool + Default + 'static>(mut self) -> Self {
        self.tools.push(Box::new(T::default()));
        self
    }

    /// Register a pre-constructed tool.
    #[cfg(feature = "tool")]
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

    /// Set a kernel-enforced sandbox policy for tool subprocesses.
    #[cfg(feature = "sandbox")]
    pub fn with_sandbox_policy(mut self, policy: SandboxPolicy) -> Self {
        self.sandbox_policy = Some(policy);
        self
    }

    /// Set an application-level tool policy.
    #[cfg(feature = "security")]
    pub fn with_tool_policy(mut self, policy: ToolPolicy) -> Self {
        self.tool_policy = Some(policy);
        self
    }

    /// Register a notification callback fired when a tool call requires user approval.
    #[cfg(feature = "security")]
    pub fn on_approval_required(
        mut self,
        cb: impl Fn(Arc<str>, String, String) + Send + Sync + 'static,
    ) -> Self {
        self.approval_callback = Some(Arc::new(
            move |call_id: &str, tool_name: &str, reason: &str, _paths: &[std::path::PathBuf]| {
                cb(
                    Arc::from(call_id),
                    tool_name.to_string(),
                    reason.to_string(),
                );
            },
        ));
        self
    }

    /// Set a timeout for tool call approval.
    #[cfg(feature = "security")]
    pub fn with_approval_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.approval_timeout = Some(timeout);
        self
    }

    /// Register all builtin tools (Read, Write, Edit, Shell).
    #[cfg(feature = "funera-builtin-tools")]
    pub fn with_builtin_tools(mut self) -> Self {
        use funera_builtin_tools::{EditTool, ReadTool, ShellTool, WriteTool};
        self.tools.push(Box::new(ReadTool));
        self.tools.push(Box::new(WriteTool));
        self.tools.push(Box::new(EditTool));
        #[cfg(feature = "sandbox")]
        if let Some(ref policy) = self.sandbox_policy {
            self.tools
                .push(Box::new(ShellTool::with_sandbox(policy.clone())));
        } else {
            self.tools.push(Box::new(ShellTool::new()));
        }
        #[cfg(not(feature = "sandbox"))]
        self.tools.push(Box::new(ShellTool::new()));
        self
    }

    /// Build the runtime with the default DeepSeek provider.
    #[cfg(feature = "deepseek")]
    pub fn build(self) -> Result<AgentRuntime<DeepSeekProvider>, OrchestrateError> {
        self.build_with::<DeepSeekProvider>()
    }

    /// Build the runtime with a custom LLM provider.
    ///
    /// ```rust,no_run
    /// # use funera_orchestrate::{AgentRuntime, DeepSeekProvider};
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let rt = AgentRuntime::<DeepSeekProvider>::builder().api_key(std::env::var("DEEPSEEK_API_KEY")?).model("deepseek-v4-flash").build_with::<DeepSeekProvider>()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn build_with<P: ChatProvider>(
        #[allow(unused_mut)] mut self,
    ) -> Result<AgentRuntime<P>, OrchestrateError> {
        #[cfg(feature = "security")]
        let api_key = {
            self.secure_api_key
                .take()
                .map(|k| k.expose_secret().to_string())
                .or_else(|| self.api_key.take())
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        };
        #[cfg(not(feature = "security"))]
        let api_key = self
            .api_key
            .or_else(|| std::env::var("OPENAI_API_KEY").ok());
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

        // Sync sandbox policy into tool policy when only sandbox was configured.
        #[cfg(all(feature = "sandbox", feature = "security"))]
        {
            if self.tool_policy.is_none()
                && let Some(ref sp) = self.sandbox_policy
            {
                self.tool_policy = Some(ToolPolicy {
                    sandbox: sp.clone(),
                    ..Default::default()
                });
            }
        }

        #[cfg(feature = "security")]
        let audit_bus = AuditBus::default();

        #[cfg(feature = "tool")]
        let registry = {
            #[cfg(feature = "security")]
            let mut reg = match self.tool_policy {
                Some(ref policy) => ToolRegistry::new_from_policy(policy.clone()),
                None => ToolRegistry::new(),
            };
            #[cfg(not(feature = "security"))]
            let mut reg = ToolRegistry::new();
            for t in self.tools {
                reg.add_tool(t);
            }

            #[cfg(feature = "security")]
            reg.set_audit_bus(audit_bus.clone());

            #[cfg(all(feature = "sandbox", feature = "security"))]
            if let Some(ref sp) = self.sandbox_policy {
                if sp.enabled && (!sp.read_paths.is_empty() || !sp.read_write_paths.is_empty()) {
                    let all_paths: Vec<_> = sp
                        .read_paths
                        .iter()
                        .chain(sp.read_write_paths.iter())
                        .cloned()
                        .collect();
                    if !all_paths.is_empty() {
                        let path_guard = PathGuard::new(all_paths.iter().map(|p| p.as_path()));
                        reg.set_path_guard(path_guard);
                    }
                }
                reg.set_sandbox_paths(sp.read_paths.clone(), sp.read_write_paths.clone());
            }

            #[cfg(feature = "security")]
            {
                if let Some(ref cb) = self.approval_callback {
                    reg.set_approval_callback(cb.clone());
                }
                if let Some(dur) = self.approval_timeout {
                    reg.set_approval_timeout(Some(dur));
                }
            }

            reg
        };

        #[cfg(feature = "skill")]
        let mut skill_registry = SkillRegistry::new();

        #[cfg(feature = "skill")]
        {
            if self.load_default_skills {
                let default_skills = Skill::from_default_path();
                for skill in default_skills {
                    let name = skill.name.clone();
                    skill_registry.add(skill);
                    self.skill_names_to_activate.push(name);
                }
            }
            for skill in self.skills {
                skill_registry.add(skill);
            }
            for name in &self.skill_names_to_activate {
                skill_registry.activate(name);
            }
        }

        let (env, env_watcher) = FuneraEnv::new(client, &model);

        #[cfg(feature = "sandbox")]
        let env = if let Some(ref sp) = self.sandbox_policy {
            env.with_sandbox_policy(sp.clone())
        } else {
            env
        };

        #[cfg(feature = "tool")]
        let env = env.with_tool_registry(registry);
        #[cfg(feature = "skill")]
        let env = env.with_skill_registry(skill_registry);

        // Build middleware chain
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

        // Create tool bus for ToolExecutor
        #[cfg(feature = "tool")]
        let (tool_bus, exec_rx) = ToolBus::new();

        // Snapshot build-time config for the env actor
        #[cfg(feature = "sandbox")]
        let sandbox_policy = self.sandbox_policy.clone().unwrap_or_default();
        #[cfg(feature = "security")]
        let tool_policy_val = self.tool_policy.clone().unwrap_or_default();

        let max_iters = self.max_iterations;
        let chan_buf = self.channel_buffer;

        // Spawn env actor — owns FuneraEnv, watcher, ToolExecutor, all config
        let env_cmd_tx = spawn_env_actor(
            env,
            env_watcher,
            max_iters,
            chan_buf,
            #[cfg(feature = "tool")]
            tool_bus,
            #[cfg(feature = "tool")]
            exec_rx,
            #[cfg(feature = "sandbox")]
            sandbox_policy,
            #[cfg(feature = "security")]
            tool_policy_val,
            #[cfg(feature = "security")]
            audit_bus,
        );

        let session_tx = spawn_session_actor();

        Ok(AgentRuntime::<P> {
            env_cmd_tx,
            session_tx,
            #[cfg(feature = "middleware")]
            middleware_chain,
            _state: PhantomData,
            _phantom: PhantomData,
        })
    }
}

/// A runtime context for executing agent interactions.
///
/// Marker type-state: the runtime is available for a `send`/`send_stream` call.
pub struct Idle;

/// Marker type-state: a `send`/`send_stream` call is in progress.
pub struct Acquired;

/// Thin wrapper around session and env actors.
///
/// `AgentRuntime` owns **no mutable data** — all persistent state lives in
/// background actors:
///
/// - [`EnvActor`](funera_core::env_actor) — owns [`FuneraEnv`](funera_core::env::FuneraEnv)
///   (model, client, tool/skill registries, sandbox policy), the
///   [`FuneraEnvWatcher`](funera_core::env::FuneraEnvWatcher) (watch-based
///   hot-reload), [`ToolBus`](funera_core::event_bus::tool_bus::ToolBus) +
///   [`ToolExecutor`](funera_core::re_act::tool_executor::ToolExecutor), audit bus,
///   and env state broadcast channel.
/// - [`SessionActor`](funera_core::chat::session) — owns `Vec<FuneraMessage>`.
///
/// The generic parameter `S` is a type-state marker — [`Idle`] means
/// no `send`/`send_stream` is in progress, [`Acquired`] means one is active.
/// Send operations consume `AgentRuntime<P, Idle>` and return a handle that
/// eventually yields back `AgentRuntime<P, Idle>`.
///
/// # Mutation
///
/// All env mutations (e.g., [`set_model`](Self::set_model),
/// [`add_tool`](Self::add_tool)) send an [`EnvCmd`](funera_core::env_actor::EnvCmd)
/// to the actor, which atomically updates the internal state, pushes to the
/// watch channel (picked up by the ReAct loop on the next iteration), and
/// broadcasts an [`EnvStateEvent`](funera_core::event_bus::env_state_bus::EnvStateEvent).
///
/// # Hot-Reload
///
/// The ReAct loop calls [`get_react_config`](Self::get_react_config) once per
/// `fire`/`send` call to obtain a [`ReActConfig`](funera_core::env_actor::ReActConfig)
/// bundle containing the [`FuneraEnvWatcher`]. Every iteration, the watcher
/// snapshots the latest model, client, tools, and skills from the watch
/// channels — enabling zero-coordination runtime changes.
pub struct AgentRuntime<P: ChatProvider, S = Idle> {
    pub(crate) env_cmd_tx: mpsc::UnboundedSender<EnvCmd>,
    pub(crate) session_tx: mpsc::UnboundedSender<SessionCmd>,
    #[cfg(feature = "middleware")]
    pub(crate) middleware_chain: Arc<MiddlewareChain<AgentEvent, ErrorsEnabled>>,
    _state: PhantomData<S>,
    _phantom: PhantomData<fn() -> P>,
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

    /// Subscribe to runtime-level environment state events.
    pub async fn subscribe_env_state(&self) -> broadcast::Receiver<EnvStateEvent> {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::SubscribeEnvState { respond });
        rx.await
            .unwrap_or_else(|_| broadcast::channel::<EnvStateEvent>(1).1)
    }

    /// Subscribe to security audit events.
    #[cfg(feature = "security")]
    pub async fn subscribe_audit(&self) -> broadcast::Receiver<AuditEvent> {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::SubscribeAudit { respond });
        rx.await
            .unwrap_or_else(|_| broadcast::channel::<AuditEvent>(1).1)
    }

    /// Query the current LLM model name from the env actor.
    pub async fn model(&self) -> String {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::GetModel { respond });
        rx.await.unwrap_or_default()
    }

    /// Query the current sandbox policy from the env actor.
    #[cfg(feature = "sandbox")]
    pub async fn sandbox_policy(&self) -> SandboxPolicy {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::GetSandboxPolicy { respond });
        rx.await.unwrap_or_default()
    }

    /// List registered tool names from the env actor.
    #[cfg(feature = "tool")]
    pub async fn tool_names(&self) -> Vec<String> {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::GetToolNames { respond });
        rx.await.unwrap_or_default()
    }

    // ── Env mutation proxy methods ────────────────────────────

    /// Change the LLM model name at runtime.
    ///
    /// Pushes to the internal watch channel (picked up by the ReAct loop on
    /// the next iteration) and broadcasts [`EnvStateEvent::LlmChanged`].
    pub fn set_model(&self, model: impl Into<String>) {
        let _ = self.env_cmd_tx.send(EnvCmd::SetModel(model.into()));
    }

    /// Change the LLM client at runtime (endpoint, key, etc.).
    pub fn set_client(&self, client: async_openai::Client<OpenAIConfig>) {
        let _ = self.env_cmd_tx.send(EnvCmd::SetClient(client));
    }

    /// Register a new tool at runtime.
    #[cfg(feature = "tool")]
    pub fn add_tool(&self, tool: Box<dyn Tool>) {
        let _ = self.env_cmd_tx.send(EnvCmd::AddTool(tool));
    }

    /// Remove a tool by name at runtime.
    #[cfg(feature = "tool")]
    pub fn remove_tool(&self, name: impl Into<String>) {
        let _ = self.env_cmd_tx.send(EnvCmd::RemoveTool(name.into()));
    }

    /// Set tool availability (enabled/disabled) at runtime.
    #[cfg(feature = "tool")]
    pub fn set_tool_availability(&self, name: impl Into<String>, available: bool) {
        let _ = self.env_cmd_tx.send(EnvCmd::SetToolAvailability {
            name: name.into(),
            available,
        });
    }

    /// Register a new skill at runtime.
    #[cfg(feature = "skill")]
    pub fn add_skill(&self, skill: Skill) {
        let _ = self.env_cmd_tx.send(EnvCmd::AddSkill(skill));
    }

    /// Remove a skill by name at runtime.
    #[cfg(feature = "skill")]
    pub fn remove_skill(&self, name: impl Into<String>) {
        let _ = self.env_cmd_tx.send(EnvCmd::RemoveSkill(name.into()));
    }

    /// Activate a skill by name at runtime. Returns `true` on success.
    #[cfg(feature = "skill")]
    pub async fn activate_skill(&self, name: impl Into<String>) -> bool {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::ActivateSkill {
            name: name.into(),
            respond,
        });
        rx.await.unwrap_or(false)
    }

    /// Deactivate a skill by name at runtime. Returns `true` on success.
    #[cfg(feature = "skill")]
    pub async fn deactivate_skill(&self, name: impl Into<String>) -> bool {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::DeactivateSkill {
            name: name.into(),
            respond,
        });
        rx.await.unwrap_or(false)
    }

    /// Set the skill system prompt at runtime.
    #[cfg(feature = "skill")]
    pub fn set_skill_prompt(&self, prompt: impl Into<String>) {
        let _ = self.env_cmd_tx.send(EnvCmd::SetSkillPrompt(prompt.into()));
    }

    /// Get the current skill system prompt.
    #[cfg(feature = "skill")]
    pub async fn skill_prompt(&self) -> String {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::GetSkillPrompt { respond });
        rx.await.unwrap_or_default()
    }

    // ── End env mutation methods ──────────────────────────────

    /// Access the middleware chain for event filtering.
    #[cfg(feature = "middleware")]
    pub fn middleware_chain(&self) -> Arc<MiddlewareChain<AgentEvent, ErrorsEnabled>> {
        self.middleware_chain.clone()
    }

    /// Approve or reject a pending tool call that is awaiting user approval.
    #[cfg(all(feature = "tool", feature = "security"))]
    pub async fn approve_tool_call(&self, call_id: &str, approved: bool) -> Result<(), String> {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::ApproveToolCall {
            call_id: call_id.to_string(),
            approved,
            respond,
        });
        rx.await.unwrap_or(Err("env actor died".into()))
    }

    /// Transform the runtime into `Acquired` state (internal use).
    pub(crate) fn into_acquired(self) -> AgentRuntime<P, Acquired> {
        AgentRuntime::<P, Acquired> {
            env_cmd_tx: self.env_cmd_tx,
            session_tx: self.session_tx,
            #[cfg(feature = "middleware")]
            middleware_chain: self.middleware_chain,
            _state: PhantomData,
            _phantom: PhantomData,
        }
    }

    /// Query the env actor for the bundle of resources needed by the ReAct loop.
    pub(crate) async fn get_react_config(&self) -> ReActConfig {
        let (respond, rx) = tokio::sync::oneshot::channel();
        let _ = self.env_cmd_tx.send(EnvCmd::GetReActConfig { respond });
        rx.await.expect("env actor died")
    }
}

// ── Acquired → Idle ─────────────────────────────────────────

impl<P: ChatProvider> AgentRuntime<P, Acquired> {
    pub(crate) fn into_idle(self) -> AgentRuntime<P, Idle> {
        AgentRuntime::<P, Idle> {
            env_cmd_tx: self.env_cmd_tx,
            session_tx: self.session_tx,
            #[cfg(feature = "middleware")]
            middleware_chain: self.middleware_chain,
            _state: PhantomData,
            _phantom: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use funera_core::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};

    // ── builder defaults ───────────────────────────────────────────

    #[test]
    fn builder_defaults() {
        let b = AgentRuntimeBuilder::new();
        assert_eq!(b.max_iterations, 10);
        assert_eq!(b.channel_buffer, 32);
        assert!(b.api_key.is_none());
        assert!(b.model.is_none());
    }

    #[cfg(feature = "tool")]
    mod tool_tests {
        use super::*;
        use funera_core::re_act::tool::ToolCallError;

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

        #[test]
        fn builder_defaults_tools_empty() {
            let b = AgentRuntimeBuilder::new();
            assert!(b.tools.is_empty());
        }

        #[test]
        fn builder_with_tool_instance() {
            let b = AgentRuntimeBuilder::new().with_tool_instance(Box::new(MockTool));
            assert_eq!(b.tools.len(), 1);
        }

        #[tokio::test]
        async fn build_with_tool_adds_to_registry() {
            let rt = AgentRuntimeBuilder::new()
                .api_key("sk-test")
                .model("x")
                .with_tool::<MockTool>()
                .build()
                .unwrap();
            let names = rt.tool_names().await;
            assert!(names.contains(&"mock_tool".to_string()));
        }

        #[tokio::test]
        async fn tool_names_empty_by_default() {
            let rt = AgentRuntimeBuilder::new()
                .api_key("sk-test")
                .model("x")
                .build()
                .unwrap();
            let names = rt.tool_names().await;
            assert!(names.is_empty());
        }
    }

    #[cfg(feature = "skill")]
    mod skill_tests {
        use super::*;

        #[test]
        fn builder_with_skill_inline() {
            let b = AgentRuntimeBuilder::new().with_skill("s1", "desc", "content");
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

    // ── build ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn build_with_explicit_key() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("test-model")
            .build()
            .expect("build should succeed with api_key");
        assert_eq!(rt.model().await, "test-model");
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
        assert_eq!(rt.model().await, "my-model");
    }

    #[tokio::test]
    async fn build_fails_without_key() {
        let has_key = std::env::var("OPENAI_API_KEY").is_ok();
        if has_key {
            return;
        }
        let result = AgentRuntimeBuilder::new().model("x").build();
        assert!(matches!(result, Err(OrchestrateError::Config(_))));
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
        assert_eq!(rt.model().await, "gpt-4o");
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
        let ctx = FuneraSession::new(rt.session_tx()).session_context().await;
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
            MsgVariant::Text(TextMessage {
                text: "hi".into(),
                reasoning_content: None,
            }),
        ));
        let ctx_before = session.session_context().await;
        assert_eq!(ctx_before.len(), 1);

        rt.reset();

        let ctx_after = session.session_context().await;
        assert_eq!(ctx_after.len(), 0);
    }

    #[tokio::test]
    async fn subscribe_env_state_works() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let mut rx = rt.subscribe_env_state().await;
        rt.set_model("new-model");
        let got = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await;
        assert!(matches!(
            got,
            Ok(Ok(EnvStateEvent::LlmChanged(m))) if m == "new-model"
        ));
    }

    // ── sandbox integration tests ───────────────────────────────────

    #[cfg(feature = "sandbox")]
    #[tokio::test]
    async fn builder_sandbox_policy_flows_to_env() {
        let custom_policy = SandboxPolicy {
            read_write_paths: vec!["/project".into()],
            block_network: true,
            ..Default::default()
        };

        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .with_sandbox_policy(custom_policy.clone())
            .build()
            .unwrap();

        let stored = rt.sandbox_policy().await;
        assert_eq!(stored.read_write_paths, custom_policy.read_write_paths);
        assert_eq!(stored.block_network, custom_policy.block_network);
        assert!(stored.enabled);
    }

    #[cfg(feature = "sandbox")]
    #[tokio::test]
    async fn builder_no_sandbox_uses_default() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .build()
            .unwrap();
        let stored = rt.sandbox_policy().await;
        assert!(stored.enabled);
        assert!(stored.block_network);
        assert!(stored.read_paths.is_empty());
        assert!(stored.read_write_paths.is_empty());
        assert!(stored.execute_paths.is_empty());
    }

    #[cfg(feature = "sandbox")]
    #[tokio::test]
    async fn builder_sandbox_with_custom_environments() {
        let rt = AgentRuntimeBuilder::new()
            .api_key("sk-test")
            .model("x")
            .with_sandbox_policy(SandboxPolicy::disabled())
            .build()
            .unwrap();

        let stored = rt.sandbox_policy().await;
        assert!(!stored.enabled, "disabled policy should stay disabled");
    }
}
