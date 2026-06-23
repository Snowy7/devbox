//! Git adapter boundary for Bindhub.
//!
//! This crate will own Git-specific inspection and restore behavior so `.git`
//! directories are never treated as ordinary synced files.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepositoryStatus {
    pub worktree_root: String,
    pub head: Option<String>,
    pub has_uncommitted_changes: bool,
}

impl GitRepositoryStatus {
    pub fn placeholder(worktree_root: impl Into<String>) -> Self {
        Self {
            worktree_root: worktree_root.into(),
            head: None,
            has_uncommitted_changes: false,
        }
    }
}
