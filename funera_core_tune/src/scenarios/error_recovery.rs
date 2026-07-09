use std::sync::Arc;

use anyhow::Result;
use funera_core::env::FuneraEnv;
use funera_core::event_bus::env_state_bus::EnvStateBus;
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::{ToolCallError, ToolRegistry};
use funera_core::re_act::tool_executor::ToolExecutor;
use funera_core::re_act::ReActLoop;
use crate::utils::env_config::default_model;
use crate::utils::fixtures::{default_schema, err_tool};
use crate::utils::mock_tool::MockTool;
use serde_json::json;
use tokio::sync::{broadcast, RwLock};

pub async fn tool_execution_error() -> Result<String> {
    let (_env_state_bus, turn_highway_handle) = EnvStateBus::new();
    let client = async_openai::Client::new();
    let mut registry = ToolRegistry::new();
    registry.add_tool(Box::new(
        err_tool("faulty", ToolCallError::ToolExecutionError(anyhow::anyhow!("network error"))),
    ));
    let (_env, env_watcher) = FuneraEnv::new(registry, client, default_model());
    let (tool_bus, exec_rx) = ToolBus::new();

    let treg = Arc::new(RwLock::new(ToolRegistry::new()));
    {
        let mut reg = treg.write().await;
        reg.add_tool(Box::new(
            err_tool("faulty", ToolCallError::ToolExecutionError(anyhow::anyhow!("network error"))),
        ));
    }
    tokio::spawn(ToolExecutor::new(treg, exec_rx).run());

    let (state_tx, _state_rx) = broadcast::channel(20);
    let session_msgs: Arc<parking_lot::RwLock<Vec<funera_core::chat::message::FuneraMessage>>> = Default::default();
    session_msgs.write().push(funera_core::chat::message::FuneraMessage::new(
        funera_core::chat::message::Role::User,
        funera_core::chat::message::MsgVariant::Text(funera_core::chat::message::TextMessage { text: "call faulty tool".into() }),
    ));
    let loop_instance = ReActLoop::new(10, 2, session_msgs, env_watcher, tool_bus, state_tx, turn_highway_handle);
    let loop_handle = loop_instance.run();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    loop_handle.cancel_token.cancel();

    Ok("tool_execution_error completed successfully".into())
}

pub async fn tool_not_found_error() -> Result<String> {
    let (_env_state_bus, turn_highway_handle) = EnvStateBus::new();
    let client = async_openai::Client::new();
    let registry = ToolRegistry::new();
    let (_env, env_watcher) = FuneraEnv::new(registry, client, default_model());
    let (tool_bus, exec_rx) = ToolBus::new();

    let treg = Arc::new(RwLock::new(ToolRegistry::new()));
    tokio::spawn(ToolExecutor::new(treg, exec_rx).run());

    let (state_tx, _state_rx) = broadcast::channel(20);
    let history = vec![json!({"role": "user", "content": "use nonexistent tool"})];
    let session_msgs: Arc<parking_lot::RwLock<Vec<funera_core::chat::message::FuneraMessage>>> = Default::default();
    session_msgs.write().push(funera_core::chat::message::FuneraMessage::new(
        funera_core::chat::message::Role::User,
        funera_core::chat::message::MsgVariant::Text(funera_core::chat::message::TextMessage { text: "use nonexistent tool".into() }),
    ));
    let loop_instance = ReActLoop::new(10, 2, session_msgs, env_watcher, tool_bus, state_tx, turn_highway_handle);
    let loop_handle = loop_instance.run();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    loop_handle.cancel_token.cancel();

    Ok("tool_not_found_error completed successfully".into())
}

pub async fn env_watcher_tracks_changes() -> Result<String> {
    let registry = ToolRegistry::new();
    let client = async_openai::Client::new();
    let (mut env, mut watcher) = FuneraEnv::new(registry, client, default_model());

    assert!(!watcher.has_tool_changed());
    let tool = MockTool::new("ping", default_schema("ping"));
    env.add_tool(Box::new(tool)).await;
    assert!(watcher.has_tool_changed());

    let tools = watcher.watch_tool();
    assert_eq!(tools.as_array().unwrap().len(), 1);

    env.set_model("gpt-4");
    assert_eq!(watcher.watch_model(), "gpt-4");

    Ok("env_watcher_tracks_changes completed successfully".into())
}
