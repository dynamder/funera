use std::sync::Arc;

use anyhow::Result;
use funera_core::env::FuneraEnv;
use funera_core::event_bus::env_state_bus::EnvStateBus;
use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::ToolRegistry;
use funera_core::re_act::tool_executor::ToolExecutor;
use funera_core::provider::deepseek::DeepSeekProvider;
use funera_core::re_act::ReActLoop;
use crate::utils::env_config::default_model;
use crate::utils::fixtures::default_schema;
use crate::utils::mock_tool::MockTool;
use serde_json::json;
use tokio::sync::{broadcast, RwLock};

pub async fn multiple_tool_calls() -> Result<String> {
    let (_env_state_bus, turn_highway_handle) = EnvStateBus::new();
    let client = async_openai::Client::new();
    let mut registry = ToolRegistry::new();
    let tool_a = MockTool::new("tool_a", default_schema("tool_a"))
        .with_result(Ok("result_a".into()));
    let tool_b = MockTool::new("tool_b", default_schema("tool_b"))
        .with_result(Ok("result_b".into()));
    let tool_c = MockTool::new("tool_c", default_schema("tool_c"))
        .with_result(Ok("result_c".into()));
    registry.add_tool(Box::new(tool_a));
    registry.add_tool(Box::new(tool_b));
    registry.add_tool(Box::new(tool_c));

    let (_env, env_watcher) = FuneraEnv::new(registry, client, default_model());
    let (tool_bus, exec_rx) = ToolBus::new();

    let treg = Arc::new(RwLock::new(ToolRegistry::new()));
    {
        let mut reg = treg.write().await;
        reg.add_tool(Box::new(MockTool::new("tool_a", default_schema("tool_a"))));
        reg.add_tool(Box::new(MockTool::new("tool_b", default_schema("tool_b"))));
        reg.add_tool(Box::new(MockTool::new("tool_c", default_schema("tool_c"))));
    }
    tokio::spawn(ToolExecutor::new(treg, exec_rx).run());

    let (state_tx, _state_rx) = broadcast::channel(20);
    let session_msgs: Arc<parking_lot::RwLock<Vec<funera_core::chat::message::FuneraMessage>>> = Default::default();
    session_msgs.write().push(funera_core::chat::message::FuneraMessage::new(
        funera_core::chat::message::Role::User,
        funera_core::chat::message::MsgVariant::Text(funera_core::chat::message::TextMessage { text: "use multiple tools".into(), reasoning_content: None }),
    ));
    let loop_instance = ReActLoop::<DeepSeekProvider>::new(10, 3, session_msgs, env_watcher, tool_bus, state_tx, turn_highway_handle);
    let loop_handle = loop_instance.run();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    loop_handle.cancel_token.cancel();

    Ok("multiple_tool_calls completed successfully".into())
}

pub async fn tool_registry_operations() -> Result<String> {
    let mut registry = ToolRegistry::new();
    registry.add_tool(Box::new(MockTool::new("a", default_schema("a"))));
    registry.add_tool(Box::new(MockTool::new("b", default_schema("b"))));

    assert_eq!(registry.tool_count(), 2);
    assert!(registry.tool_exists("a"));
    assert!(registry.get_tool("a").unwrap().is_available());

    registry.remove_tool("a");
    assert_eq!(registry.tool_count(), 1);
    assert!(!registry.tool_exists("a"));

    let tools_json = registry.available_tools_json();
    assert_eq!(tools_json.as_array().unwrap().len(), 1);

    Ok("tool_registry_operations completed successfully".into())
}
