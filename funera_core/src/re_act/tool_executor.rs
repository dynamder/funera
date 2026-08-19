#![cfg(feature = "tool")]

use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};

use crate::event_bus::tool_bus::ToolExecCommand;
use crate::re_act::tool::ToolRegistry;

pub struct ToolExecutor {
    tool_registry: Arc<RwLock<ToolRegistry>>,
    exec_rx: mpsc::Receiver<ToolExecCommand>,
}

impl ToolExecutor {
    pub fn new(
        tool_registry: Arc<RwLock<ToolRegistry>>,
        exec_rx: mpsc::Receiver<ToolExecCommand>,
    ) -> Self {
        Self {
            tool_registry,
            exec_rx,
        }
    }

    pub async fn run(mut self) {
        while let Some(cmd) = self.exec_rx.recv().await {
            let result = {
                // Without the security layer, clone the tool `Arc` and execute
                // it outside the registry lock so slow tools do not block
                // dynamic tool add/remove/availability changes.
                #[cfg(not(feature = "security"))]
                {
                    use crate::re_act::tool::ToolCallError;
                    let tool = {
                        let registry = self.tool_registry.read().await;
                        registry.get_tool_arc(&cmd.name)
                    };
                    match tool {
                        Some(tool) => tool.execute(cmd.args).await,
                        None => Err(ToolCallError::ToolNotFound(cmd.name.clone())),
                    }
                }

                // Clone the guarded registry so policy/boundary/approval/audit
                // run against a snapshot while the tool itself executes outside
                // the registry lock. Pending approvals and the react bus are
                // `Arc`-shared, so cloned registries still cooperate.
                #[cfg(feature = "security")]
                {
                    let registry = {
                        let guard = self.tool_registry.read().await;
                        guard.clone()
                    };
                    registry.set_react_bus(cmd.react_bus.clone());
                    registry.call_tool(&cmd.name, cmd.args).await
                }
            };
            let _ = cmd.resp_tx.send(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::re_act::tool::{Tool, ToolCallError};
    use serde_json::{Value as JsonValue, json};
    use tokio::sync::oneshot;

    struct MockTool;
    #[async_trait::async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            "mock"
        }
        fn description(&self) -> &str {
            "mock tool"
        }
        fn schema(&self) -> JsonValue {
            json!({"type": "function", "function": {"name": "mock"}})
        }
        async fn execute(&self, _args: JsonValue) -> Result<String, ToolCallError> {
            Ok("done".into())
        }
    }

    #[tokio::test]
    async fn run_executes_tool_commands() {
        let registry = Arc::new(RwLock::new(ToolRegistry::new()));
        registry.write().await.add_tool(Arc::new(MockTool));

        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let handle = tokio::spawn(ToolExecutor::new(registry, cmd_rx).run());

        let (resp_tx, resp_rx) = oneshot::channel();
        cmd_tx
            .send(ToolExecCommand {
                call_id: "call-1".into(),
                name: "mock".into(),
                args: json!({}),
                resp_tx,
                react_bus: None,
            })
            .await
            .expect("executor must consume commands");

        // A bounded wait turns a silently-empty `run` into a test failure
        // instead of a hang.
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), resp_rx)
            .await
            .expect("executor must answer the tool command")
            .expect("response channel must be delivered")
            .expect("tool must execute successfully");
        assert_eq!(result, "done");

        drop(cmd_tx);
        handle.await.expect("executor task must exit cleanly");
    }
}
