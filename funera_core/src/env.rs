use async_openai::config::OpenAIConfig;
use secrecy::SecretString;

use crate::re_act::tool::ToolRegistry;
use serde_json::Value as JsonValue;
use tokio::sync::watch::{self, error::RecvError};

pub struct FuneraEnv {
    tool_registry: ToolRegistry,
    llm_client: async_openai::Client<OpenAIConfig>,
    tool_tx: watch::Sender<JsonValue>,
    client_tx: watch::Sender<async_openai::Client<OpenAIConfig>>,
}

impl FuneraEnv {
    pub fn new(
        tool_registry: ToolRegistry,
        llm_client: async_openai::Client<OpenAIConfig>,
    ) -> (Self, FuneraEnvWatcher) {
        let (tool_tx, tool_rx) = watch::channel(tool_registry.available_tools_json());
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

    pub fn use_client(&mut self) -> async_openai::Client<OpenAIConfig> {
        let client = self.client_rx.borrow_and_update().clone();
        todo!(
            "create token and msg channel and wire them up correctly, by sending request to FuneraEnv."
        )
    }

    pub async fn tool_changed(&mut self) -> Result<(), RecvError> {
        self.tool_rx.changed().await
    }

    pub async fn client_changed(&mut self) -> Result<(), RecvError> {
        self.client_rx.changed().await
    }
}
