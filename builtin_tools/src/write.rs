use std::path::Path;

use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
use serde_json::{json, Value as JsonValue};

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates parent directories if needed."
    }

    fn schema(&self) -> JsonValue {
        json!({
            "type": "function",
            "function": {
                "name": "write",
                "description": "Write content to a file, overwriting if it exists.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "filePath": {
                            "type": "string",
                            "description": "Absolute path to the file to write"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write to the file"
                        }
                    },
                    "required": ["filePath", "content"]
                }
            }
        })
    }

    async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError> {
        let file_path = args.get("filePath").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolCallError::ParameterMismatch(json!({"error": "missing filePath"}))
        })?;

        let content = args.get("content").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolCallError::ParameterMismatch(json!({"error": "missing content"}))
        })?;

        let path = Path::new(file_path);

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolCallError::ToolExecutionError(anyhow::anyhow!(
                    "cannot create parent directories: {}",
                    e
                ))
            })?;
        }

        tokio::fs::write(path, content).await.map_err(|e| {
            ToolCallError::ToolExecutionError(anyhow::anyhow!("cannot write file: {}", e))
        })?;

        Ok(format!("Successfully wrote {} bytes to {}", content.len(), file_path))
    }
}
