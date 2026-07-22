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

/// Three-tier model:
/// 1. **Inside PathGuard**  → `AutoApproved`
/// 2. **Inside sandbox boundary** (if enabled) → `RequiresApproval`
/// 3. **Outside sandbox boundary** → `Rejected`
///
/// When the `sandbox` feature is disabled there is no outer fence,
/// so everything outside PathGuard becomes `RequiresApproval`.
#[cfg(feature = "sandbox")]
pub fn check_boundary(
    tool_name: &str,
    paths: &[PathBuf],
    path_guard: Option<&PathGuard>,
    sandbox_enabled: bool,
    is_within_boundary: impl Fn(&PathBuf) -> bool,
) -> BoundaryDecision {
    if let Some(guard) = path_guard
        && paths.iter().all(|p| guard.verify(p).is_ok())
    {
        return BoundaryDecision::AutoApproved;
    }
    if path_guard.is_none() && paths.is_empty() {
        return BoundaryDecision::AutoApproved;
    }

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

    BoundaryDecision::RequiresApproval {
        tool_name: tool_name.to_string(),
        paths: paths.to_vec(),
        reason: "paths outside the trusted zone".into(),
    }
}

/// No sandbox boundary — everything outside PathGuard becomes `RequiresApproval`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn guard_with_root(root: &str) -> PathGuard {
        PathGuard::new([root])
    }

    fn always_inside(_: &PathBuf) -> bool {
        true
    }

    fn never_inside(_: &PathBuf) -> bool {
        false
    }

    #[cfg(feature = "sandbox")]
    mod sandbox_on {
        use super::*;

        #[test]
        fn auto_approved_empty_paths_no_guard() {
            let d = check_boundary("tool", &[], None, true, always_inside);
            assert_eq!(d, BoundaryDecision::AutoApproved);
        }

        #[test]
        fn auto_approved_within_pathguard() {
            let guard = guard_with_root(".");
            let d = check_boundary(
                "tool",
                &[PathBuf::from("Cargo.toml")],
                Some(&guard),
                true,
                always_inside,
            );
            assert_eq!(d, BoundaryDecision::AutoApproved);
        }

        #[test]
        fn auto_approved_sandbox_disabled_in_guard() {
            let guard = guard_with_root(".");
            let d = check_boundary(
                "tool",
                &[PathBuf::from("Cargo.toml")],
                Some(&guard),
                false,
                always_inside,
            );
            assert_eq!(d, BoundaryDecision::AutoApproved);
        }

        #[test]
        fn requires_approval_outside_guard_sandbox_off() {
            let guard = guard_with_root("src");
            let d = check_boundary(
                "tool",
                &[PathBuf::from("/nonexistent")],
                Some(&guard),
                false,
                always_inside,
            );
            assert!(matches!(d, BoundaryDecision::RequiresApproval { .. }));
        }

        #[test]
        fn requires_approval_inside_sandbox_outside_guard() {
            let guard = guard_with_root("src");
            let d = check_boundary(
                "tool",
                &[PathBuf::from("/nonexistent")],
                Some(&guard),
                true,
                always_inside,
            );
            assert!(matches!(d, BoundaryDecision::RequiresApproval { .. }));
        }

        #[test]
        fn rejected_outside_sandbox() {
            let guard = guard_with_root(".");
            let d = check_boundary(
                "tool",
                &[PathBuf::from("/etc")],
                Some(&guard),
                true,
                never_inside,
            );
            assert!(matches!(d, BoundaryDecision::Rejected { .. }));
        }

        #[test]
        fn rejected_multiple_paths_one_outside() {
            let guard = guard_with_root(".");
            let d = check_boundary(
                "tool",
                &[PathBuf::from("Cargo.toml"), PathBuf::from("/etc")],
                Some(&guard),
                true,
                |p| p.to_string_lossy().contains("Cargo"),
            );
            assert!(matches!(d, BoundaryDecision::Rejected { .. }));
        }
    }

    #[cfg(not(feature = "sandbox"))]
    mod sandbox_off {
        use super::*;

        #[test]
        fn no_sandbox_auto_approved_in_guard() {
            let guard = guard_with_root(".");
            let d = check_boundary("tool", &[PathBuf::from("Cargo.toml")], Some(&guard));
            assert_eq!(d, BoundaryDecision::AutoApproved);
        }

        #[test]
        fn no_sandbox_requires_approval_outside_guard() {
            let guard = guard_with_root("src");
            let d = check_boundary("tool", &[PathBuf::from("/nonexistent")], Some(&guard));
            assert!(matches!(d, BoundaryDecision::RequiresApproval { .. }));
        }

        #[test]
        fn no_sandbox_empty_paths_no_guard() {
            let d = check_boundary("tool", &[], None);
            assert_eq!(d, BoundaryDecision::AutoApproved);
        }

        #[test]
        fn no_sandbox_outside_guard_no_guard() {
            let d = check_boundary("tool", &[PathBuf::from("/etc")], None);
            assert!(matches!(d, BoundaryDecision::RequiresApproval { .. }));
        }
    }
}
