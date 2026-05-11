//! Workspace path trust boundary.
//!
//! `WorkspaceGuard` wraps a canonicalized workspace root. Untrusted relative
//! paths are validated via `resolve()` which checks:
//!   1. No NUL bytes
//!   2. Not absolute
//!   3. No `..` components
//!   4. Resolved parent is inside workspace_root (canonical comparison)
//!   5. No ancestor on the resolved path is a symlink (defeats symlink-bait)
//!
//! Returns `WorkspacePath` — a newtype that exposes only `.as_path()`,
//! providing compile-time assurance the path is workspace-confined.

use std::path::{Component, Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("path contains NUL byte")]
    NulByte,
    #[error("path must be relative, got absolute path")]
    Absolute,
    #[error("path contains '..' component")]
    ParentDir,
    #[error("path escapes workspace root: resolved to {0}")]
    OutsideRoot(PathBuf),
    #[error("path or ancestor is a symlink: {0}")]
    SymlinkAncestor(PathBuf),
    #[error("io error resolving path: {0}")]
    Io(#[from] std::io::Error),
}

/// A guard rooted at a canonical workspace path.
#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    root_canonical: PathBuf,
}

/// A validated workspace-confined path. Constructible only via `WorkspaceGuard::resolve`.
#[derive(Debug, Clone)]
pub struct WorkspacePath(PathBuf);

impl WorkspacePath {
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl WorkspaceGuard {
    /// Build a guard rooted at `workspace_root` (canonicalized).
    pub fn new(workspace_root: &Path) -> Result<Self, GuardError> {
        let canonical = workspace_root.canonicalize()?;
        Ok(Self {
            root_canonical: canonical,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root_canonical
    }

    /// Validate `untrusted` (a relative path string) and return a `WorkspacePath`.
    /// Used for paths that may not exist yet (creation).
    pub fn resolve(&self, untrusted: &str) -> Result<WorkspacePath, GuardError> {
        if untrusted.contains('\0') {
            return Err(GuardError::NulByte);
        }
        let p = Path::new(untrusted);
        if p.is_absolute() {
            return Err(GuardError::Absolute);
        }
        for comp in p.components() {
            if matches!(comp, Component::ParentDir) {
                return Err(GuardError::ParentDir);
            }
        }

        // Walk components left-to-right starting from root_canonical.
        // For every existing component on the path, check symlink_metadata and reject
        // if it is a symlink. This catches symlink-bait regardless of whether the
        // symlink target is inside or outside the workspace.
        let mut accumulated = self.root_canonical.clone();
        for comp in p.components() {
            accumulated.push(comp);
            match std::fs::symlink_metadata(&accumulated) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(GuardError::SymlinkAncestor(accumulated));
                }
                Ok(_) => continue,
                Err(_) => break, // leaf or intermediate doesn't exist yet — fine
            }
        }

        let joined = self.root_canonical.join(p);

        // Defense-in-depth: walk up to first existing ancestor, canonicalize, verify
        // the canonical form stays inside root_canonical.
        let mut probe = joined.clone();
        let parent = loop {
            if probe.exists() {
                break probe.clone();
            }
            match probe.parent() {
                Some(par) if par != probe.as_path() => probe = par.to_path_buf(),
                _ => break self.root_canonical.clone(),
            }
        };
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(&self.root_canonical) {
            return Err(GuardError::OutsideRoot(canonical_parent));
        }
        Ok(WorkspacePath(joined))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup() -> (tempfile::TempDir, WorkspaceGuard) {
        let dir = tempdir().unwrap();
        let guard = WorkspaceGuard::new(dir.path()).unwrap();
        (dir, guard)
    }

    #[test]
    fn rejects_absolute() {
        let (_dir, g) = setup();
        assert!(matches!(
            g.resolve("/etc/passwd"),
            Err(GuardError::Absolute)
        ));
    }

    #[test]
    fn rejects_parent_dir() {
        let (_dir, g) = setup();
        assert!(matches!(g.resolve("../foo"), Err(GuardError::ParentDir)));
        assert!(matches!(
            g.resolve("a/../../etc"),
            Err(GuardError::ParentDir)
        ));
    }

    #[test]
    fn rejects_nul_byte() {
        let (_dir, g) = setup();
        assert!(matches!(g.resolve("a\0b"), Err(GuardError::NulByte)));
    }

    #[test]
    fn accepts_simple_relative() {
        let (_dir, g) = setup();
        let r = g.resolve("src/lib.rs").unwrap();
        assert!(r.as_path().ends_with("src/lib.rs"));
    }

    #[test]
    fn rejects_symlink_ancestor() {
        let (dir, g) = setup();
        // Create a symlink inside the workspace pointing OUTSIDE.
        let evil = dir.path().join("evil");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", &evil).unwrap();
        #[cfg(not(unix))]
        return;
        // Try to write through it: evil/passwd. Should reject because `evil` IS a symlink.
        assert!(matches!(
            g.resolve("evil/passwd"),
            Err(GuardError::SymlinkAncestor(_))
        ));
    }
}
