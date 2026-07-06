use std::sync::Arc;

use async_openai::config::OpenAIConfig;

use crate::re_act::tool::{Tool, ToolRegistry};
use serde_json::Value as JsonValue;
use tokio::sync::{
    watch::{self, error::RecvError},
    RwLock,
};

pub struct FuneraEnv {
    pub tool_registry: Arc<RwLock<ToolRegistry>>,
    llm_client: async_openai::Client<OpenAIConfig>,
    tool_tx: watch::Sender<JsonValue>,
    client_tx: watch::Sender<async_openai::Client<OpenAIConfig>>,
}

impl FuneraEnv {
    pub fn new(
        tool_registry: ToolRegistry,
        llm_client: async_openai::Client<OpenAIConfig>,
    ) -> (Self, FuneraEnvWatcher) {
        let tool_snapshot = tool_registry.available_tools_json();
        let tool_registry = Arc::new(RwLock::new(tool_registry));
        let (tool_tx, tool_rx) = watch::channel(tool_snapshot);
        let (client_tx, client_rx) = watch::channel(llm_client.clone());
        (
            Self {
                tool_registry,
                llm_client,
                tool_tx,
                client_tx,
            },
            FuneraEnvWatcher { tool_rx, client_rx },
        )
    }

    pub async fn add_tool(&mut self, tool: Box<dyn Tool>) {
        let mut registry = self.tool_registry.write().await;
        registry.add_tool(tool);
        let _ = self.tool_tx.send(registry.available_tools_json());
    }

    pub async fn remove_tool(&mut self, name: &str) {
        let mut registry = self.tool_registry.write().await;
        registry.remove_tool(name);
        let _ = self.tool_tx.send(registry.available_tools_json());
    }

    pub async fn set_tool_availability(&mut self, _name: &str, _available: bool) {
        let registry = self.tool_registry.read().await;
        let _ = self.tool_tx.send(registry.available_tools_json());
    }

    pub fn set_client(&mut self, client: async_openai::Client<OpenAIConfig>) {
        self.llm_client = client.clone();
        let _ = self.client_tx.send(client);
    }
}

#[derive(Debug, Clone)]
pub struct FuneraEnvWatcher {
    tool_rx: watch::Receiver<JsonValue>,
    client_rx: watch::Receiver<async_openai::Client<OpenAIConfig>>,
}

impl FuneraEnvWatcher {
    pub fn watch_tool(&mut self) -> JsonValue {
        self.tool_rx.borrow_and_update().clone()
    }

    pub fn watch_client(&mut self) -> async_openai::Client<OpenAIConfig> {
        self.client_rx.borrow_and_update().clone()
    }

    pub fn has_tool_changed(&self) -> bool {
        self.tool_rx.has_changed().unwrap_or(false)
    }

    pub fn has_client_changed(&self) -> bool {
        self.client_rx.has_changed().unwrap_or(false)
    }

    pub fn use_client(&mut self) -> async_openai::Client<OpenAIConfig> {
        self.watch_client()
    }

    pub async fn tool_changed(&mut self) -> Result<(), RecvError> {
        self.tool_rx.changed().await
    }

    pub async fn client_changed(&mut self) -> Result<(), RecvError> {
        self.client_rx.changed().await
    }
}
