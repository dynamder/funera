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
}
