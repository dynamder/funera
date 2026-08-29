use serde_json::Value as JsonValue;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::event_bus::react_bus::ReactBus;
use crate::re_act::tool::ToolCallError;

pub struct ToolExecCommand {
    pub call_id: String,
    pub name: String,
    pub args: JsonValue,
    pub resp_tx: oneshot::Sender<Result<String, ToolCallError>>,
    pub react_bus: Option<ReactBus>,
    /// Cancel token of the agent call that issued this command: when it fires,
    /// the executing worker abandons the in-flight tool execution.
    pub cancel: CancellationToken,
}

#[derive(Clone)]
pub struct ToolBus {
    exec_tx: mpsc::Sender<ToolExecCommand>,
}

impl ToolBus {
    pub fn new() -> (Self, mpsc::Receiver<ToolExecCommand>) {
        let (exec_tx, exec_rx) = mpsc::channel(64);
        (Self { exec_tx }, exec_rx)
    }

    pub async fn execute(
        &self,
        call_id: String,
        name: String,
        args: JsonValue,
        react_bus: Option<ReactBus>,
        cancel: CancellationToken,
    ) -> Result<String, ToolCallError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.exec_tx
            .send(ToolExecCommand {
                call_id,
                name,
                args,
                resp_tx,
                react_bus,
                cancel,
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
