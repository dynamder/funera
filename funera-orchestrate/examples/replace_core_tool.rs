//! Replace a built-in core tool with a custom implementation at runtime.
//!
//! The runtime initially loads the builtin `read` tool through a `ToolPlugin`.
//! We then reconcile the same loader entry id with a custom `read` tool and a
//! bumped revision, which hot-replaces the old implementation in place.
//!
//! ```bash
//! cargo run --example replace_core_tool --features funera-builtin-tools
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use funera_core::plugin::{Plugin, ToolPlugin};
use funera_core::re_act::tool::{Tool, ToolCallError};
use funera_orchestrate::{AgentRuntime, DeepSeekProvider};
use serde_json::Value as JsonValue;

#[derive(Default)]
struct CustomReadTool;

impl Plugin for CustomReadTool {
    fn name(&self) -> &str {
        // Same tool name as the built-in `read` tool.
        "read"
    }
}

#[async_trait]
impl Tool for CustomReadTool {
    fn description(&self) -> &str {
        "Custom replacement for the built-in read tool"
    }

    fn schema(&self) -> JsonValue {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "read",
                "description": "Custom replacement for the built-in read tool",
                "parameters": {"type": "object", "properties": {}}
            }
        })
    }

    async fn execute(&self, _args: JsonValue) -> Result<String, ToolCallError> {
        Ok("custom-read-tool".into())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = AgentRuntime::<DeepSeekProvider>::builder()
        .api_key("sk-test")
        .model("deepseek-v4-flash")
        .with_builtin_tools()
        .build()?;

    let names = runtime.tool_names().await;
    assert!(
        names.contains(&"read".to_string()),
        "builtin read should be present: {names:?}"
    );

    // The builtin tools were loaded as `tool:read:0`, `tool:write:1`, ...
    // Reconcile the same entry id with a custom plugin and a bumped revision.
    let report = runtime
        .reconcile_plugins(vec![
            funera_core::loader::PluginEntry::new(
                "tool:read:0",
                Arc::new(ToolPlugin::new(Arc::new(CustomReadTool))),
            )
            .revision(1),
        ])
        .await;

    println!("reloaded: {:?}", report.reloaded);
    assert_eq!(report.reloaded, vec!["tool:read:0".to_string()]);

    let names = runtime.tool_names().await;
    assert!(
        names.contains(&"read".to_string()),
        "read should still be present: {names:?}"
    );

    println!("replace_core_tool: all assertions passed");
    Ok(())
}
