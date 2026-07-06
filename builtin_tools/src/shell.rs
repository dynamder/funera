use std::time::Duration;

use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
use serde_json::{json, Value as JsonValue};
use tokio::process::Command;
use tokio::time::timeout;

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute shell commands. Use with caution."
    }

    fn schema(&self) -> JsonValue {
        json!({
            "type": "function",
            "function": {
                "name": "shell",
                "description": "Execute a shell command and return its output.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Shell command to execute"
                        },
                        "workdir": {
                            "type": "string",
                            "description": "Working directory for the command"
                        },
                        "timeout": {
                            "type": "number",
                            "description": "Timeout in seconds (default 30)"
                        }
                    },
                    "required": ["command"]
                }
            }
        })
    }

    async fn execute(&self, args: JsonValue) -> Result<String, ToolCallError> {
        let command_str = args.get("command").and_then(|v| v.as_str()).ok_or_else(|| {
            ToolCallError::ParameterMismatch(json!({"error": "missing command"}))
        })?;

        let workdir = args.get("workdir").and_then(|v| v.as_str());
        let timeout_secs = args.get("timeout").and_then(|v| v.as_f64()).unwrap_or(30.0);
        let timeout_dur = Duration::from_secs_f64(timeout_secs.max(1.0).min(300.0));

        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.arg("/c").arg(command_str);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command_str);
            c
        };

        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        if let Some(dir) = workdir {
            cmd.current_dir(dir);
        }

        let output = timeout(timeout_dur, cmd.output())
            .await
            .map_err(|_| {
                ToolCallError::ToolExecutionError(anyhow::anyhow!(
                    "command timed out after {:.1}s",
                    timeout_secs
                ))
            })?
            .map_err(|e| {
                ToolCallError::ToolExecutionError(anyhow::anyhow!(
                    "failed to execute command: {}",
                    e
                ))
            })?;

        let mut result = String::new();

        if !output.stdout.is_empty() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            result.push_str("stdout:\n");
            result.push_str(&stdout);
        }

        if !output.stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            result.push_str("stderr:\n");
            result.push_str(&stderr);
        }

        if output.status.success() {
            if result.is_empty() {
                result.push_str("(command completed with no output)");
            }
        } else {
            let exit_code = output.status.code().unwrap_or(-1);
            result.push_str(&format!("\n(exit code: {})", exit_code));
        }

        Ok(result)
    }
}
