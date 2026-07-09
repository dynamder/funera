use std::sync::Arc;

use anyhow::Result;
use funera_core::chat::message::{FuneraMessage, MsgVariant, Role, TextMessage};
use funera_core::chat::session::{FuneraSession, Idle};
use funera_core::env::FuneraEnv;
use funera_core::event_bus::env_state_bus::EnvStateBus;
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::ToolRegistry;
use funera_core::re_act::tool_executor::ToolExecutor;
use funera_core::re_act::ReActLoop;
use crate::utils::env_config::default_model;
use crate::utils::fixtures::default_schema;
use crate::utils::mock_tool::MockTool;
use serde_json::json;
use tokio::sync::broadcast;
use tokio::sync::RwLock;

pub async fn multi_turn_conversation() -> Result<String> {
    let (env_state_bus, turn_highway_handle) = EnvStateBus::new();
    let mut registry = ToolRegistry::new();
    registry.add_tool(Box::new(
        MockTool::new("echo", default_schema("echo"))
            .with_description("Echoes back the input"),
    ));
    registry.add_tool(Box::new(
        MockTool::new("greet", default_schema("greet"))
            .with_description("Greets the user"),
    ));

    let client = async_openai::Client::new();
    let (_env, env_watcher) = FuneraEnv::new(registry, client, default_model());
    let (tool_bus, exec_rx) = ToolBus::new();

    let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let mut reg = tool_registry.write().await;
    reg.add_tool(Box::new(MockTool::new("echo", default_schema("echo"))));
    reg.add_tool(Box::new(MockTool::new("greet", default_schema("greet"))));
    drop(reg);
    tokio::spawn(ToolExecutor::new(tool_registry, exec_rx).run());

    let (state_tx, _state_rx) = broadcast::channel(20);
    let session_msgs: Arc<parking_lot::RwLock<Vec<FuneraMessage>>> = Default::default();
    {
        let mut msgs = session_msgs.write();
        msgs.push(FuneraMessage::new(Role::System, MsgVariant::Text(TextMessage { text: "You are a helpful assistant.".into() })));
        msgs.push(FuneraMessage::new(Role::User, MsgVariant::Text(TextMessage { text: "Hello!".into() })));
    }
    let loop_instance = ReActLoop::new(10, 3, session_msgs, env_watcher, tool_bus, state_tx, turn_highway_handle);
    let sender = loop_instance.sender();
    sender
        .send(FuneraMessage::new(
            Role::User,
            MsgVariant::Text(TextMessage {
                text: "What can you do?".into(),
            }),
        ))
        .await?;

    let handle = loop_instance.run();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    handle.cancel_token.cancel();
    drop(env_state_bus);

    Ok("multi_turn_conversation completed successfully".into())
}

pub async fn session_state_transitions() -> Result<String> {
    let session = FuneraSession::<Idle>::new();
    let id = session.id();
    let running = session.run();
    assert_eq!(running.id(), id);
    let idle = running.idle();
    assert_eq!(idle.id(), id);
    let _running2 = idle.run();
    Ok("session_state_transitions completed successfully".into())
}
