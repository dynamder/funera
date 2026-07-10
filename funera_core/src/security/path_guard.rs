use std::collections::HashSet;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct PathGuard {
    allowed_roots: HashSet<PathBuf>,
}

impl PathGuard {
    pub fn new(roots: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        let allowed_roots: HashSet<PathBuf> = roots
            .into_iter()
            .map(|r| {
                let p = r.into();
                p.canonicalize().unwrap_or(p)
            })
            .collect();
        Self { allowed_roots }
    }

    pub fn add_root(&mut self, root: impl Into<PathBuf>) {
        let p = root.into();
        let canonical = p.canonicalize().unwrap_or(p);
        self.allowed_roots.insert(canonical);
    }

    pub fn verify(&self, path: &Path) -> Result<PathBuf, PathError> {
        if !path.exists() {
            return Err(PathError::NotExist {
                path: path.display().to_string(),
            });
        }

        let canonical = path.canonicalize().map_err(|e| PathError::ResolveError {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;

        let within = self
            .allowed_roots
            .iter()
            .any(|root| canonical.starts_with(root));

        if !within {
            return Err(PathError::OutsideScope {
                path: canonical.display().to_string(),
                allowed: self
                    .allowed_roots
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
        }

        Ok(canonical)
    }

    pub fn is_path_allowed(&self, path: &Path) -> bool {
        self.verify(path).is_ok()
    }

    pub fn allowed_roots(&self) -> &HashSet<PathBuf> {
        &self.allowed_roots
    }
}

impl<T: Into<PathBuf>> From<T> for PathGuard {
    fn from(root: T) -> Self {
        Self::new([root.into()])
    }
}

#[derive(Debug, Error)]
pub enum PathError {
    #[error("path '{path}' is outside allowed scope (allowed: {allowed})")]
    OutsideScope { path: String, allowed: String },

    #[error("path '{path}' does not exist")]
    NotExist { path: String },

    #[error("cannot resolve path '{path}': {reason}")]
    ResolveError { path: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_within_root() {
        let guard = PathGuard::new(["."]);
        assert!(guard.verify(Path::new("Cargo.toml")).is_ok());
    }

    #[test]
    fn path_not_exist() {
        let guard = PathGuard::new(["."]);
        let result = guard.verify(Path::new("nonexistent_file_xyz"));
        assert!(result.is_err());
        match result.unwrap_err() {
            PathError::NotExist { .. } => {}
            e => panic!("expected NotExist, got {e:?}"),
        }
    }

    #[test]
    fn custom_root() {
        let guard = PathGuard::new([std::env::current_dir().unwrap()]);
        assert!(guard.verify(Path::new("Cargo.toml")).is_ok());
    }

    #[test]
    fn path_outside_root_is_blocked() {
        let guard = PathGuard::new(["."]);
        // Try to reference a file outside the current directory
        let parent = Path::new("..").join("Cargo.lock");
        let result = guard.verify(&parent);
        assert!(
            result.is_err(),
            "paths outside allowed root should be blocked"
        );
        match result.unwrap_err() {
            PathError::OutsideScope { .. } => {}
            e => panic!("expected OutsideScope, got {e:?}"),
        }
    }

    #[test]
    fn path_guard_multiple_roots() {
        let guard = PathGuard::new([".", "src"]);
        assert!(guard.verify(Path::new("Cargo.toml")).is_ok());
        assert!(guard.verify(Path::new("src/lib.rs")).is_ok());
    }

    #[test]
    fn path_guard_is_path_allowed() {
        let guard = PathGuard::new(["."]);
        assert!(guard.is_path_allowed(Path::new("Cargo.toml")));
        assert!(!guard.is_path_allowed(Path::new("nonexistent_file_xyz")));
    }

    #[test]
    fn path_guard_add_root_increases_scope() {
        let mut guard = PathGuard::new(["."]);
        let file_outside = Path::new("..").join("Cargo.lock");
        assert!(
            guard.verify(&file_outside).is_err(),
            "outside root should fail"
        );

        // Adding parent as a root should allow it
        guard.add_root("..");
        assert!(
            guard.verify(&file_outside).is_ok(),
            "after adding parent root, should be allowed"
        );
    }

    #[test]
    fn path_guard_from_trait() {
        let guard: PathGuard = ".".into();
        assert!(guard.verify(Path::new("Cargo.toml")).is_ok());
    }

    #[test]
    fn path_guard_blocked_path_has_expected_error() {
        let guard = PathGuard::new(["."]);
        let result = guard.verify(Path::new("nonexistent_file_xyz"));
        match result {
            Err(PathError::NotExist { .. }) => {}
            _ => panic!("expected NotExist error for nonexistent path"),
        }
    }

    #[test]
    fn path_guard_allowed_roots_accessible() {
        let guard = PathGuard::new([".", "src"]);
        let roots = guard.allowed_roots();
        assert_eq!(roots.len(), 2);
    }
}
