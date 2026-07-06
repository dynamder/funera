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
                let registry = self.tool_registry.read().await;
                registry.call_tool(&cmd.name, cmd.args).await
            };
            let _ = cmd.resp_tx.send(result);
        }
    }
}
