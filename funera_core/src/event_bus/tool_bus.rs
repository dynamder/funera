use serde_json::Value as JsonValue;
use tokio::sync::{mpsc, oneshot};

use crate::re_act::tool::ToolCallError;

pub struct ToolExecCommand {
    pub call_id: String,
    pub name: String,
    pub args: JsonValue,
    pub resp_tx: oneshot::Sender<Result<String, ToolCallError>>,
}

#[derive(Clone)]
pub struct ToolBus {
    exec_tx: mpsc::Sender<ToolExecCommand>,
}

impl ToolBus {
    pub fn new() -> (Self, mpsc::Receiver<ToolExecCommand>) {
        let (exec_tx, exec_rx) = mpsc::channel(10);
        (Self { exec_tx }, exec_rx)
    }

    pub async fn execute(
        &self,
        call_id: String,
        name: String,
        args: JsonValue,
    ) -> Result<String, ToolCallError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.exec_tx
            .send(ToolExecCommand {
                call_id,
                name,
                args,
                resp_tx,
            })
            .await
            .map_err(|_| ToolCallError::ToolExecutionError(anyhow::anyhow!("tool bus closed")))?;
        resp_rx
            .await
            .unwrap_or(Err(ToolCallError::ToolExecutionError(anyhow::anyhow!(
                "tool executor dropped"
            ))))
    }
}
