use std::path::PathBuf;

use super::path_guard::PathGuard;

/// Result of a boundary check: whether an operation on some paths
/// is allowed, requires user approval, or is outright rejected.
#[derive(Debug, Clone, PartialEq)]
pub enum BoundaryDecision {
    /// Paths are inside the trusted zone (PathGuard) — proceed.
    AutoApproved,
    /// Paths are inside the sandbox but outside the trusted zone —
    /// requires user approval to proceed.
    RequiresApproval {
        tool_name: String,
        paths: Vec<PathBuf>,
        reason: String,
    },
    /// Paths are outside the sandbox boundary — rejected.
    Rejected {
        tool_name: String,
        paths: Vec<PathBuf>,
        reason: String,
    },
}

/// Check whether the given paths cross any security boundary.
///
/// Three-tier model:
/// 1. **Inside PathGuard**  → `AutoApproved`
/// 2. **Inside sandbox boundary** (if enabled) → `RequiresApproval`
/// 3. **Outside sandbox boundary** → `Rejected`
///
/// When the `sandbox` feature is disabled there is no outer fence,
/// so everything outside PathGuard becomes `RequiresApproval`.
///
/// ## `is_within_boundary` (sandbox feature only)
///
/// A closure that returns `true` if the path is within the sandbox
/// perimeter. This is only compiled when `feature = "sandbox"` and
/// should be provided by the caller (typically by checking against
/// `SandboxPolicy::read_paths` + `read_write_paths`).
#[cfg(feature = "sandbox")]
pub fn check_boundary(
    tool_name: &str,
    paths: &[PathBuf],
    path_guard: Option<&PathGuard>,
    sandbox_enabled: bool,
    is_within_boundary: impl Fn(&PathBuf) -> bool,
) -> BoundaryDecision {
    // 1. PathGuard zone → auto-approved
    if let Some(guard) = path_guard
        && paths.iter().all(|p| guard.verify(p).is_ok())
    {
        return BoundaryDecision::AutoApproved;
    }
    if path_guard.is_none() && paths.is_empty() {
        return BoundaryDecision::AutoApproved;
    }

    // 2. Sandbox outer boundary check
    if sandbox_enabled {
        for path in paths {
            if !is_within_boundary(path) {
                return BoundaryDecision::Rejected {
                    tool_name: tool_name.to_string(),
                    paths: paths.to_vec(),
                    reason: format!("path {} is outside the sandbox boundary", path.display()),
                };
            }
        }
        return BoundaryDecision::RequiresApproval {
            tool_name: tool_name.to_string(),
            paths: paths.to_vec(),
            reason: "paths inside sandbox but outside the trusted zone".into(),
        };
    }

    // 3. No sandbox boundary → everything outside PathGuard needs approval
    BoundaryDecision::RequiresApproval {
        tool_name: tool_name.to_string(),
        paths: paths.to_vec(),
        reason: "paths outside the trusted zone".into(),
    }
}

/// Boundary check when the sandbox feature is disabled.
/// Everything outside PathGuard becomes `RequiresApproval`.
#[cfg(not(feature = "sandbox"))]
pub fn check_boundary(
    tool_name: &str,
    paths: &[PathBuf],
    path_guard: Option<&PathGuard>,
) -> BoundaryDecision {
    if let Some(guard) = path_guard
        && paths.iter().all(|p| guard.verify(p).is_ok())
    {
        return BoundaryDecision::AutoApproved;
    }
    if path_guard.is_none() && paths.is_empty() {
        return BoundaryDecision::AutoApproved;
    }
    BoundaryDecision::RequiresApproval {
        tool_name: tool_name.to_string(),
        paths: paths.to_vec(),
        reason: "paths outside the trusted zone".into(),
    }
}
