use std::path::Path;

use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
use serde_json::{json, Value as JsonValue};

use crate::hashline;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read files from the filesystem. Returns content with LINE#HASH: prefixes on each line. "
    }

    fn schema(&self) -> JsonValue {
        json!({
            "type": "function",
            "function": {
                "name": "read",
                "description": "Read a file from the filesystem. Returns content hashline-tagged.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "filePath": {
                            "type": "string",
                            "description": "Absolute path to the file to read"
                        },
                        "offset": {
                            "type": "number",
                            "description": "Start reading from this line number (1-indexed)"
                        },
                        "limit": {
                            "type": "number",
                            "description": "Maximum number of lines to return"
                        },
                        "raw": {
                            "type": "boolean",
                            "description": "Return plain content without LINE#HASH prefixes"
                        }
                    },
                    "required": ["filePath"]
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

        let offset = args.get("offset").and_then(|v| v.as_i64()).unwrap_or(1).max(1) as usize;
        let limit = args.get("limit").and_then(|v| v.as_i64());
        let raw = args.get("raw").and_then(|v| v.as_bool()).unwrap_or(false);
        let path = Path::new(file_path);

        if path.is_dir() {
            let mut entries = Vec::new();
            let mut read_dir = tokio::fs::read_dir(path).await.map_err(|e| {
                ToolCallError::ToolExecutionError(anyhow::anyhow!("cannot read directory: {}", e))
            })?;
            while let Some(entry) = read_dir.next_entry().await.map_err(|e| {
                ToolCallError::ToolExecutionError(anyhow::anyhow!("readdir error: {}", e))
            })? {
                let name = entry.file_name().to_string_lossy().to_string();
                entries.push(name);
            }
            entries.sort();
            let output = entries.join("\n");
            return Ok(output);
        }

        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            ToolCallError::ToolExecutionError(anyhow::anyhow!("cannot read file: {}", e))
        })?;

        if content.is_empty() {
            return Ok("(empty file — use prepend/append)".to_string());
        }

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = offset.saturating_sub(1);
        let end = match limit {
            Some(l) if l > 0 => (start + l as usize).min(total_lines),
            _ => total_lines,
        };

        if start >= total_lines {
            return Ok("(offset beyond end of file)".to_string());
        }

        if raw {
            let selected: Vec<&str> = lines[start..end].to_vec();
            return Ok(selected.join("\n"));
        }

        let mut output = String::new();
        for i in start..end {
            let prev = if i > 0 { lines[i - 1] } else { "" };
            let curr = lines[i];
            let next = if i + 1 < total_lines { lines[i + 1] } else { "" };
            let anchor = hashline::compute_anchor(prev, curr, next);
            output.push_str(&hashline::format_line_trimmed(i + 1, &anchor, curr));
            output.push('\n');
        }

        Ok(output.trim_end().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("funera_read_test_{}_{}", std::process::id(), label))
    }

    async fn setup_file(label: &str, name: &str, content: &str) -> PathBuf {
        let dir = test_dir(label);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join(name);
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    async fn cleanup(label: &str) {
        let _ = tokio::fs::remove_dir_all(test_dir(label)).await;
    }

    #[tokio::test]
    async fn read_missing_file_path_param() {
        let tool = ReadTool;
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolCallError::ParameterMismatch(_) => {}
            e => panic!("expected ParameterMismatch, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn read_nonexistent_file() {
        let tool = ReadTool;
        let result = tool.execute(json!({"filePath": "C:\\nonexistent_file_xyz\\test.txt"})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_simple_file() {
        let path = setup_file("simple", "simple.txt", "hello\nworld\n").await;
        let tool = ReadTool;
        let result = tool.execute(json!({"filePath": path.to_string_lossy()})).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("hello"));
        assert!(output.contains("world"));
        assert!(output.contains("#"));
        cleanup("simple").await;
    }

    #[tokio::test]
    async fn read_raw_mode() {
        let path = setup_file("raw", "raw_test.txt", "line1\nline2\nline3\n").await;
        let tool = ReadTool;
        let result = tool.execute(json!({"filePath": path.to_string_lossy(), "raw": true})).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "line1\nline2\nline3");
        cleanup("raw").await;
    }

    #[tokio::test]
    async fn read_with_offset() {
        let path = setup_file("offset", "offset.txt", "a\nb\nc\nd\n").await;
        let tool = ReadTool;
        let result = tool.execute(
            json!({"filePath": path.to_string_lossy(), "offset": 3})
        ).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.contains("a"));
        assert!(output.contains("c"));
        assert!(output.contains("d"));
        cleanup("offset").await;
    }

    #[tokio::test]
    async fn read_with_limit() {
        let path = setup_file("limit", "limit.txt", "1\n2\n3\n4\n5\n").await;
        let tool = ReadTool;
        let result = tool.execute(
            json!({"filePath": path.to_string_lossy(), "limit": 2})
        ).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("1"));
        assert!(output.contains("2"));
        assert!(!output.contains("3"));
        cleanup("limit").await;
    }

    #[tokio::test]
    async fn read_empty_file() {
        let path = setup_file("empty", "empty.txt", "").await;
        let tool = ReadTool;
        let result = tool.execute(json!({"filePath": path.to_string_lossy()})).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("empty file"));
        cleanup("empty").await;
    }

    #[tokio::test]
    async fn read_directory() {
        let dir = test_dir("dir");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("a.txt"), "").await.unwrap();
        tokio::fs::write(dir.join("b.txt"), "").await.unwrap();
        let tool = ReadTool;
        let result = tool.execute(json!({"filePath": dir.to_string_lossy()})).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("a.txt"));
        assert!(output.contains("b.txt"));
        cleanup("dir").await;
    }
}
