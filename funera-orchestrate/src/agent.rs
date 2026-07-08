use std::sync::Arc;

use async_openai::config::OpenAIConfig;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use funera_core::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};
use funera_core::chat::session::{FuneraSession, Idle};
use funera_core::env::FuneraEnv;
use funera_core::event_bus::env_state_bus::{EnvStateBus, EnvStateEvent};
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::{Tool, ToolRegistry};
use funera_core::re_act::tool_executor::ToolExecutor;

use crate::dispatcher::{CallbackDispatcher, CallbackRegistry};
use crate::error::OrchestrateError;
use crate::event::AgentEvent;
use crate::response::ChatResponse;

pub struct AgentBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
    client: Option<async_openai::Client<OpenAIConfig>>,
    model: Option<String>,
    max_iterations: usize,
    channel_buffer: usize,
    system_prompt: Option<String>,
    tools: Vec<Box<dyn Tool>>,
    callbacks: CallbackRegistry,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBuilder {
    pub fn new() -> Self {
        Self {
            api_key: None,
            base_url: None,
            client: None,
            model: None,
            max_iterations: 10,
            channel_buffer: 32,
            system_prompt: None,
            tools: Vec::new(),
            callbacks: CallbackRegistry::new(),
        }
    }

    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn client(mut self, client: async_openai::Client<OpenAIConfig>) -> Self {
        self.client = Some(client);
        self
    }

    pub fn max_iterations(mut self, n: usize) -> Self {
        self.max_iterations = n;
        self
    }

    pub fn channel_buffer(mut self, n: usize) -> Self {
        self.channel_buffer = n;
        self
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn with_tool<T: Tool + 'static>(mut self) -> Self
    where
        T: Default,
    {
        let tool: T = T::default();
        self.tools.push(Box::new(tool));
        self
    }

    pub fn with_tool_instance(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    #[cfg(feature = "builtin-tools")]
    pub fn with_builtin_tools(mut self) -> Self {
        use builtin_tools::{EditTool, ReadTool, ShellTool, WriteTool};
        self.tools.push(Box::new(ReadTool));
        self.tools.push(Box::new(WriteTool));
        self.tools.push(Box::new(EditTool));
        self.tools.push(Box::new(ShellTool));
        self
    }

    pub fn on_token<F>(mut self, f: F) -> Self
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if let AgentEvent::Token(t) = event {
                f(t);
            }
        }));
        self
    }

    pub fn on_tool_call<F>(mut self, f: F) -> Self
    where
        F: Fn(String, serde_json::Value) + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if let AgentEvent::ToolCallStart { name, args, .. } = event {
                f(name, args);
            }
        }));
        self
    }

    pub fn on_tool_result<F>(mut self, f: F) -> Self
    where
        F: Fn(String, Result<String, String>) + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if let AgentEvent::ToolCallResult { name, result, .. } = event {
                f(name, result);
            }
        }));
        self
    }

    pub fn on_turn_start<F>(mut self, f: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if matches!(event, AgentEvent::TurnStart) {
                f();
            }
        }));
        self
    }

    pub fn on_turn_end<F>(mut self, f: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(move |event| {
            if matches!(event, AgentEvent::TurnEnd) {
                f();
            }
        }));
        self
    }

    pub fn on_event<F>(mut self, f: F) -> Self
    where
        F: Fn(AgentEvent) + Send + Sync + 'static,
    {
        self.callbacks.add(Arc::new(f));
        self
    }

    pub fn build(self) -> Result<Agent, OrchestrateError> {
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
                let api_key = api_key.ok_or_else(|| {
                    OrchestrateError::Config(
                        "no API key provided; set OPENAI_API_KEY or call .api_key()".into(),
                    )
                })?;

                let mut config = OpenAIConfig::default().with_api_key(api_key);

                if let Some(base_url) = &self.base_url {
                    config = config.with_api_base(base_url);
                }

                async_openai::Client::with_config(config)
            }
        };

        let mut registry = ToolRegistry::new();
        for tool in self.tools {
            registry.add_tool(tool);
        }

        let (env, env_watcher) = FuneraEnv::new(registry, client.clone(), model.clone());
        let (env_state_bus, turn_highway_handle) = EnvStateBus::new();
        let env_state_tx = env_state_bus.env_state_tx.clone();
        let env_state_rx = env_state_bus.subscribe();
        env_state_bus.start_turn_highway();

        let (tool_bus, exec_rx) = ToolBus::new();
        let tool_registry = env.tool_registry.clone();

        let executor_handle = tokio::spawn(async move {
            ToolExecutor::new(tool_registry, exec_rx).run().await;
        });

        let callbacks = Arc::new(self.callbacks);
        let (event_tx, _) = broadcast::channel(256);

        let _dispatcher =
            CallbackDispatcher::new(env_state_rx, callbacks.clone(), event_tx.clone());

        let session = FuneraSession::<Idle>::new();
        let system_prompt = self.system_prompt;

        Ok(Agent {
            env: Some(env),
            env_watcher: Some(env_watcher),
            env_state_tx,
            turn_highway_handle: Some(turn_highway_handle),
            tool_bus,
            _executor_handle: executor_handle,
            callbacks,
            event_tx,
            session: Some(session),
            session_msg_count: 0,
            max_iterations: self.max_iterations,
            channel_buffer: self.channel_buffer,
            system_prompt,
            model,
        })
    }
}

pub struct Agent {
    env: Option<FuneraEnv>,
    env_watcher: Option<funera_core::env::FuneraEnvWatcher>,
    env_state_tx: broadcast::Sender<EnvStateEvent>,
    turn_highway_handle: Option<funera_core::event_bus::env_state_bus::TurnHighWayHandle>,
    tool_bus: ToolBus,
    _executor_handle: JoinHandle<()>,
    callbacks: Arc<CallbackRegistry>,
    event_tx: broadcast::Sender<AgentEvent>,
    session: Option<FuneraSession<Idle>>,
    session_msg_count: usize,
    max_iterations: usize,
    channel_buffer: usize,
    system_prompt: Option<String>,
    model: String,
}

impl Agent {
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.event_tx.subscribe()
    }

    pub fn callbacks(&self) -> Arc<CallbackRegistry> {
        self.callbacks.clone()
    }

    /// Send a message and wait for the full response.
    /// This is a single-turn call; previous history is NOT retained.
    pub async fn chat(&mut self, msg: impl Into<String>) -> Result<ChatResponse, OrchestrateError> {
        self.run_session(msg, false).await
    }

    /// Send a message and wait for the full response.
    /// Unlike `chat`, history from previous `send` calls IS retained,
    /// enabling natural multi-turn conversations.
    pub async fn send(&mut self, msg: impl Into<String>) -> Result<ChatResponse, OrchestrateError> {
        self.run_session(msg, true).await
    }

    pub async fn chat_stream(
        &mut self,
        msg: impl Into<String>,
    ) -> Result<mpsc::Receiver<AgentEvent>, OrchestrateError> {
        let (tx, rx) = mpsc::channel(256);
        let event_tx = self.event_tx.clone();
        let guard_tx = tx.clone();

        let event_rx = event_tx.subscribe();
        tokio::spawn(async move {
            let mut rx = event_rx;
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if guard_tx.send(event.clone()).await.is_err() {
                            break;
                        }
                        if matches!(event, AgentEvent::Done) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let _ = self.run_session(msg, false).await?;
        let _ = tx.send(AgentEvent::Done).await;

        Ok(rx)
    }

    pub fn reset(&mut self) {
        self.session = Some(FuneraSession::<Idle>::new());
        self.session_msg_count = 0;
    }

    async fn run_session(
        &mut self,
        msg: impl Into<String>,
        retain_history: bool,
    ) -> Result<ChatResponse, OrchestrateError> {
        let text = msg.into();

        let _ = self.env_state_tx.send(EnvStateEvent::SessionStart);

        let session = self.session.take().unwrap_or_else(|| {
            self.session_msg_count = 0;
            FuneraSession::<Idle>::new()
        });
        let mut running = session.run();

        let init_msg = FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage {
                text: text.clone().into(),
            }),
        );

        let mut history_msg: Vec<serde_json::Value> = Vec::new();

        if let Some(ref system_prompt) = self.system_prompt {
            history_msg.push(serde_json::json!({
                "role": "system",
                "content": system_prompt,
            }));
        }

        let env_watcher = self.env_watcher.take().unwrap_or_else(|| {
            let (env, watcher) = self.rebuild_env();
            self.env = Some(env);
            watcher
        });

        let turn_highway_handle = self.turn_highway_handle.take().unwrap();

        let config = funera_core::re_act::ReActLoopConfig::new(
            self.channel_buffer,
            self.max_iterations,
            env_watcher,
            self.tool_bus.clone(),
            self.env_state_tx.clone(),
            turn_highway_handle,
        );

        let result = running
            .react_loop(init_msg, config, self.env_state_tx.clone())
            .await;

        let idle_session = running.idle();

        match result {
            Ok(()) => {
                let ctx = idle_session.session_context();
                let assistant_msgs: Vec<&serde_json::Value> =
                    ctx.iter().filter(|m| m["role"] == "assistant").collect();

                let mut content = String::new();
                let tool_calls = Vec::new();
                let mut iterations = 0;

                for msg in &assistant_msgs {
                    if let Some(c) = msg["content"].as_str() {
                        if !c.is_empty() {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(c);
                        }
                    }
                    iterations += 1;
                }

                let finish_reason = assistant_msgs
                    .last()
                    .and_then(|m| m["finish_reason"].as_str())
                    .map(|s| s.to_string());

                if retain_history {
                    self.session = Some(idle_session);
                    self.session_msg_count += 1;
                } else {
                    self.session = Some(FuneraSession::<Idle>::new());
                    self.session_msg_count = 0;
                }

                let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);

                Ok(ChatResponse {
                    content,
                    tool_calls,
                    iterations,
                    finish_reason,
                })
            }
            Err(e) => {
                self.session = Some(if retain_history {
                    idle_session
                } else {
                    FuneraSession::<Idle>::new()
                });
                let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);
                Err(OrchestrateError::Session(e))
            }
        }
    }

    fn rebuild_env(&self) -> (FuneraEnv, funera_core::env::FuneraEnvWatcher) {
        let config = OpenAIConfig::default();
        let client = async_openai::Client::with_config(config);
        FuneraEnv::new(ToolRegistry::new(), client, self.model.clone())
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.env_state_tx.send(EnvStateEvent::SessionClosed);
    }
}
