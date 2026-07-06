use funera_core::re_act::tool::{Tool, ToolCallError, ToolRegistry, ToolType};
use funera_core_tune::utils::mock_tool::MockTool;
use funera_core_tune::utils::fixtures::{create_registry_with_tools, default_schema, err_tool};
use serde_json::json;

#[test]
fn tool_type_display() {
    assert_eq!(ToolType::Function.to_string(), "function");
}

#[test]
fn tool_type_serde() {
    let json = serde_json::to_string(&ToolType::Function).unwrap();
    assert_eq!(json, "\"Function\"");
    let deserialized: ToolType = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, ToolType::Function);
}

#[test]
fn tool_registry_new_is_empty() {
    let registry = ToolRegistry::new();
    assert_eq!(registry.tool_count(), 0);
}

#[test]
fn tool_registry_add_and_get() {
    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("test_tool", default_schema("test_tool"));
    registry.add_tool(Box::new(tool));
    assert_eq!(registry.tool_count(), 1);
    assert!(registry.tool_exists("test_tool"));
    assert!(registry.get_tool("test_tool").is_some());
}

#[test]
fn tool_registry_remove() {
    let mut registry = create_registry_with_tools(&["a", "b", "c"]);
    assert_eq!(registry.tool_count(), 3);
    registry.remove_tool("b");
    assert_eq!(registry.tool_count(), 2);
    assert!(!registry.tool_exists("b"));
    assert!(registry.tool_exists("a"));
    assert!(registry.tool_exists("c"));
}

#[test]
fn tool_registry_remove_nonexistent() {
    let mut registry = ToolRegistry::new();
    registry.remove_tool("nonexistent");
    assert_eq!(registry.tool_count(), 0);
}

#[test]
fn tool_registry_get_nonexistent() {
    let registry = ToolRegistry::new();
    assert!(registry.get_tool("nonexistent").is_none());
}

#[test]
fn tool_registry_default_available() {
    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("my_tool", default_schema("my_tool"));
    registry.add_tool(Box::new(tool));
    let entry = registry.get_tool("my_tool").unwrap();
    assert!(entry.is_available());
}

#[test]
fn tool_registry_available_tools_json() {
    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("active", default_schema("active"));
    registry.add_tool(Box::new(tool));
    let json = registry.available_tools_json();
    assert_eq!(json.as_array().unwrap().len(), 1);
}

#[test]
fn tool_registry_all_tools() {
    let registry = create_registry_with_tools(&["x", "y"]);
    let all = registry.get_all_tools();
    assert_eq!(all.len(), 2);
    assert!(all.contains_key("x"));
    assert!(all.contains_key("y"));
}

#[test]
fn tool_call_error_parameter_mismatch() {
    let err = ToolCallError::ParameterMismatch(json!({"missing": "field"}));
    let msg = format!("{}", err);
    assert!(msg.contains("parameter mismatch"));
}

#[test]
fn tool_call_error_execution() {
    let err = ToolCallError::ToolExecutionError(anyhow::anyhow!("something went wrong"));
    let msg = format!("{}", err);
    assert!(msg.contains("tool execution error"));
    assert!(msg.contains("something went wrong"));
}

#[test]
fn tool_call_error_unavailable() {
    let err = ToolCallError::ToolUnavailable("weather".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("tool unavailable"));
    assert!(msg.contains("weather"));
}

#[test]
fn tool_call_error_not_found() {
    let err = ToolCallError::ToolNotFound("missing_tool".to_string());
    let msg = format!("{}", err);
    assert!(msg.contains("tool not found"));
    assert!(msg.contains("missing_tool"));
}

#[tokio::test]
async fn call_tool_success() {
    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("calculator", default_schema("calculator"));
    registry.add_tool(Box::new(tool));
    let result = registry.call_tool("calculator", json!({"a": 1})).await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "mock_result");
}

#[tokio::test]
async fn call_tool_not_found() {
    let registry = ToolRegistry::new();
    let result = registry.call_tool("ghost", json!({})).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        ToolCallError::ToolNotFound(name) => assert_eq!(name, "ghost"),
        _ => panic!("expected ToolNotFound"),
    }
}

#[tokio::test]
async fn call_tool_execution_error() {
    let mut registry = ToolRegistry::new();
    let tool = err_tool("faulty", ToolCallError::ToolExecutionError(anyhow::anyhow!("oops")));
    registry.add_tool(Box::new(tool));
    let result = registry.call_tool("faulty", json!({})).await;
    assert!(result.is_err());
}

#[test]
fn tool_trait_methods() {
    let tool = MockTool::new("greeter", default_schema("greeter"))
        .with_description("A greeter tool");
    assert_eq!(tool.name(), "greeter");
    assert_eq!(tool.description(), "A greeter tool");
    assert_eq!(tool.get_type(), ToolType::Function);
    assert_eq!(tool.schema(), default_schema("greeter"));
}
