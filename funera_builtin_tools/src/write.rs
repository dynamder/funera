use std::path::Path;

use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
use serde_json::{Value as JsonValue, json};

/// Tool for writing content to files.
///
/// Overwrites existing files and creates parent directories as needed.
/// Requires `filePath` and `content` parameters.
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
        let file_path = args
            .get("filePath")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolCallError::ParameterMismatch(json!({"error": "missing filePath"}))
            })?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolCallError::ParameterMismatch(json!({"error": "missing content"})))?;

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

        Ok(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            file_path
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "funera_write_test_{}_{}",
            std::process::id(),
            label
        ))
    }

    async fn cleanup(label: &str) {
        let _ = tokio::fs::remove_dir_all(test_dir(label)).await;
    }

    #[tokio::test]
    async fn write_missing_file_path() {
        let tool = WriteTool;
        let result = tool.execute(json!({"content": "hello"})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolCallError::ParameterMismatch(_) => {}
            e => panic!("expected ParameterMismatch, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn write_missing_content() {
        let tool = WriteTool;
        let result = tool.execute(json!({"filePath": "dummy.txt"})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolCallError::ParameterMismatch(_) => {}
            e => panic!("expected ParameterMismatch, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn write_creates_file() {
        let dir = test_dir("creates");
        let path = dir.join("new_file.txt");
        let tool = WriteTool;
        let result = tool
            .execute(json!({
                "filePath": path.to_string_lossy(),
                "content": "hello world"
            }))
            .await;
        assert!(result.is_ok());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "hello world");
        cleanup("creates").await;
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let dir = test_dir("nested");
        let path = dir.join("sub").join("deep").join("nested.txt");
        let tool = WriteTool;
        let result = tool
            .execute(json!({
                "filePath": path.to_string_lossy(),
                "content": "nested content"
            }))
            .await;
        assert!(result.is_ok());
        assert!(path.exists());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "nested content");
        cleanup("nested").await;
    }

    #[tokio::test]
    async fn write_overwrites_existing() {
        let dir = test_dir("overwrite");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("overwrite.txt");
        tokio::fs::write(&path, "old content").await.unwrap();
        let tool = WriteTool;
        let result = tool
            .execute(json!({
                "filePath": path.to_string_lossy(),
                "content": "new content"
            }))
            .await;
        assert!(result.is_ok());
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "new content");
        cleanup("overwrite").await;
    }
}
