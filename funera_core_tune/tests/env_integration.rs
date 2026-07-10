use std::sync::Arc;

use funera_core::env::FuneraEnv;
use funera_core::re_act::tool::{ToolCallError, ToolRegistry};
use funera_core_tune::utils::fixtures::{create_client, default_schema};
use funera_core_tune::utils::mock_tool::MockTool;
use serde_json::json;
use tokio::sync::RwLock;

fn default_model() -> String {
    "gpt-4o-mini".to_string()
}

// ── Watcher async change detection ──────────────────────────────

#[tokio::test]
async fn env_watcher_client_changed_async() {
    let registry = ToolRegistry::new();
    let client = create_client();
    let (mut env, mut watcher) = FuneraEnv::new(registry, client.clone(), default_model());

    let new_client = create_client();
    env.set_client(new_client);

    let result = watcher.client_changed().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn env_watcher_model_changed_async() {
    let registry = ToolRegistry::new();
    let client = create_client();
    let (mut env, mut watcher) = FuneraEnv::new(registry, client, default_model());

    env.set_model("gpt-4-turbo");

    let result = watcher.model_changed().await;
    assert!(result.is_ok());
    assert_eq!(watcher.watch_model(), "gpt-4-turbo");
}

#[tokio::test]
async fn env_watcher_use_client_returns_current() {
    let registry = ToolRegistry::new();
    let client = create_client();
    let (_, mut watcher) = FuneraEnv::new(registry, client.clone(), default_model());

    let watched = watcher.use_client();
    let _ = watched;
}

// ── Tool availability states ────────────────────────────────────

#[test]
fn tool_registry_entry_new_unavailable() {
    use funera_core::re_act::tool::ToolRegistryEntry;
    let tool = MockTool::new("disabled", default_schema("disabled"));
    let entry = ToolRegistryEntry::new_unavailable(Box::new(tool));

    assert!(!entry.is_available());
}

#[test]
fn tool_registry_default_available_json() {
    let mut registry = ToolRegistry::new();

    let available = MockTool::new("active", default_schema("active"));
    registry.add_tool(Box::new(available));

    let disabled = MockTool::new("disabled", default_schema("disabled"));
    registry.add_tool(Box::new(disabled));

    let json = registry.available_tools_json();
    assert_eq!(json.as_array().unwrap().len(), 2);
}

#[test]
fn tool_registry_entry_new_available_vs_unavailable() {
    use funera_core::re_act::tool::ToolRegistryEntry;
    let tool1 = MockTool::new("a", default_schema("a"));
    let tool2 = MockTool::new("b", default_schema("b"));

    let entry_avail = ToolRegistryEntry::new_available(Box::new(tool1));
    let entry_unavail = ToolRegistryEntry::new_unavailable(Box::new(tool2));

    assert!(entry_avail.is_available());
    assert!(!entry_unavail.is_available());
}

// ── Tool env tracking ──────────────────────────────────────────

#[tokio::test]
async fn env_add_tool_updates_watcher_json() {
    let registry = ToolRegistry::new();
    let client = create_client();
    let (mut env, mut watcher) = FuneraEnv::new(registry, client, default_model());

    let tool = MockTool::new("tracked_tool", default_schema("tracked_tool"));
    env.add_tool(Box::new(tool)).await;

    let tools = watcher.watch_tool();
    let array = tools.as_array().unwrap();
    assert_eq!(array.len(), 1);
    assert_eq!(array[0]["function"]["name"], "tracked_tool");
}

#[tokio::test]
async fn env_remove_tool_updates_watcher_json() {
    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("to_remove", default_schema("to_remove"));
    registry.add_tool(Box::new(tool));
    let client = create_client();
    let (mut env, mut watcher) = FuneraEnv::new(registry, client, default_model());

    env.remove_tool("to_remove").await;

    let tools = watcher.watch_tool();
    assert!(tools.as_array().unwrap().is_empty());
}

// ── Tool registry with Arc<RwLock<>> pattern ────────────────────

#[tokio::test]
async fn arc_rwlock_tool_registry_call_tool() {
    let mut registry = ToolRegistry::new();
    let tool = MockTool::new("arc_tool", default_schema("arc_tool"));
    registry.add_tool(Box::new(tool));
    let registry = Arc::new(RwLock::new(registry));

    let result = {
        let reg = registry.read().await;
        reg.call_tool("arc_tool", json!({})).await
    };
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "mock_result");
}

#[tokio::test]
async fn arc_rwlock_tool_not_found() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let result = {
        let reg = registry.read().await;
        reg.call_tool("ghost", json!({})).await
    };
    assert!(result.is_err());
}
