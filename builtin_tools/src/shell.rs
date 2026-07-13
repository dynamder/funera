use std::time::Duration;

use async_trait::async_trait;
use funera_core::re_act::tool::{Tool, ToolCallError};
#[cfg(feature = "sandbox")]
use funera_core::security::sandbox::{Sandbox, SandboxPolicy, format_triple_output};
use serde_json::{Value as JsonValue, json};
use tokio::time::timeout;

/// Tool for executing shell commands.
///
/// Cross-platform: uses `cmd /c` on Windows, `sh -c` on Unix.
/// Supports working directory override and configurable timeout (default 30s,
/// clamped to 1–300s).
///
/// When compiled with `sandbox` feature, an optional [`SandboxPolicy`]
/// can be set to apply kernel-enforced isolation:
///
/// | Platform | Mechanism |
/// |----------|-----------|
/// | Linux    | Landlock (nono crate) via `pre_exec` |
/// | macOS    | Seatbelt (nono crate) via `pre_exec` |
/// | Windows  | Write-Restricted Token + synthetic SID + ACLs |
///
/// If the sandbox cannot be applied (unsupported kernel, missing privileges),
/// execution falls back to the normal path.
pub struct ShellTool {
    #[cfg(feature = "sandbox")]
    sandbox_policy: Option<SandboxPolicy>,
}

impl ShellTool {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "sandbox")]
            sandbox_policy: None,
        }
    }

    /// Create a ShellTool that applies sandbox isolation to every
    /// subprocess invocation.
    ///
    /// Uses Landlock/Seatbelt on Linux/macOS (via `nono`),
    /// Write-Restricted Tokens on Windows.
    ///
    /// If the platform cannot enforce isolation, the tool falls back
    /// to normal execution gracefully.
    #[cfg(feature = "sandbox")]
    pub fn with_sandbox(policy: SandboxPolicy) -> Self {
        Self {
            sandbox_policy: Some(policy),
        }
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

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
        let command_str = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolCallError::ParameterMismatch(json!({"error": "missing command"})))?;

        let workdir = args.get("workdir").and_then(|v| v.as_str());
        let timeout_secs = args.get("timeout").and_then(|v| v.as_f64()).unwrap_or(30.0);
        let timeout_dur = Duration::from_secs_f64(timeout_secs.max(1.0).min(300.0));

        let (shell, shell_flag) = if cfg!(target_os = "windows") {
            ("cmd", "/c")
        } else {
            ("sh", "-c")
        };

        // ── sandboxed path (platform-independent) ───────────────────
        #[cfg(feature = "sandbox")]
        if let Some(ref policy) = self.sandbox_policy {
            if policy.enabled {
                return self
                    .execute_sandboxed(shell, shell_flag, command_str, workdir, timeout_dur)
                    .await;
            }
        }

        // ── normal path ─────────────────────────────────────────────
        self.execute_normal(shell, shell_flag, command_str, workdir, timeout_dur)
            .await
    }
}

// ── private helpers ────────────────────────────────────────────────

impl ShellTool {
    /// Normal execution via tokio::process::Command (no sandbox).
    async fn execute_normal(
        &self,
        shell: &str,
        shell_flag: &str,
        command_str: &str,
        workdir: Option<&str>,
        timeout_dur: Duration,
    ) -> Result<String, ToolCallError> {
        let mut cmd = tokio::process::Command::new(shell);
        cmd.arg(shell_flag).arg(command_str);
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
                    timeout_dur.as_secs_f64()
                ))
            })?
            .map_err(|e| {
                ToolCallError::ToolExecutionError(anyhow::anyhow!(
                    "failed to execute command: {}",
                    e
                ))
            })?;

        Ok(format_output(output))
    }

    /// Sandboxed execution — delegates to [`Sandbox`] which is platform-independent.
    #[cfg(feature = "sandbox")]
    async fn execute_sandboxed(
        &self,
        shell: &str,
        shell_flag: &str,
        command_str: &str,
        workdir: Option<&str>,
        timeout_dur: Duration,
    ) -> Result<String, ToolCallError> {
        let policy = self.sandbox_policy.as_ref().ok_or_else(|| {
            ToolCallError::ToolExecutionError(anyhow::anyhow!("sandbox policy missing"))
        })?;

        let sandbox = Sandbox::new(policy).map_err(|e| {
            ToolCallError::ToolExecutionError(anyhow::anyhow!("failed to set up sandbox: {e}"))
        })?;

        let (stdout, stderr, exit_code) = sandbox
            .execute(shell, shell_flag, command_str, workdir, timeout_dur)
            .await
            .map_err(|e| {
                ToolCallError::ToolExecutionError(anyhow::anyhow!("sandboxed command failed: {e}"))
            })?;

        Ok(format_triple_output(&stdout, &stderr, exit_code))
    }
}

// ── output formatting ──────────────────────────────────────────────

fn format_output(output: std::process::Output) -> String {
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

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── basic execution tests ──────────────────────────────────────

    #[tokio::test]
    async fn shell_missing_command() {
        let tool = ShellTool::new();
        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolCallError::ParameterMismatch(_) => {}
            e => panic!("expected ParameterMismatch, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn shell_echo() {
        let tool = ShellTool::new();
        let result = tool.execute(json!({"command": "echo hello"})).await;
        assert!(result.is_ok(), "echo failed: {:?}", result.err());
        let output = result.unwrap();
        assert!(output.contains("hello"));
    }

    #[tokio::test]
    async fn shell_exit_code() {
        let tool = ShellTool::new();
        let cmd = if cfg!(target_os = "windows") {
            "cmd /c exit 42"
        } else {
            "exit 42"
        };
        let result = tool.execute(json!({"command": cmd})).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("42"));
    }

    #[tokio::test]
    async fn shell_timeout_param() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({"command": "echo quick", "timeout": 5}))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn shell_long_command() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({"command": "echo a b c d e f g h i j k l m n o p"}))
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("a b c d e f g h i"));
    }

    #[tokio::test]
    async fn shell_workdir() {
        let tool = ShellTool::new();
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        let cmd = if cfg!(target_os = "windows") {
            "cd"
        } else {
            "pwd"
        };
        let result = tool.execute(json!({"command": cmd, "workdir": dir})).await;
        assert!(result.is_ok());
    }

    // ── sandbox execution tests ────────────────────────────────────
    //
    // These tests fork a child process and apply nono Landlock/Seatbelt
    // restrictions via the pre_exec hook.  They require:
    //   1. sandbox feature enabled
    //   2. A non-Windows OS (nono uses Unix-only APIs internally)
    //
    // When the kernel does not support Landlock (e.g. old kernel, Docker
    // with --security-opt seccomp=unconfined), the sandboxed path
    // gracefully runs without isolation — tests that verify *blocked*
    // access skip themselves at runtime.

    /// Helper: run a quick shell command through a sandboxed ShellTool
    /// and return (success, stdout_text, exit_code).
    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    async fn run_sandboxed(
        policy: funera_core::security::sandbox::SandboxPolicy,
        cmd: &str,
        workdir: Option<&str>,
    ) -> (bool, String, i32) {
        let tool = ShellTool::with_sandbox(policy);
        let mut args = json!({"command": cmd, "timeout": 10});
        if let Some(d) = workdir {
            args["workdir"] = json!(d);
        }
        let result = tool.execute(args).await;
        match result {
            Ok(output) => {
                let exit_code = extract_exit_code(&output);
                (true, output, exit_code)
            }
            Err(e) => {
                eprintln!("[run_sandboxed] execute error: {e:?}");
                (false, String::new(), -1)
            }
        }
    }

    /// Extract the exit code from ShellTool's formatted output.
    /// Returns 0 if the command succeeded (no explicit exit code line).
    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    fn extract_exit_code(output: &str) -> i32 {
        if let Some(pos) = output.find("(exit code: ") {
            let rest = &output[pos + "(exit code: ".len()..];
            if let Some(end) = rest.find(')') {
                return rest[..end].parse().unwrap_or(-1);
            }
        }
        0 // no explicit exit code means success
    }

    /// Unique counter so parallel tests don't clobber each other's
    /// temp directories.
    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    /// Create a fresh temp directory for a sandbox test and write a
    /// known file into it.  Each call uses a unique suffix so that
    /// parallel test execution does not cause races.
    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    fn sandbox_temp_dir() -> (std::path::PathBuf, std::path::PathBuf) {
        use std::fs;
        let id = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("funera_sandbox_test_{id}"));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("create sandbox temp dir");
        let file = base.join("test_file.txt");
        fs::write(&file, b"hello from sandbox").expect("write test file");
        (base, file)
    }

    /// Cleanup a sandbox temp directory after a test.
    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    fn cleanup_sandbox_temp(base: &std::path::Path) {
        let _ = std::fs::remove_dir_all(base);
    }

    /// Check whether the kernel supports Landlock sandboxing.
    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    fn landlock_available() -> bool {
        nono::Sandbox::is_supported()
    }

    // ── test: sandboxed process can read allowed paths ─────────────

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[tokio::test]
    async fn sandbox_can_access_allowed_dir() {
        if !landlock_available() {
            eprintln!("SKIP: Landlock not available on this kernel");
            return;
        }
        // Allow system paths + /tmp (the parent of our temp dir).
        // Landlock requires every ancestor directory in the path to
        // be explicitly allowed for traversal.
        let (tmpdir, test_file) = sandbox_temp_dir();
        let mut sys = funera_core::security::sandbox::system_sandbox_read_paths();
        sys.push(std::path::PathBuf::from("/tmp"));
        let policy = funera_core::security::sandbox::SandboxPolicy {
            read_paths: sys,
            ..Default::default()
        };
        let cmd = format!("cat {}", test_file.display());
        let (success, output, exit_code) = run_sandboxed(policy, &cmd, None).await;
        cleanup_sandbox_temp(&tmpdir);
        assert!(
            success,
            "sandboxed command should succeed - output: {output}"
        );
        assert_eq!(exit_code, 0, "exit code should be 0 - output: {output}");
        assert!(output.contains("hello from sandbox"), "output: {output}");
    }

    // ── test: sandboxed process cannot read outside allowed paths ───

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[tokio::test]
    async fn sandbox_cannot_access_restricted_path() {
        if !landlock_available() {
            eprintln!("SKIP: Landlock not available on this kernel");
            return;
        }
        // /tmp is NOT in the allowed set → Landlock blocks traversal.
        // We also do NOT create any temp dir here since we are
        // deliberately testing that Landlock denies access.
        let (tmpdir, _test_file) = sandbox_temp_dir();
        let cmd = format!("cat {}", _test_file.display());
        let policy = funera_core::security::sandbox::SandboxPolicy {
            read_paths: funera_core::security::sandbox::system_sandbox_read_paths(),
            ..Default::default()
        };
        let (success, output, exit_code) = run_sandboxed(policy, &cmd, None).await;
        cleanup_sandbox_temp(&tmpdir);
        // /tmp is outside the allowed set → EACCES or ENOENT
        assert!(success, "command should produce exit code output: {output}");
        assert_ne!(
            exit_code, 0,
            "expected non-zero exit for blocked read: {output}"
        );
    }

    // ── test: only system paths allowed, file creation blocked ─────

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[tokio::test]
    async fn sandbox_without_user_paths_fails_file_write() {
        if !landlock_available() {
            eprintln!("SKIP: Landlock not available on this kernel");
            return;
        }
        let (tmpdir, _test_file) = sandbox_temp_dir();
        let blocked_path = tmpdir.join("blocked_write.txt");
        // Only system paths — /tmp is NOT in the allowed set
        let policy = funera_core::security::sandbox::SandboxPolicy {
            read_paths: funera_core::security::sandbox::system_sandbox_read_paths(),
            ..Default::default()
        };
        let cmd = format!("echo forbidden > {}", blocked_path.display());
        let (success, output, exit_code) = run_sandboxed(policy, &cmd, None).await;
        cleanup_sandbox_temp(&tmpdir);
        // /tmp is excluded → Landlock blocks file creation
        assert!(success, "command should produce exit code output: {output}");
        assert_ne!(
            exit_code, 0,
            "expected non-zero exit for blocked write: {output}"
        );
    }

    // ── test: workdir is respected inside sandbox ───────────────────

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[tokio::test]
    async fn sandbox_workdir_respected_in_sandbox() {
        if !landlock_available() {
            eprintln!("SKIP: Landlock not available on this kernel");
            return;
        }
        let (tmpdir, _test_file) = sandbox_temp_dir();
        // Allow /tmp for traversal, then the subdir for read+write + pwd
        let mut sys_r = funera_core::security::sandbox::system_sandbox_read_paths();
        sys_r.push(std::path::PathBuf::from("/tmp"));
        let policy = funera_core::security::sandbox::SandboxPolicy {
            read_paths: sys_r,
            read_write_paths: vec![tmpdir.clone()],
            ..Default::default()
        };
        let workdir_str = tmpdir.to_string_lossy().to_string();
        let (success, output, exit_code) = run_sandboxed(policy, "pwd", Some(&workdir_str)).await;
        cleanup_sandbox_temp(&tmpdir);
        assert!(success, "pwd should succeed - output: {output}");
        assert_eq!(exit_code, 0, "expected exit code 0 - output: {output}");
        assert!(
            output.contains(&workdir_str),
            "expected workdir ({workdir_str}) in pwd output: {output}"
        );
    }

    // ── test: sandbox disabled → no isolation ──────────────────────

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[tokio::test]
    async fn sandbox_disabled_no_isolation() {
        // This test runs even when Landlock is unavailable because
        // the disabled policy skips the sandbox path entirely.
        let policy = funera_core::security::sandbox::SandboxPolicy::disabled();
        let (success, output, exit_code) = run_sandboxed(policy, "uname -o", None).await;
        assert!(success, "disabled sandbox should not block commands");
        assert_eq!(exit_code, 0, "expected exit code 0");
        assert!(!output.is_empty(), "expected uname output");
    }

    // ── test: sandbox on Windows runs with fallback if needed ───────
    //
    // On Windows, `Sandbox::new` creates a `WindowsSandbox` (Write-Restricted
    // Token + ACLs). The sandboxed command runs under the restricted token;
    // if that fails (e.g. missing admin privileges), execution falls back
    // to a normal subprocess with network-only isolation. Either way, the
    // command should produce correct output.
    #[cfg(all(feature = "sandbox", target_os = "windows"))]
    #[tokio::test]
    async fn sandbox_windows_falls_through_gracefully() {
        let policy = funera_core::security::sandbox::SandboxPolicy::default();
        let tool = ShellTool::with_sandbox(policy);
        let result = tool
            .execute(json!({"command": "echo windows_sandbox_graceful"}))
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("windows_sandbox_graceful"));
    }
}
