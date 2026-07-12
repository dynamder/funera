use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;

#[cfg(feature = "regex")]
use regex::Regex;

/// Policy configuration for tool execution.
///
/// Controls which tools are allowed or denied, enforces argument size limits,
/// timeout bounds, working directory restrictions, and delegates shell-command
/// scrutiny to an optional [`ShellPolicy`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPolicy {
    /// If set, only tools in this set are allowed.
    pub allowed_tools: Option<HashSet<String>>,

    /// Tools in this set are always denied.
    pub denied_tools: HashSet<String>,

    /// Maximum size (in bytes) of a tool's JSON arguments.
    pub max_args_size: usize,

    /// Maximum allowed timeout in seconds.
    pub max_timeout_secs: f64,

    /// Shell command policy (applies to shell/bash/sh/cmd/powershell tools).
    pub shell_policy: Option<ShellPolicy>,

    /// Allowed working directories for shell tools.
    pub allowed_workdirs: HashSet<String>,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            allowed_tools: None,
            denied_tools: HashSet::new(),
            max_args_size: 1024 * 1024,
            max_timeout_secs: 300.0,
            shell_policy: Some(ShellPolicy::permissive()),
            allowed_workdirs: HashSet::new(),
        }
    }
}

impl ToolPolicy {
    /// A fully permissive policy — all tools allowed, no restrictions.
    pub fn permissive() -> Self {
        Self::default()
    }

    /// A strict policy — no tools allowed by default, dangerous shell commands blocked.
    pub fn strict() -> Self {
        Self {
            allowed_tools: Some(HashSet::new()),
            denied_tools: HashSet::new(),
            max_args_size: 1024 * 1024,
            max_timeout_secs: 300.0,
            shell_policy: Some(ShellPolicy::strict()),
            allowed_workdirs: HashSet::new(),
        }
    }

    /// Check whether a tool is allowed by the allow/deny lists.
    pub fn check_tool_allowed(&self, name: &str) -> Result<(), PolicyError> {
        if self.denied_tools.contains(name) {
            return Err(PolicyError::ToolDenied(name.to_string()));
        }
        if let Some(ref allowed) = self.allowed_tools {
            if !allowed.contains(name) {
                return Err(PolicyError::ToolNotAllowed(name.to_string()));
            }
        }
        Ok(())
    }

    /// Check that tool arguments do not exceed the maximum size.
    pub fn check_args(&self, args: &JsonValue) -> Result<(), PolicyError> {
        let size = serde_json::to_vec(args).map(|v| v.len()).unwrap_or(0);
        if size > self.max_args_size {
            return Err(PolicyError::ArgsTooLarge(size, self.max_args_size));
        }
        Ok(())
    }

    /// Check a shell command against the configured shell policy.
    ///
    /// Only applies if the tool is recognized as a shell tool and a
    /// [`ShellPolicy`] is configured.
    pub fn check_shell_command(
        &self,
        tool_name: &str,
        args: &JsonValue,
    ) -> Result<(), PolicyError> {
        let policy = match self.shell_policy {
            Some(ref p) => p,
            None => return Ok(()),
        };
        if !policy.is_relevant_tool(tool_name) {
            return Ok(());
        }
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(()),
        };
        policy.check_command(command)
    }

    /// Check that a timeout value does not exceed the maximum.
    pub fn check_timeout(&self, timeout: f64) -> Result<(), PolicyError> {
        if timeout > self.max_timeout_secs {
            return Err(PolicyError::TimeoutExceeded(timeout, self.max_timeout_secs));
        }
        Ok(())
    }

    /// Check that a working directory is within the allowed paths.
    pub fn check_workdir(&self, workdir: &str) -> Result<(), PolicyError> {
        if self.allowed_workdirs.is_empty() {
            return Ok(());
        }
        if !self
            .allowed_workdirs
            .iter()
            .any(|root| workdir.starts_with(root))
        {
            return Err(PolicyError::WorkdirDenied(workdir.to_string()));
        }
        Ok(())
    }
}

/// Policy for shell command execution.
///
/// Controls which commands are allowed or denied, with built-in detection of
/// dangerous patterns (e.g., `rm -rf`, `diskpart`, `reg add`, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellPolicy {
    /// Commands matching these prefixes are allowed.
    pub allow_commands: Vec<String>,

    /// Patterns (substring or regex) that cause commands to be denied.
    pub deny_patterns: Vec<String>,

    /// Block common dangerous commands automatically.
    pub block_builtin_dangerous: bool,
}

impl ShellPolicy {
    /// A permissive shell policy — no restrictions.
    pub fn permissive() -> Self {
        Self {
            allow_commands: vec![],
            deny_patterns: vec![],
            block_builtin_dangerous: false,
        }
    }

    /// A strict policy — blocks built-in dangerous commands.
    pub fn strict() -> Self {
        Self {
            allow_commands: vec![],
            deny_patterns: vec![],
            block_builtin_dangerous: true,
        }
    }

    /// Allow only the given commands (prefix match) and block built-in dangerous ones.
    pub fn with_allowed(commands: Vec<String>) -> Self {
        Self {
            allow_commands: commands,
            deny_patterns: vec![],
            block_builtin_dangerous: true,
        }
    }

    fn is_relevant_tool(&self, tool_name: &str) -> bool {
        matches!(tool_name, "shell" | "bash" | "sh" | "cmd" | "powershell")
    }

    /// Evaluate a shell command against all allow/deny rules.
    pub fn check_command(&self, command: &str) -> Result<(), PolicyError> {
        if !self.allow_commands.is_empty() {
            let allowed = self.allow_commands.iter().any(|prefix| {
                command.starts_with(prefix.as_str()) || command.starts_with(&format!("{} ", prefix))
            });
            if !allowed {
                return Err(PolicyError::CommandNotAllowed(command.to_string()));
            }
        }

        if self.block_builtin_dangerous {
            self.match_dangerous(command)?;
        }

        for pattern in &self.deny_patterns {
            if self.match_pattern(command, pattern) {
                return Err(PolicyError::CommandDenied(
                    command.to_string(),
                    pattern.clone(),
                ));
            }
        }

        Ok(())
    }

    #[cfg(feature = "regex")]
    fn match_pattern(&self, command: &str, pattern: &str) -> bool {
        Regex::new(pattern).map_or(false, |re| re.is_match(command))
    }

    #[cfg(not(feature = "regex"))]
    fn match_pattern(&self, command: &str, pattern: &str) -> bool {
        command.contains(pattern)
    }

    fn match_dangerous(&self, cmd: &str) -> Result<(), PolicyError> {
        #[cfg(feature = "regex")]
        {
            let dangerous: &[&str] = &[
                r"(?i)\brm\s+-r[fR]",
                r"(?i)\brmdir\b.*/s.*/q",
                r"\bdd\s+if=.*of=",
                r"(?i)\bformat\s+[A-Za-z]:",
                r"\bdiskpart\b",
                r"\bbcdedit\b",
                r"\bshutdown\b",
                r"\btaskkill\b",
                r"\brundll32\b",
                r"\bmshta\b",
                r"\b(sudo|su\b|doas)\b",
                r"(?i)\bchmod\b.*777",
                r"(?i)\bchown\b.*root",
                r"\bmkfs\.\w+",
                r"(?i)\breg\s+(add|delete)\b",
                r"\b(icacls|cacls)\s",
                r"\btakeown\b",
                r"\bsc\s+(stop|delete|config)\b",
                r"(?i)\bcertutil\b.*-urlcache",
                r"(?i)\bbitsadmin\b.*/transfer",
                r"(?i)-Ep\s+Bypass|-ExecutionPolicy\s+Bypass",
                r"(?i)\b(curl|wget)\b.*\|",
                r"(?i)(-EncodedCommand|-enc\s+[A-Za-z0-9+/=]{20,})",
                r"\$\([^)]+\)",
                r"(?i)\b(cat|less|head|tail|vi|nano|vim|cP|mv)\s+[^\s]*/(etc|var|usr|root|proc|sys|dev)/",
                r"[A-Za-z]:\\(Windows|Program Files|ProgramData)\b",
                r"(?i)%(WINDIR|SYSTEMROOT|APPDATA|TEMP|USERPROFILE)%",
                r"(?i)\bdel\b.*/f",
            ];

            for pattern in dangerous {
                if let Ok(re) = Regex::new(pattern) {
                    if re.is_match(cmd) {
                        return Err(PolicyError::CommandDenied(
                            cmd.to_string(),
                            pattern.to_string(),
                        ));
                    }
                }
            }
        }

        #[cfg(not(feature = "regex"))]
        {
            let dangerous: &[(&str, &str)] = &[
                ("rm -rf", "rm -rf"),
                ("rm -rf ", "rm -rf"),
                ("rm -r ", "rm -r"),
                ("rmdir /s", "rmdir /s"),
                ("format ", "format"),
                ("diskpart", "diskpart"),
                ("bcdedit", "bcdedit"),
                ("shutdown", "shutdown"),
                ("taskkill", "taskkill"),
                ("rundll32", "rundll32"),
                ("mshta", "mshta"),
                ("sudo ", "sudo "),
                ("su -", "su -"),
                ("doas ", "doas "),
                ("chmod ", "chmod "),
                ("chown ", "chown "),
                ("mkfs.", "mkfs"),
                ("reg add", "reg add"),
                ("reg delete", "reg delete"),
                ("icacls ", "icacls"),
                ("cacls ", "cacls"),
                ("takeown", "takeown"),
                ("sc stop", "sc stop"),
                ("sc delete", "sc delete"),
                ("sc config", "sc config"),
                ("certutil", "certutil"),
                ("bitsadmin", "bitsadmin"),
                ("-EncodedCommand", "-EncodedCommand"),
                ("Bypass", "Bypass"),
                ("\\Windows\\", "\\Windows\\"),
                ("%WINDIR%", "%WINDIR%"),
                ("%SYSTEMROOT%", "%SYSTEMROOT%"),
                ("%APPDATA%", "%APPDATA%"),
            ];

            for (pattern, _label) in dangerous {
                if cmd.contains(pattern) {
                    return Err(PolicyError::CommandDenied(
                        cmd.to_string(),
                        pattern.to_string(),
                    ));
                }
            }
        }

        Ok(())
    }
}

/// Errors returned by policy checks.
#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("tool '{0}' is denied by policy")]
    ToolDenied(String),

    #[error("tool '{0}' is not in the allowed list")]
    ToolNotAllowed(String),

    #[error("arguments exceed maximum size ({0} > {1} bytes)")]
    ArgsTooLarge(usize, usize),

    #[error("command '{0}' matches deny pattern '{1}'")]
    CommandDenied(String, String),

    #[error("command '{0}' is not in the allowed commands list")]
    CommandNotAllowed(String),

    #[error("timeout {0}s exceeds maximum {1}s")]
    TimeoutExceeded(f64, f64),

    #[error("workdir '{0}' is not allowed")]
    WorkdirDenied(String),

    #[error("path error: {0}")]
    PathDenied(String),
}

impl From<PolicyError> for crate::re_act::tool::ToolCallError {
    fn from(e: PolicyError) -> Self {
        crate::re_act::tool::ToolCallError::ToolUnavailable(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_allows_all() {
        let policy = ToolPolicy::default();
        assert!(policy.check_tool_allowed("any_tool").is_ok());
        assert!(policy.check_args(&JsonValue::Null).is_ok());
    }

    #[test]
    fn denied_tool_is_blocked() {
        let mut policy = ToolPolicy::default();
        policy.denied_tools.insert("danger".into());
        assert!(policy.check_tool_allowed("danger").is_err());
        assert!(policy.check_tool_allowed("safe").is_ok());
    }

    #[test]
    fn allowed_tools_restrict() {
        let mut allowed = HashSet::new();
        allowed.insert("safe".into());
        let policy = ToolPolicy {
            allowed_tools: Some(allowed),
            ..Default::default()
        };
        assert!(policy.check_tool_allowed("safe").is_ok());
        assert!(policy.check_tool_allowed("other").is_err());
    }

    #[test]
    fn args_size_limit() {
        let policy = ToolPolicy {
            max_args_size: 10,
            ..Default::default()
        };
        let small = serde_json::json!({"a": 1});
        assert!(policy.check_args(&small).is_ok());
        let large = serde_json::json!({"data": "x".repeat(100)});
        assert!(policy.check_args(&large).is_err());
    }

    #[test]
    fn shell_policy_blocks_dangerous() {
        let policy = ShellPolicy::strict();
        assert!(policy.check_command("rm -rf /").is_err());
        assert!(policy.check_command("sudo rm -rf").is_err());
        assert!(policy.check_command("ls -la").is_ok());
    }

    #[test]
    fn shell_policy_allow_commands() {
        let policy = ShellPolicy::with_allowed(vec!["git".into(), "cargo".into()]);
        assert!(policy.check_command("git status").is_ok());
        assert!(policy.check_command("cargo build").is_ok());
        assert!(policy.check_command("rm -rf /").is_err());
    }

    #[test]
    fn shell_policy_custom_deny() {
        let policy = ShellPolicy {
            deny_patterns: vec!["secret_file".into()],
            ..ShellPolicy::permissive()
        };
        assert!(policy.check_command("cat /tmp/secret_file").is_err());
        assert!(policy.check_command("echo hello").is_ok());
    }

    #[cfg(feature = "regex")]
    #[test]
    fn shell_policy_regex_deny() {
        let policy = ShellPolicy {
            deny_patterns: vec![r"rm\s+-rf".into()],
            ..ShellPolicy::permissive()
        };
        assert!(policy.check_command("rm -rf /").is_err());
        assert!(policy.check_command("rm -r dir").is_ok());
    }
}
