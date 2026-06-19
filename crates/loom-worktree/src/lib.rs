//! Shared-folder worktree boundary for Loom.
//!
//! Scanning, generated-file policy, materialization, restore safety, and
//! file-version capture belong here as the old snapshot crate is migrated.

use loom_core::{RevisionBoundary, SharedFolder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRequest {
    pub shared_folder: SharedFolder,
    pub boundary: RevisionBoundary,
}

impl CaptureRequest {
    pub fn new(shared_folder: SharedFolder, boundary: RevisionBoundary) -> Self {
        Self {
            shared_folder,
            boundary,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    Preview,
    Apply,
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::{FolderScope, SharedFolderId};

    #[test]
    fn capture_requests_are_for_shared_folders() {
        let folder = SharedFolder::new(
            SharedFolderId::new("folder-devbox").expect("folder id"),
            "/workspace/devbox",
            "devbox",
            FolderScope::WholeFolder,
        )
        .expect("folder");

        let request = CaptureRequest::new(folder, RevisionBoundary::LoomCommand);

        assert_eq!(request.shared_folder.display_name(), "devbox");
        assert_eq!(request.boundary, RevisionBoundary::LoomCommand);
    }
}
