//! Local Loom persistence boundary.
//!
//! This crate will own object storage, file-version metadata, folder revisions,
//! retention, checkpoints, pins, and cursors. The old `devbox-store` crate is
//! still compiled for alpha compatibility while these responsibilities migrate.

use loom_core::{Cursor, FileVersion, FolderRevision, ObjectId};

pub const CRATE_ROLE: &str = "local Loom object and metadata store for shared-folder history";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreBoundary {
    pub stores_objects: bool,
    pub stores_file_versions: bool,
    pub stores_folder_revisions: bool,
    pub stores_cursors: bool,
}

impl StoreBoundary {
    pub fn loom_owned() -> Self {
        Self {
            stores_objects: true,
            stores_file_versions: true,
            stores_folder_revisions: true,
            stores_cursors: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub id: ObjectId,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFolderState {
    pub revision: FolderRevision,
    pub file_versions: Vec<FileVersion>,
    pub cursors: Vec<Cursor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_boundary_names_loom_owned_state() {
        let boundary = StoreBoundary::loom_owned();

        assert!(boundary.stores_objects);
        assert!(boundary.stores_file_versions);
        assert!(boundary.stores_folder_revisions);
        assert!(boundary.stores_cursors);
        assert!(CRATE_ROLE.contains("Loom"));
    }
}
