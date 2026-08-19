#![cfg(feature = "tool")]

use std::sync::Arc;

use tokio::sync::{Mutex, RwLock, mpsc};
#[cfg(test)]
use tokio_util::sync::CancellationToken;

use crate::event_bus::tool_bus::ToolExecCommand;
use crate::re_act::tool::{ToolCallError, ToolRegistry};

pub struct ToolExecutor {
    tool_registry: Arc<RwLock<ToolRegistry>>,
    exec_rx: Arc<Mutex<mpsc::Receiver<ToolExecCommand>>>,
}

impl ToolExecutor {
    pub fn new(
        tool_registry: Arc<RwLock<ToolRegistry>>,
        exec_rx: mpsc::Receiver<ToolExecCommand>,
    ) -> Self {
        Self {
            tool_registry,
            exec_rx: Arc::new(Mutex::new(exec_rx)),
        }
    }

    /// Build an executor over a receiver shared by multiple workers.
    ///
    /// Used by the env actor to fan one tool bus out to
    /// `max_concurrent_tools` workers: receive is serialized (brief lock),
    /// execution runs in parallel across workers.
    pub fn with_shared_receiver(
        tool_registry: Arc<RwLock<ToolRegistry>>,
        exec_rx: Arc<Mutex<mpsc::Receiver<ToolExecCommand>>>,
    ) -> Self {
        Self {
            tool_registry,
            exec_rx,
        }
    }

    pub async fn run(self) {
        loop {
            let cmd = { self.exec_rx.lock().await.recv().await };
            let Some(cmd) = cmd else { break };

            // Cooperative cancellation: if the issuing agent call was
            // cancelled, abandon this in-flight tool execution (the worker
            // itself survives — it is shared across calls). `biased` makes
            // cancellation win over a simultaneously-completed tool.
            let result = tokio::select! {
                biased;
                _ = cmd.cancel.cancelled() => {
                    Err(ToolCallError::ToolUnavailable("cancelled".into()))
                }
                r = self.execute_command(&cmd) => r,
            };
            let _ = cmd.resp_tx.send(result);
        }
    }

    /// Run one tool command (policy/registry/execution) without cancellation.
    async fn execute_command(&self, cmd: &ToolExecCommand) -> Result<String, ToolCallError> {
        // Without the security layer, clone the tool `Arc` and execute
        // it outside the registry lock so slow tools do not block
        // dynamic tool add/remove/availability changes.
        #[cfg(not(feature = "security"))]
        {
            let tool = {
                let registry = self.tool_registry.read().await;
                registry.get_tool_arc(&cmd.name)
            };
            match tool {
                Some(tool) => tool.execute(cmd.args.clone()).await,
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
            registry.call_tool(&cmd.name, cmd.args.clone()).await
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
                cancel: CancellationToken::new(),
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
