use std::sync::Arc;

use funera_core::{
    env::FuneraEnv, event_bus::env_state_bus::EnvStateBus, re_act::tool::ToolRegistry,
};
use tokio::sync::RwLock;

use crate::utils::env_config::default_model;

pub struct TestHarness {
    pub env: FuneraEnv,
    pub env_watcher: funera_core::env::FuneraEnvWatcher,
    pub env_state_bus: EnvStateBus,
    pub tool_registry: Arc<RwLock<ToolRegistry>>,
    pub _turn_highway_handle: funera_core::event_bus::env_state_bus::TurnHighWayHandle,
}

impl TestHarness {
    pub async fn new() -> Self {
        let client = async_openai::Client::new();
        let model = default_model();
        let (env_state_bus, turn_highway_handle) = EnvStateBus::new();

        let tool_registry = ToolRegistry::new();
        let (env, env_watcher) = FuneraEnv::new(tool_registry, client, model);

        Self {
            env,
            env_watcher,
            env_state_bus,
            tool_registry: Arc::new(RwLock::new(ToolRegistry::new())),
            _turn_highway_handle: turn_highway_handle,
        }
    }

    pub fn default() -> Self {
        Self::new_sync()
    }

    fn new_sync() -> Self {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(Self::new())
    }
}

impl Default for TestHarness {
    fn default() -> Self {
        Self::new_sync()
    }
}
