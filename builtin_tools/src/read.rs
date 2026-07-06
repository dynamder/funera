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
