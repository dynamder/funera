use std::sync::Arc;

use anyhow::Result;
use serde_json::Value as JsonValue;
use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

use crate::{
    chat::message::{FuneraMessage, MsgVariant, Role, ToolResponseMessage},
    event_bus::react_bus::{ReactBus, ReactEvent, ToolCallRequest, ToolCallResponse},
    re_act::tool::{ToolCallError, ToolRegistry},
};

pub struct ToolExecutor {
    tool_registry: Arc<RwLock<ToolRegistry>>,
    react_bus: ReactBus,
    buf_msg_tx: mpsc::Sender<FuneraMessage>,
}

impl ToolExecutor {
    pub fn new(
        tool_registry: Arc<RwLock<ToolRegistry>>,
        react_bus: ReactBus,
        buf_msg_tx: mpsc::Sender<FuneraMessage>,
    ) -> Self {
        Self {
            tool_registry,
            react_bus,
            buf_msg_tx,
        }
    }

    pub fn react_bus(&self) -> &ReactBus {
        &self.react_bus
    }

    pub async fn execute(
        &self,
        call_id: String,
        name: String,
        args: JsonValue,
    ) -> Result<String, ToolCallError> {
        let request = ToolCallRequest {
            index: 0,
            call_id: call_id.clone(),
            name: name.clone(),
            args: args.clone(),
        };
        let _ = self.react_bus.send(ReactEvent::ToolExecRequest(request));

        let result = {
            let registry = self.tool_registry.read().await;
            registry.call_tool(&name, args).await
        };

        match &result {
            Ok(response) => {
                let response_event = ToolCallResponse {
                    call_id: call_id.clone(),
                    result: response.clone(),
                };
                let _ = self
                    .react_bus
                    .send(ReactEvent::ToolExecResponse(Ok(response_event)));

                let tool_response_msg = FuneraMessage::new(
                    Role::Tool,
                    MsgVariant::ToolResponse(ToolResponseMessage {
                        tool_call_id: Uuid::parse_str(&call_id).unwrap_or_default(),
                        result: response.clone().into(),
                    }),
                );
                let _ = self.buf_msg_tx.send(tool_response_msg).await;
            }
            Err(e) => {
                let _ = self
                    .react_bus
                    .send(ReactEvent::ToolExecResponse(Err(e.to_string())));
            }
        }

        result
    }
}
