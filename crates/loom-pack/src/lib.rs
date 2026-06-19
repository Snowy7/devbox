//! Loom pack-format boundary.
//!
//! This crate will own compact object transport bundles. It is intentionally a
//! skeleton in PR 1 so object identity remains anchored in `loom-core`.

use loom_core::{FolderRevisionId, ObjectId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackObject {
    pub object_id: ObjectId,
    pub compressed_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackManifest {
    pub folder_revision_id: FolderRevisionId,
    pub objects: Vec<PackObject>,
}

impl PackManifest {
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_manifest_counts_objects() {
        let object_id = ObjectId::from_blake3_hex(
            "a3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03",
        )
        .expect("object id");
        let manifest = PackManifest {
            folder_revision_id: FolderRevisionId::new("revision-1").expect("revision id"),
            objects: vec![PackObject {
                object_id,
                compressed_size_bytes: 12,
            }],
        };

        assert_eq!(manifest.object_count(), 1);
    }
}
