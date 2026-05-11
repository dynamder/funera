use std::sync::{Arc, Weak};

use serde::{Deserialize, Serialize};

use crate::{
    chat::session::FuneraSession,
    env::{FuneraEnv, FuneraEnvWatcher},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentConfig {
    pub max_loop_iterations: usize,
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_loop_iterations: 100,
        }
    }
}

pub enum AgentState {
    Idle,
    Running,
}
pub enum AgentMode {
    Plan,
    Agent,
    Yolo,
}

pub struct FuneraAgent {
    config: AgentConfig,
    state: AgentState,
    mode: AgentMode,
}

impl FuneraAgent {
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            state: AgentState::Idle,
            mode: AgentMode::Agent,
        }
    }
    pub fn set_config(&mut self, config: AgentConfig) {
        self.config = config;
    }

    pub fn react_loop(&mut self, session: FuneraSession, env_watcher: FuneraEnvWatcher) {
        for _ in 0..self.config.max_loop_iterations {
            let messages = session.session_context();
        }
    }
}
