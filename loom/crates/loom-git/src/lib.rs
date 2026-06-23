//! Git compatibility analyzer boundary for Loom.
//!
//! Git is folder context. Loom must protect `.git` metadata instead of treating
//! it as ordinary shared-folder content.

use loom_core::SharedFolderId;
use std::path::PathBuf;

pub const PROTECTED_GIT_DIR: &str = ".git";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCompatibilityReport {
    pub shared_folder_id: SharedFolderId,
    pub worktree_root: PathBuf,
    pub repository_detected: bool,
    pub git_metadata_protected: bool,
}

impl GitCompatibilityReport {
    pub fn no_repository(
        shared_folder_id: SharedFolderId,
        worktree_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            shared_folder_id,
            worktree_root: worktree_root.into(),
            repository_detected: false,
            git_metadata_protected: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_reports_keep_metadata_protected() {
        let report = GitCompatibilityReport::no_repository(
            SharedFolderId::new("folder-bindhub").expect("folder id"),
            "/workspace/bindhub",
        );

        assert!(report.git_metadata_protected);
        assert_eq!(PROTECTED_GIT_DIR, ".git");
    }
}
