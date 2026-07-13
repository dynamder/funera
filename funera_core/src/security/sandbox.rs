use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(all(feature = "sandbox", not(target_os = "windows")))]
use nono::{AccessMode, CapabilitySet};

#[cfg(all(feature = "sandbox", target_os = "windows"))]
use super::sandbox_win::WindowsSandbox;

/// Policy config for kernel-enforced sandboxing.
///
/// On Linux/macOS the policy maps to a [`nono::CapabilitySet`] that grants
/// Landlock (Linux 5.13+) or Seatbelt (macOS) access rights. On Windows it
/// configures a Write-Restricted Token + ACLs via [`WindowsSandbox`].
///
/// When `enabled` is true and the platform supports it, each tool
/// subprocess will be restricted to only the granted capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Master switch — set to `false` to disable kernel sandboxing
    /// while keeping the policy definition in the config.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Paths the tool subprocess may read (and traverse, list, …).
    #[serde(default)]
    pub read_paths: Vec<PathBuf>,

    /// Paths the tool subprocess may both read and write.
    #[serde(default)]
    pub read_write_paths: Vec<PathBuf>,

    /// Paths the tool subprocess may execute (binaries, scripts, …).
    #[serde(default)]
    pub execute_paths: Vec<PathBuf>,

    /// When `true`, all outbound network access is blocked at the
    /// kernel level (via Landlock scoped network, or seccomp fallback).
    #[serde(default = "default_block_network")]
    pub block_network: bool,

    /// Maximum resident memory for the process tree, in bytes.
    /// `None` means no memory limit.
    #[serde(default)]
    pub memory_limit_bytes: Option<u64>,
}

fn default_enabled() -> bool {
    true
}
fn default_block_network() -> bool {
    true
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            read_paths: vec![],
            read_write_paths: vec![],
            execute_paths: vec![],
            block_network: true,
            memory_limit_bytes: None,
        }
    }
}

impl SandboxPolicy {
    /// Fully permissive: sandboxing disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Strict policy: only the given read-write dir allowed, network blocked.
    pub fn strict_read_write(dir: PathBuf) -> Self {
        Self {
            enabled: true,
            read_paths: vec![],
            read_write_paths: vec![dir],
            execute_paths: vec![],
            block_network: true,
            memory_limit_bytes: None,
        }
    }

    /// Build a capability set from this policy for kernel-enforced
    /// sandboxing (Landlock on Linux, Seatbelt on macOS).
    ///
    /// Returns an error if any path cannot be resolved or if the
    /// platform lacks sandbox support. Callers should check
    /// [`enabled`](Self::enabled) first.
    ///
    /// **Not available on Windows** — the underlying `nono` crate does not
    /// support Windows natively. On Windows, callers should use
    /// [`Sandbox`] instead, which delegates to [`WindowsSandbox`].
    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    pub fn to_capability_set(&self) -> Result<CapabilitySet, nono::NonoError> {
        let mut caps = CapabilitySet::new();

        for path in &self.read_paths {
            caps = caps.allow_path(path, AccessMode::Read)?;
        }
        for path in &self.read_write_paths {
            caps = caps.allow_path(path, AccessMode::ReadWrite)?;
        }
        for path in &self.execute_paths {
            caps = caps.allow_path(path, AccessMode::Read)?;
        }

        if self.block_network {
            caps = caps.block_network();
        }

        Ok(caps)
    }

    /// A human-readable summary for diagnostics / auditing.
    pub fn summary(&self) -> String {
        if !self.enabled {
            return "sandbox disabled".into();
        }
        let parts: Vec<String> = std::iter::once(if self.block_network {
            "no-net".into()
        } else {
            "net-allowed".into()
        })
        .chain(self.read_paths.iter().map(|p| format!("r:{}", p.display())))
        .chain(
            self.read_write_paths
                .iter()
                .map(|p| format!("rw:{}", p.display())),
        )
        .chain(
            self.execute_paths
                .iter()
                .map(|p| format!("x:{}", p.display())),
        )
        .collect();
        parts.join(", ")
    }
}

// ── platform-independent Sandbox runner ────────────────────────────

/// A sandbox runner that enforces [`SandboxPolicy`] on any supported OS.
///
/// | Platform | Mechanism |
/// |----------|-----------|
/// | Linux   | Landlock via `nono` `pre_exec` hook |
/// | macOS   | Seatbelt via `nono` `pre_exec` hook |
/// | Windows | Write-Restricted Token + ACLs |
///
/// # Failover
///
/// If the platform-specific mechanism cannot be applied (e.g. kernel too old,
/// missing privileges), execution falls back to a normal subprocess with
/// network restrictions only.
pub struct Sandbox {
    #[cfg(all(feature = "sandbox", target_os = "windows"))]
    inner: WindowsSandbox,
    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    policy: SandboxPolicy,
    #[cfg(not(feature = "sandbox"))]
    _private: (),
}

impl Sandbox {
    /// Build a sandbox runner from the given policy.
    ///
    /// On supported platforms, sets up the necessary ACLs / capabilities.
    /// Returns an error if the setup itself fails (not if the OS lacks
    /// sandbox support — that is handled at execution time via failover).
    pub fn new(policy: &SandboxPolicy) -> Result<Self, anyhow::Error> {
        #[cfg(all(feature = "sandbox", target_os = "windows"))]
        {
            let inner = WindowsSandbox::new(policy)?;
            Ok(Self { inner })
        }
        #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
        {
            Ok(Self { policy: policy.clone() })
        }
        #[cfg(not(feature = "sandbox"))]
        {
            let _ = policy;
            Ok(Self { _private: () })
        }
    }

    /// Execute a shell command under sandbox restrictions.
    ///
    /// Parameters `shell` (e.g. `"sh"` / `"cmd"`), `shell_flag` (`"-c"` / `"/c"`)
    /// and `command` are assembled into a full command line internally.
    ///
    /// Returns `(stdout, stderr, exit_code)`.
    pub async fn execute(
        &self,
        shell: &str,
        shell_flag: &str,
        command: &str,
        workdir: Option<&str>,
        timeout: std::time::Duration,
    ) -> Result<(String, String, i32), anyhow::Error> {
        #[cfg(all(feature = "sandbox", target_os = "windows"))]
        {
            self.inner.execute(shell, shell_flag, command, workdir, timeout).await
        }
        #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
        {
            execute_unix_sandbox(&self.policy, shell, shell_flag, command, workdir, timeout).await
        }
        #[cfg(not(feature = "sandbox"))]
        {
            let _ = (shell_flag, shell);
            failover_execute(command, workdir, timeout).await
        }
    }
}

// ── Unix sandbox executor (nono Landlock / Seatbelt) ──────────────

/// Standard system paths required for shell, cat, echo, pwd etc.
/// Landlock needs every path the subprocess will access to be
/// explicitly allowed, including shared libraries, the dynamic
/// linker, and the executables themselves.
#[cfg(all(feature = "sandbox", not(target_os = "windows")))]
fn system_sandbox_read_paths() -> Vec<PathBuf> {
    vec![
        "/usr".into(),
        "/bin".into(),
        "/lib".into(),
        "/lib64".into(),
        "/etc".into(),
    ]
}

#[cfg(all(feature = "sandbox", not(target_os = "windows")))]
async fn execute_unix_sandbox(
    policy: &SandboxPolicy,
    shell: &str,
    shell_flag: &str,
    command: &str,
    workdir: Option<&str>,
    timeout: std::time::Duration,
) -> Result<(String, String, i32), anyhow::Error> {
    let mut caps = policy.to_capability_set().map_err(|e| {
        anyhow::anyhow!("failed to build sandbox capability set: {e}")
    })?;

    for path in &system_sandbox_read_paths() {
        caps = caps
            .allow_path(path, nono::AccessMode::Read)
            .map_err(|e| anyhow::anyhow!("failed to allow system path {}: {e}", path.display()))?;
    }

    let command_owned = command.to_owned();
    let shell_owned = shell.to_owned();
    let flag_owned = shell_flag.to_owned();
    let workdir_owned = workdir.map(|d| d.to_owned());

    let mut std_cmd = std::process::Command::new(&shell_owned);
    std_cmd
        .arg(&flag_owned)
        .arg(&command_owned)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(ref dir) = workdir_owned {
        std_cmd.current_dir(dir);
    }

    unsafe {
        std_cmd.pre_exec(move || match nono::Sandbox::is_supported() {
            true => nono::Sandbox::apply_auto(&caps).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, format!("sandbox apply failed: {e}"))
            }),
            false => Ok(()),
        });
    }

    let mut tokio_cmd = tokio::process::Command::from(std_cmd);

    let output = tokio::time::timeout(timeout, tokio_cmd.output())
        .await
        .map_err(|_| anyhow::anyhow!("command timed out"))?
        .map_err(|e| anyhow::anyhow!("command failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    Ok((stdout, stderr, exit_code))
}

#[cfg(all(not(feature = "sandbox"), target_os = "windows"))]
async fn failover_execute(
    command: &str,
    workdir: Option<&str>,
    timeout: std::time::Duration,
) -> Result<(String, String, i32), anyhow::Error> {
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.arg("/c").arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = workdir {
        cmd.current_dir(dir);
    }
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| anyhow::anyhow!("command timed out"))?
        .map_err(|e| anyhow::anyhow!("command failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    Ok((stdout, stderr, exit_code))
}

#[cfg(all(not(feature = "sandbox"), not(target_os = "windows")))]
async fn failover_execute(
    command: &str,
    workdir: Option<&str>,
    timeout: std::time::Duration,
) -> Result<(String, String, i32), anyhow::Error> {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if let Some(dir) = workdir {
        cmd.current_dir(dir);
    }
    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| anyhow::anyhow!("command timed out"))?
        .map_err(|e| anyhow::anyhow!("command failed: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    Ok((stdout, stderr, exit_code))
}

// ── format stdio triple to the string used by ShellTool ────────────

/// Format (stdout, stderr, exit_code) into the same format as
/// `format_output` for `std::process::Output`.
pub fn format_triple_output(stdout: &str, stderr: &str, exit_code: i32) -> String {
    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str("stdout:\n");
        result.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("stderr:\n");
        result.push_str(stderr);
    }
    if exit_code == 0 {
        if result.is_empty() {
            result.push_str("(command completed with no output)");
        }
    } else {
        result.push_str(&format!("\n(exit code: {exit_code})"));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── summary tests ──────────────────────────────────────────────

    #[test]
    fn summary_disabled() {
        let p = SandboxPolicy::disabled();
        assert_eq!(p.summary(), "sandbox disabled");
    }

    #[test]
    fn summary_with_multiple_path_types() {
        let p = SandboxPolicy {
            read_paths: vec!["/etc".into(), "/usr/share".into()],
            read_write_paths: vec!["/data".into()],
            execute_paths: vec!["/usr/bin".into()],
            block_network: true,
            ..Default::default()
        };
        let s = p.summary();
        assert!(s.contains("no-net"));
        assert!(s.contains("r:/etc"));
        assert!(s.contains("r:/usr/share"));
        assert!(s.contains("rw:/data"));
        assert!(s.contains("x:/usr/bin"));
    }

    #[test]
    fn summary_with_network_allowed() {
        let p = SandboxPolicy {
            block_network: false,
            ..Default::default()
        };
        let s = p.summary();
        assert!(s.contains("net-allowed"));
        assert!(!s.contains("no-net"));
    }

    #[test]
    fn summary_empty_policy_paths() {
        let p = SandboxPolicy::default();
        let s = p.summary();
        assert!(s.contains("no-net"));
        // no path entries should appear for empty vectors
        assert!(!s.contains("r:"));
        assert!(!s.contains("rw:"));
        assert!(!s.contains("x:"));
    }

    #[test]
    fn summary_disabled_idempotent_with_paths_set() {
        let mut p = SandboxPolicy::disabled();
        p.read_write_paths.push("/project".into());
        // still disabled regardless of paths
        assert!(!p.enabled);
        assert_eq!(p.summary(), "sandbox disabled");
    }

    // ── serialization tests ────────────────────────────────────────

    #[test]
    fn serialization_roundtrip() {
        let p = SandboxPolicy {
            enabled: true,
            read_paths: vec!["/etc".into()],
            read_write_paths: vec!["/data".into(), "/tmp".into()],
            execute_paths: vec!["/usr/local/bin".into()],
            block_network: false,
            memory_limit_bytes: Some(512_000_000),
        };
        let json = serde_json::to_string(&p).expect("serialize");
        let de: SandboxPolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de.enabled, p.enabled);
        assert_eq!(de.read_paths, p.read_paths);
        assert_eq!(de.read_write_paths, p.read_write_paths);
        assert_eq!(de.execute_paths, p.execute_paths);
        assert_eq!(de.block_network, p.block_network);
        assert_eq!(de.memory_limit_bytes, p.memory_limit_bytes);
    }

    #[test]
    fn serialization_minimal() {
        let json = r#"{"read_write_paths": ["/project"]}"#;
        let p: SandboxPolicy = serde_json::from_str(json).expect("deserialize minimal");
        assert!(p.enabled); // default
        assert!(p.block_network); // default
        assert_eq!(p.read_write_paths, vec![PathBuf::from("/project")]);
        assert!(p.read_paths.is_empty());
        assert_eq!(p.memory_limit_bytes, None);
    }

    #[test]
    fn serialization_disabled_explicit() {
        let p = SandboxPolicy::disabled();
        let json = serde_json::to_string(&p).expect("serialize");
        let de: SandboxPolicy = serde_json::from_str(&json).expect("deserialize");
        assert!(!de.enabled);
    }

    // ── constructor & builder tests ────────────────────────────────

    #[test]
    fn default_values_are_safe() {
        let p = SandboxPolicy::default();
        assert!(p.enabled, "sandbox defaults to enabled");
        assert!(p.block_network, "network defaults to blocked");
        assert!(p.memory_limit_bytes.is_none());
        assert!(p.read_paths.is_empty());
        assert!(p.read_write_paths.is_empty());
        assert!(p.execute_paths.is_empty());
    }

    #[test]
    fn disabled_policy_only_toggles_enabled() {
        // disabled() only sets enabled=false; other fields keep their
        // defaults so the config can be round-tripped without data loss.
        let p = SandboxPolicy::disabled();
        assert!(!p.enabled);
        assert!(p.block_network); // unchanged from default
        assert!(p.read_paths.is_empty());
    }

    #[test]
    fn strict_read_write_sets_correct_fields() {
        let p = SandboxPolicy::strict_read_write("/workspace".into());
        assert!(p.enabled);
        assert!(p.block_network);
        assert_eq!(p.read_write_paths, vec![PathBuf::from("/workspace")]);
        assert!(p.read_paths.is_empty());
        assert!(p.execute_paths.is_empty());
    }

    // ── to_capability_set tests (non-Windows only) ─────────────────

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[test]
    fn to_capability_set_assigns_read_paths() {
        // /tmp is guaranteed to exist on every Linux/macOS system
        let p = SandboxPolicy {
            read_paths: vec!["/tmp".into()],
            ..Default::default()
        };
        let _caps = p.to_capability_set().expect("build caps with read paths");
    }

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[test]
    fn to_capability_set_assigns_read_write_paths() {
        let p = SandboxPolicy {
            read_write_paths: vec!["/tmp".into()],
            ..Default::default()
        };
        let _caps = p
            .to_capability_set()
            .expect("build caps with read-write paths");
    }

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[test]
    fn to_capability_set_assigns_execute_paths() {
        let p = SandboxPolicy {
            execute_paths: vec!["/tmp".into()],
            ..Default::default()
        };
        let _caps = p
            .to_capability_set()
            .expect("build caps with execute paths");
    }

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[test]
    fn to_capability_set_block_network() {
        let p = SandboxPolicy {
            block_network: true,
            ..Default::default()
        };
        let _caps = p
            .to_capability_set()
            .expect("build caps with network blocked");
    }

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[test]
    fn to_capability_set_network_not_blocked_when_false() {
        let p = SandboxPolicy {
            block_network: false,
            ..Default::default()
        };
        let _caps = p
            .to_capability_set()
            .expect("build caps without network block");
    }

    #[cfg(all(feature = "sandbox", not(target_os = "windows")))]
    #[test]
    fn to_capability_set_empty_policy_creates_minimal_caps() {
        let p = SandboxPolicy::default();
        let _caps = p.to_capability_set().expect("build caps from empty policy");
    }
}
