use std::sync::Arc;

use async_openai::config::OpenAIConfig;

use crate::re_act::skills::{Skill, SkillRegistry};
use crate::re_act::tool::{Tool, ToolRegistry};
use serde_json::Value as JsonValue;
use tokio::sync::{
    watch::{self, error::RecvError},
    RwLock,
};

pub struct FuneraEnv {
    pub tool_registry: Arc<RwLock<ToolRegistry>>,
    pub skill_registry: Arc<RwLock<SkillRegistry>>,
    llm_client: async_openai::Client<OpenAIConfig>,
    model: String,
    tool_tx: watch::Sender<JsonValue>,
    client_tx: watch::Sender<async_openai::Client<OpenAIConfig>>,
    model_tx: watch::Sender<String>,
    skill_tx: watch::Sender<String>,
}

impl FuneraEnv {
    pub fn new(
        tool_registry: ToolRegistry,
        llm_client: async_openai::Client<OpenAIConfig>,
        model: impl Into<String>,
    ) -> (Self, FuneraEnvWatcher) {
        Self::with_skills(tool_registry, llm_client, model, SkillRegistry::new())
    }

    pub fn with_skills(
        tool_registry: ToolRegistry,
        llm_client: async_openai::Client<OpenAIConfig>,
        model: impl Into<String>,
        skill_registry: SkillRegistry,
    ) -> (Self, FuneraEnvWatcher) {
        let tool_snapshot = tool_registry.available_tools_json();
        let tool_registry = Arc::new(RwLock::new(tool_registry));
        let (tool_tx, tool_rx) = watch::channel(tool_snapshot);
        let (client_tx, client_rx) = watch::channel(llm_client.clone());
        let model = model.into();
        let (model_tx, model_rx) = watch::channel(model.clone());
        let skill_prompt = skill_registry.get_active_skills_prompt();
        let (skill_tx, skill_rx) = watch::channel(skill_prompt);
        (
            Self {
                tool_registry,
                skill_registry: Arc::new(RwLock::new(skill_registry)),
                llm_client,
                model,
                tool_tx,
                client_tx,
                model_tx,
                skill_tx,
            },
            FuneraEnvWatcher {
                tool_rx,
                client_rx,
                model_rx,
                skill_rx,
            },
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

    pub fn set_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.model = model.clone();
        let _ = self.model_tx.send(model);
    }

    pub async fn add_skill(&mut self, skill: Skill) {
        let mut registry = self.skill_registry.write().await;
        registry.add(skill);
        let _ = self.skill_tx.send(registry.get_active_skills_prompt());
    }

    pub async fn remove_skill(&mut self, name: &str) {
        let mut registry = self.skill_registry.write().await;
        registry.remove(name);
        let _ = self.skill_tx.send(registry.get_active_skills_prompt());
    }

    pub async fn activate_skill(&mut self, name: &str) -> bool {
        let mut registry = self.skill_registry.write().await;
        let ok = registry.activate(name);
        if ok {
            let _ = self.skill_tx.send(registry.get_active_skills_prompt());
        }
        ok
    }

    pub async fn deactivate_skill(&mut self, name: &str) -> bool {
        let mut registry = self.skill_registry.write().await;
        let ok = registry.deactivate(name);
        if ok {
            let _ = self.skill_tx.send(registry.get_active_skills_prompt());
        }
        ok
    }

    pub fn skill_prompt_now(&self) -> String {
        // Best-effort snapshot of current skill prompt. Returns empty string if lock is poisoned.
        self.skill_tx.borrow().clone()
    }

    pub fn set_skill_prompt(&mut self, prompt: String) {
        let _ = self.skill_tx.send(prompt);
    }
}

#[derive(Debug, Clone)]
pub struct FuneraEnvWatcher {
    tool_rx: watch::Receiver<JsonValue>,
    client_rx: watch::Receiver<async_openai::Client<OpenAIConfig>>,
    model_rx: watch::Receiver<String>,
    skill_rx: watch::Receiver<String>,
}

impl FuneraEnvWatcher {
    pub fn watch_tool(&mut self) -> JsonValue {
        self.tool_rx.borrow_and_update().clone()
    }

    pub fn watch_client(&mut self) -> async_openai::Client<OpenAIConfig> {
        self.client_rx.borrow_and_update().clone()
    }

    pub fn watch_model(&mut self) -> String {
        self.model_rx.borrow_and_update().clone()
    }

    pub fn watch_skill(&mut self) -> String {
        self.skill_rx.borrow_and_update().clone()
    }

    pub fn has_tool_changed(&self) -> bool {
        self.tool_rx.has_changed().unwrap_or(false)
    }

    pub fn has_client_changed(&self) -> bool {
        self.client_rx.has_changed().unwrap_or(false)
    }

    pub fn has_model_changed(&self) -> bool {
        self.model_rx.has_changed().unwrap_or(false)
    }

    pub fn has_skill_changed(&self) -> bool {
        self.skill_rx.has_changed().unwrap_or(false)
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

    pub async fn model_changed(&mut self) -> Result<(), RecvError> {
        self.model_rx.changed().await
    }

    pub async fn skill_changed(&mut self) -> Result<(), RecvError> {
        self.skill_rx.changed().await
    }
}
