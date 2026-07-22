use std::sync::Arc;

use async_openai::config::OpenAIConfig;

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

pub struct FuneraEnv {
    #[cfg(feature = "tool")]
    pub tool_registry: Arc<RwLock<ToolRegistry>>,
    #[cfg(feature = "skill")]
    pub skill_registry: Arc<RwLock<SkillRegistry>>,
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

    /// Access the current sandbox policy.
    #[cfg(feature = "sandbox")]
    pub fn sandbox_policy(&self) -> &SandboxPolicy {
        &self.sandbox_policy
    }

    /// Set a custom sandbox policy.
    #[cfg(feature = "sandbox")]
    pub fn with_sandbox_policy(mut self, policy: SandboxPolicy) -> Self {
        self.sandbox_policy = policy;
        self
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
    pub async fn add_tool(&mut self, tool: Box<dyn Tool>) {
        let mut registry = self.tool_registry.write().await;
        registry.add_tool(tool);
        let _ = self.tool_tx.send(registry.available_tools_json());
    }

    #[cfg(feature = "tool")]
    pub async fn remove_tool(&mut self, name: &str) {
        let mut registry = self.tool_registry.write().await;
        registry.remove_tool(name);
        let _ = self.tool_tx.send(registry.available_tools_json());
    }

    #[cfg(feature = "tool")]
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

    #[cfg(feature = "skill")]
    pub async fn add_skill(&mut self, skill: Skill) {
        let mut registry = self.skill_registry.write().await;
        registry.add(skill);
        let _ = self.skill_tx.send(registry.get_active_skills_prompt());
    }

    #[cfg(feature = "skill")]
    pub async fn remove_skill(&mut self, name: &str) {
        let mut registry = self.skill_registry.write().await;
        registry.remove(name);
        let _ = self.skill_tx.send(registry.get_active_skills_prompt());
    }

    #[cfg(feature = "skill")]
    pub async fn activate_skill(&mut self, name: &str) -> bool {
        let mut registry = self.skill_registry.write().await;
        let ok = registry.activate(name);
        if ok {
            let _ = self.skill_tx.send(registry.get_active_skills_prompt());
        }
        ok
    }

    #[cfg(feature = "skill")]
    pub async fn deactivate_skill(&mut self, name: &str) -> bool {
        let mut registry = self.skill_registry.write().await;
        let ok = registry.deactivate(name);
        if ok {
            let _ = self.skill_tx.send(registry.get_active_skills_prompt());
        }
        ok
    }

    #[cfg(feature = "skill")]
    pub fn skill_prompt_now(&self) -> String {
        self.skill_tx.borrow().clone()
    }

    #[cfg(feature = "skill")]
    pub fn set_skill_prompt(&mut self, prompt: String) {
        let _ = self.skill_tx.send(prompt);
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
