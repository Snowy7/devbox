//! Core Devbox domain types.

pub mod scanner;

use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainIdError {
    Empty,
    InvalidBlobHashLength { actual: usize },
    InvalidBlobHashCharacter { character: char },
}

impl fmt::Display for DomainIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "identifier cannot be empty"),
            Self::InvalidBlobHashLength { actual } => {
                write!(
                    f,
                    "blob id must be a 64-character BLAKE3 hex digest, got {actual}"
                )
            }
            Self::InvalidBlobHashCharacter { character } => {
                write!(f, "blob id contains non-hex character '{character}'")
            }
        }
    }
}

impl std::error::Error for DomainIdError {}

macro_rules! non_empty_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, DomainIdError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(DomainIdError::Empty);
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

non_empty_id!(ProjectId);
non_empty_id!(SnapshotId);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlobId(String);

impl BlobId {
    pub const BLAKE3_HEX_LENGTH: usize = 64;

    pub fn from_blake3_hex(value: impl Into<String>) -> Result<Self, DomainIdError> {
        let value = value.into();
        if value.len() != Self::BLAKE3_HEX_LENGTH {
            return Err(DomainIdError::InvalidBlobHashLength {
                actual: value.len(),
            });
        }

        if let Some(character) = value
            .chars()
            .find(|character| !character.is_ascii_hexdigit())
        {
            return Err(DomainIdError::InvalidBlobHashCharacter { character });
        }

        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Include,
    Exclude { reason: String },
    RequiresUserDecision { reason: String },
}

impl PolicyDecision {
    pub fn is_included(&self) -> bool {
        matches!(self, Self::Include)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestEntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    path: PathBuf,
    kind: ManifestEntryKind,
    blob_id: Option<BlobId>,
    size_bytes: u64,
    policy_decision: PolicyDecision,
}

impl ManifestEntry {
    pub fn new(
        path: impl Into<PathBuf>,
        kind: ManifestEntryKind,
        blob_id: Option<BlobId>,
        size_bytes: u64,
        policy_decision: PolicyDecision,
    ) -> Self {
        Self {
            path: path.into(),
            kind,
            blob_id,
            size_bytes,
            policy_decision,
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn kind(&self) -> &ManifestEntryKind {
        &self.kind
    }

    pub fn blob_id(&self) -> Option<&BlobId> {
        self.blob_id.as_ref()
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn policy_decision(&self) -> &PolicyDecision {
        &self.policy_decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_and_snapshot_ids_reject_empty_values() {
        assert_eq!(ProjectId::new("   "), Err(DomainIdError::Empty));
        assert_eq!(SnapshotId::new(""), Err(DomainIdError::Empty));
    }

    #[test]
    fn project_and_snapshot_ids_preserve_non_empty_values() {
        let project_id = ProjectId::new("project-local-devbox").expect("valid project id");
        let snapshot_id = SnapshotId::new("snapshot-0001").expect("valid snapshot id");

        assert_eq!(project_id.as_str(), "project-local-devbox");
        assert_eq!(snapshot_id.to_string(), "snapshot-0001");
    }

    #[test]
    fn blob_id_accepts_64_character_hex_digests() {
        let blob_id = BlobId::from_blake3_hex(
            "A3F35A5B6A1D118E4F9F4C23B77D982C84E4C3F4D53172AC89EACD1D29D98F03",
        )
        .expect("valid BLAKE3-style digest");

        assert_eq!(
            blob_id.as_str(),
            "a3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03"
        );
    }

    #[test]
    fn blob_id_rejects_invalid_digest_shape() {
        assert_eq!(
            BlobId::from_blake3_hex("abc"),
            Err(DomainIdError::InvalidBlobHashLength { actual: 3 })
        );

        assert_eq!(
            BlobId::from_blake3_hex(
                "z3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03",
            ),
            Err(DomainIdError::InvalidBlobHashCharacter { character: 'z' })
        );
    }

    #[test]
    fn manifest_entry_records_policy_and_content_identity() {
        let blob_id = BlobId::from_blake3_hex(
            "a3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03",
        )
        .expect("valid blob id");

        let entry = ManifestEntry::new(
            "src/main.rs",
            ManifestEntryKind::File,
            Some(blob_id.clone()),
            128,
            PolicyDecision::Include,
        );

        assert_eq!(entry.path(), &PathBuf::from("src/main.rs"));
        assert_eq!(entry.kind(), &ManifestEntryKind::File);
        assert_eq!(entry.blob_id(), Some(&blob_id));
        assert_eq!(entry.size_bytes(), 128);
        assert!(entry.policy_decision().is_included());
    }

    #[test]
    fn policy_can_explain_excluded_paths() {
        let decision = PolicyDecision::Exclude {
            reason: "generated dependency directory".to_string(),
        };

        assert!(!decision.is_included());
        assert_eq!(
            decision,
            PolicyDecision::Exclude {
                reason: "generated dependency directory".to_string()
            }
        );
    }
}
