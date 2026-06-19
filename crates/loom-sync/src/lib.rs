//! Loom sync and remote protocol boundary.
//!
//! Human Loom commands use `sync` and `clone`; this crate deliberately uses
//! folder-continuity vocabulary instead of Git-shaped transport commands.

use loom_core::{FolderRevisionId, SharedFolderId};
use loom_pack::PackManifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncOperation {
    Sync,
    Clone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRequest {
    pub shared_folder_id: SharedFolderId,
    pub operation: SyncOperation,
    pub target_revision_id: Option<FolderRevisionId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncExchange {
    pub request: SyncRequest,
    pub pack: Option<PackManifest>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_request_uses_folder_vocabulary() {
        let request = SyncRequest {
            shared_folder_id: SharedFolderId::new("folder-devbox").expect("folder id"),
            operation: SyncOperation::Sync,
            target_revision_id: None,
        };

        assert_eq!(request.operation, SyncOperation::Sync);
        assert_eq!(request.shared_folder_id.as_str(), "folder-devbox");
    }
}
