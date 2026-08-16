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

                // The guarded registry performs policy, boundary, approval, and
                // audit work during the call, so it still runs under the lock.
                #[cfg(feature = "security")]
                {
                    let registry = self.tool_registry.read().await;
                    registry.set_react_bus(cmd.react_bus.clone());
                    registry.call_tool(&cmd.name, cmd.args).await
                }
            };
            let _ = cmd.resp_tx.send(result);
        }
    }
}
