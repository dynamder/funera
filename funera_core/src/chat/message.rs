use std::{fmt, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use uuid::Uuid;

use crate::re_act::tool::ToolType;

pub trait Message {
    fn to_prompt_content(&self) -> String;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}
impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::System => write!(f, "system"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MsgVariant {
    Text(TextMessage),
    ToolRequest(ToolRequestMessage),
    ToolResponse(ToolResponseMessage),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuneraMessage {
    id: Uuid,
    role: Role,
    timestamp: DateTime<Utc>,
    msg_variant: MsgVariant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextMessage {
    pub text: Arc<str>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolRequestMessage {
    tool_call_id: Uuid,
    tool_type: ToolType,
    function_name: Arc<str>,
    function_args: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResponseMessage {
    pub tool_call_id: Uuid,
    pub result: Arc<str>,
}

impl Message for TextMessage {
    fn to_prompt_content(&self) -> String {
        self.text.to_string()
    }
}

impl Message for ToolRequestMessage {
    fn to_prompt_content(&self) -> String {
        format!(
            "Tool Call -> id: {}, tool_type: {}, function_name: {}, function_args: {}",
            self.tool_call_id, self.tool_type, self.function_name, self.function_args
        )
    }
}

impl Message for ToolResponseMessage {
    fn to_prompt_content(&self) -> String {
        format!(
            "Tool Response -> tool_call_id: {}, result: {}",
            self.tool_call_id, self.result
        )
    }
}

impl Message for FuneraMessage {
    fn to_prompt_content(&self) -> String {
        match &self.msg_variant {
            MsgVariant::Text(text_msg) => text_msg.to_prompt_content(),
            MsgVariant::ToolRequest(tool_requet_msg) => tool_requet_msg.to_prompt_content(),
            MsgVariant::ToolResponse(tool_response_msg) => tool_response_msg.to_prompt_content(),
        }
    }
}

impl FuneraMessage {
    pub fn new(role: Role, msg_variant: MsgVariant) -> Self {
        Self {
            id: Uuid::new_v4(),
            role,
            timestamp: Utc::now(),
            msg_variant,
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn role(&self) -> &Role {
        &self.role
    }

    pub fn timestamp(&self) -> &DateTime<Utc> {
        &self.timestamp
    }

    pub fn msg_variant(&self) -> &MsgVariant {
        &self.msg_variant
    }

    pub fn format_json(&self) -> JsonValue {
        match &self.msg_variant {
            MsgVariant::Text(text_msg) => {
                json!({
                    "role": self.role.to_string(),
                    "content": text_msg.to_prompt_content(),
                })
            }
            MsgVariant::ToolRequest(tool_request_msg) => {
                json!({
                    "role": self.role.to_string(),
                    "tool_calls": [
                        {
                            "id": tool_request_msg.tool_call_id.clone(),
                            "type": tool_request_msg.tool_type.to_string(),
                            "function": {
                                "name": tool_request_msg.function_name.clone(),
                                "arguments": tool_request_msg.function_args.clone(),
                            }
                        }
                    ]
                })
            }
            MsgVariant::ToolResponse(tool_response_msg) => {
                json!({
                    "role": self.role.to_string(),
                    "tool_call_id": tool_response_msg.tool_call_id.clone(),
                    "content": tool_response_msg.to_prompt_content(),
                })
            }
        }
    }
}
