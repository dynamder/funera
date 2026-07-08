use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinHandle;

use funera_core::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};
use funera_core::chat::session::{FuneraSession, Idle};
use funera_core::env::{FuneraEnv, FuneraEnvWatcher};
use funera_core::event_bus::env_state_bus::{EnvStateBus, EnvStateEvent};
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::{Tool, ToolRegistry};
use funera_core::re_act::tool_executor::ToolExecutor;
use funera_core::re_act::ReActLoopConfig;

use crate::error::OrchestrateError;
use crate::response::ChatResponse;

pub struct OrchestratorConfig {
    pub tool_registry: ToolRegistry,
    pub llm_client: async_openai::Client<async_openai::config::OpenAIConfig>,
    pub model: String,
    pub env_state_buffer: usize,
    pub tool_bus_buffer: usize,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            tool_registry: ToolRegistry::new(),
            llm_client: async_openai::Client::with_config(
                async_openai::config::OpenAIConfig::default(),
            ),
            model: "gpt-4o".into(),
            env_state_buffer: 20,
            tool_bus_buffer: 10,
        }
    }
}

pub struct Orchestrator {
    pub env: FuneraEnv,
    pub env_watcher: FuneraEnvWatcher,
    pub env_state_tx: broadcast::Sender<EnvStateEvent>,
    pub tool_registry: Arc<RwLock<ToolRegistry>>,
    tool_bus: ToolBus,
    _executor_handle: JoinHandle<()>,
}

impl Orchestrator {
    pub fn new(config: OrchestratorConfig) -> Self {
        let (env, env_watcher) =
            FuneraEnv::new(config.tool_registry, config.llm_client, config.model);

        let (env_state_bus, _turn_highway_handle) = EnvStateBus::new();
        let env_state_tx = env_state_bus.env_state_tx.clone();
        env_state_bus.start_turn_highway();

        let (tool_bus, exec_rx) = ToolBus::new();
        let tool_registry = env.tool_registry.clone();
        let reg = tool_registry.clone();

        let executor_handle = tokio::spawn(async move {
            ToolExecutor::new(reg, exec_rx).run().await;
        });

        Self {
            env,
            env_watcher,
            env_state_tx,
            tool_registry,
            tool_bus,
            _executor_handle: executor_handle,
        }
    }

    pub fn subscribe_env_state(&self) -> broadcast::Receiver<EnvStateEvent> {
        self.env_state_tx.subscribe()
    }

    /// Create a fresh session config for each invocation.
    /// Each call allocates a new TurnHighWayHandle pair.
    pub fn create_session_config(
        &self,
        buffer: usize,
        max_iterations: usize,
    ) -> (ReActLoopConfig, FuneraSession<Idle>) {
        let (_env_state_bus, turn_highway_handle) = EnvStateBus::new();
        let session = FuneraSession::<Idle>::new();

        let config = ReActLoopConfig::new(
            buffer,
            max_iterations,
            self.env_watcher.clone(),
            self.tool_bus.clone(),
            self.env_state_tx.clone(),
            turn_highway_handle,
        );

        (config, session)
    }

    /// Run a single-turn conversation via a fresh session.
    pub async fn run(
        &self,
        msg: impl Into<String>,
        max_iterations: usize,
    ) -> Result<ChatResponse, OrchestrateError> {
        let (config, session) = self.create_session_config(32, max_iterations);
        let mut running = session.run();

        let text = msg.into();
        let init_msg = FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage {
                text: text.into(),
            }),
        );

        let _ = self.env_state_tx.send(EnvStateEvent::SessionStart);
        running
            .react_loop(init_msg, config, self.env_state_tx.clone())
            .await?;
        let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);

        let ctx = running.idle().session_context();
        let assistant_msgs: Vec<&serde_json::Value> = ctx
            .iter()
            .filter(|m| m["role"] == "assistant")
            .collect();

        let mut content = String::new();
        for msg in &assistant_msgs {
            if let Some(c) = msg["content"].as_str() {
                if !c.is_empty() {
                    if !content.is_empty() {
                        content.push('\n');
                    }
                    content.push_str(c);
                }
            }
        }

        Ok(ChatResponse {
            content,
            tool_calls: Vec::new(),
            iterations: assistant_msgs.len(),
            finish_reason: None,
        })
    }

    pub fn add_tool(&mut self, tool: Box<dyn Tool>) {
        let tool_name = tool.name().to_string();
        let registry = self.env.tool_registry.clone();
        tokio::spawn(async move {
            registry.write().await.add_tool(tool);
        });
        let _ = self
            .env_state_tx
            .send(EnvStateEvent::ToolAdded(tool_name));
    }

    pub fn remove_tool(&self, name: &str) {
        let n = name.to_string();
        let registry = self.env.tool_registry.clone();
        tokio::spawn(async move {
            registry.write().await.remove_tool(&n);
        });
        let _ = self
            .env_state_tx
            .send(EnvStateEvent::ToolRemoved(name.to_string()));
    }
}
