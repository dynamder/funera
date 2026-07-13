use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(all(feature = "sandbox", not(target_os = "windows")))]
use nono::{AccessMode, CapabilitySet};

/// Policy config for kernel-enforced sandboxing via nono.
///
/// Maps to [`nono::CapabilitySet`] at the moment of application.
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
    /// **Not available on Windows** — the underlying `nono` crate
    /// does not support Windows natively. Use WSL2 for sandboxed
    /// tool execution on Windows.
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
