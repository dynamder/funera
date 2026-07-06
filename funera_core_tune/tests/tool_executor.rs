use std::sync::Arc;

use funera_core::event_bus::tool_bus::ToolBus;
use funera_core::re_act::tool::ToolRegistry;
use funera_core::re_act::tool_executor::ToolExecutor;
use funera_core_tune::utils::fixtures::default_schema;
use funera_core_tune::utils::mock_tool::MockTool;
use serde_json::json;
use tokio::sync::RwLock;

fn setup_executor() -> (Arc<RwLock<ToolRegistry>>, ToolBus, ToolExecutor) {
    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("echo", default_schema("echo"));
    registry.add_tool(Box::new(tool));
    let registry = Arc::new(RwLock::new(registry));
    let (bus, rx) = ToolBus::new();
    let executor = ToolExecutor::new(registry.clone(), rx);
    (registry, bus, executor)
}

#[tokio::test]
async fn executor_executes_tool() {
    let (_registry, bus, executor) = setup_executor();
    tokio::spawn(executor.run());
    let result = bus.execute("call_1".into(), "echo".into(), json!({"msg": "hi"})).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "mock_result");
}

#[tokio::test]
async fn executor_tool_not_found() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let (bus, rx) = ToolBus::new();
    let executor = ToolExecutor::new(registry, rx);
    tokio::spawn(executor.run());
    let result = bus.execute("call_1".into(), "nonexistent".into(), json!({})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn executor_handles_multiple_calls() {
    let (_registry, bus, executor) = setup_executor();

    let _handle = tokio::spawn(executor.run());

    let r1 = bus.execute("c1".into(), "echo".into(), json!({})).await;
    let r2 = bus.execute("c2".into(), "echo".into(), json!({})).await;
    let r3 = bus.execute("c3".into(), "echo".into(), json!({})).await;

    assert!(r1.is_ok());
    assert!(r2.is_ok());
    assert!(r3.is_ok());
}

#[tokio::test]
async fn executor_stops_when_dropped() {
    let (_registry, bus, executor) = setup_executor();
    drop(executor); // Executor dropped without being spawned
    let result = bus.execute("c1".into(), "echo".into(), json!({})).await;
    assert!(result.is_err());
}
