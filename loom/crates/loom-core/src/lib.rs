//! Canonical Loom domain vocabulary.
//!
//! Loom owns source-control and sync semantics for shared folders. These types
//! are intentionally small: persistence, scanning, packing, transport, and
//! background behavior live in sibling crates.

use std::fmt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoomError {
    EmptyId { kind: &'static str },
    InvalidObjectHashLength { actual: usize },
    InvalidObjectHashCharacter { character: char },
    InvalidChunkHashLength { actual: usize },
    InvalidChunkHashCharacter { character: char },
    EmptyPath { kind: &'static str },
    AbsolutePath { kind: &'static str, path: PathBuf },
    ParentPath { kind: &'static str, path: PathBuf },
    FileVersionMissingObject { path: PathBuf },
    EmptyMessage { kind: &'static str },
}

impl fmt::Display for LoomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId { kind } => write!(f, "{kind} cannot be empty"),
            Self::InvalidObjectHashLength { actual } => write!(
                f,
                "object id must be a 64-character BLAKE3 hex digest, got {actual}"
            ),
            Self::InvalidObjectHashCharacter { character } => {
                write!(f, "object id contains non-hex character '{character}'")
            }
            Self::InvalidChunkHashLength { actual } => write!(
                f,
                "chunk id must be a 64-character BLAKE3 hex digest, got {actual}"
            ),
            Self::InvalidChunkHashCharacter { character } => {
                write!(f, "chunk id contains non-hex character '{character}'")
            }
            Self::EmptyPath { kind } => write!(f, "{kind} cannot be empty"),
            Self::AbsolutePath { kind, path } => {
                write!(f, "{kind} must be relative, got {}", path.display())
            }
            Self::ParentPath { kind, path } => {
                write!(f, "{kind} must not contain '..', got {}", path.display())
            }
            Self::FileVersionMissingObject { path } => write!(
                f,
                "file version for {} requires an object id",
                path.display()
            ),
            Self::EmptyMessage { kind } => write!(f, "{kind} cannot be empty"),
        }
    }
}

impl std::error::Error for LoomError {}

macro_rules! non_empty_id {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, LoomError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(LoomError::EmptyId { kind: $kind });
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

non_empty_id!(FileVersionId, "file version id");
non_empty_id!(FolderRevisionId, "folder revision id");
non_empty_id!(CheckpointId, "checkpoint id");
non_empty_id!(PinId, "pin id");
non_empty_id!(CursorId, "cursor id");
non_empty_id!(SharedFolderId, "shared folder id");
non_empty_id!(MachineId, "machine id");
non_empty_id!(WorkspaceId, "workspace id");
non_empty_id!(WorkspaceSessionId, "workspace session id");

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(String);

impl ObjectId {
    pub const BLAKE3_HEX_LENGTH: usize = 64;

    pub fn from_blake3_hex(value: impl Into<String>) -> Result<Self, LoomError> {
        let value = value.into();
        if value.len() != Self::BLAKE3_HEX_LENGTH {
            return Err(LoomError::InvalidObjectHashLength {
                actual: value.len(),
            });
        }

        if let Some(character) = value
            .chars()
            .find(|character| !character.is_ascii_hexdigit())
        {
            return Err(LoomError::InvalidObjectHashCharacter { character });
        }

        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkId(String);

impl ChunkId {
    pub const BLAKE3_HEX_LENGTH: usize = 64;

    pub fn from_blake3_hex(value: impl Into<String>) -> Result<Self, LoomError> {
        let value = value.into();
        if value.len() != Self::BLAKE3_HEX_LENGTH {
            return Err(LoomError::InvalidChunkHashLength {
                actual: value.len(),
            });
        }

        if let Some(character) = value
            .chars()
            .find(|character| !character.is_ascii_hexdigit())
        {
            return Err(LoomError::InvalidChunkHashCharacter { character });
        }

        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    id: ObjectId,
    size_bytes: u64,
}

impl Object {
    pub fn new(id: ObjectId, size_bytes: u64) -> Self {
        Self { id, size_bytes }
    }

    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    id: ChunkId,
    size_bytes: u64,
}

impl Chunk {
    pub fn new(id: ChunkId, size_bytes: u64) -> Self {
        Self { id, size_bytes }
    }

    pub fn id(&self) -> &ChunkId {
        &self.id
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HydrationState {
    RemoteOnly,
    Partial,
    Hydrated,
}

impl HydrationState {
    pub fn has_local_bytes(self) -> bool {
        matches!(self, Self::Partial | Self::Hydrated)
    }

    pub fn is_complete(self) -> bool {
        matches!(self, Self::Hydrated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    object_id: ObjectId,
    hydration_state: HydrationState,
    local_path: PathBuf,
    size_bytes: Option<u64>,
    updated_at: String,
}

impl CacheEntry {
    pub fn new(
        object_id: ObjectId,
        hydration_state: HydrationState,
        local_path: impl Into<PathBuf>,
        size_bytes: Option<u64>,
        updated_at: impl Into<String>,
    ) -> Result<Self, LoomError> {
        Ok(Self {
            object_id,
            hydration_state,
            local_path: validate_relative_path(local_path.into(), "cache entry path")?,
            size_bytes,
            updated_at: non_empty_message("updated_at", updated_at.into())?,
        })
    }

    pub fn object_id(&self) -> &ObjectId {
        &self.object_id
    }

    pub fn hydration_state(&self) -> HydrationState {
        self.hydration_state
    }

    pub fn local_path(&self) -> &Path {
        &self.local_path
    }

    pub fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }

    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersion {
    id: FileVersionId,
    path: PathBuf,
    kind: FileKind,
    object_id: Option<ObjectId>,
    size_bytes: Option<u64>,
    captured_at: String,
}

impl FileVersion {
    pub fn new(
        id: FileVersionId,
        path: impl Into<PathBuf>,
        kind: FileKind,
        object_id: Option<ObjectId>,
        size_bytes: Option<u64>,
        captured_at: impl Into<String>,
    ) -> Result<Self, LoomError> {
        let path = validate_relative_path(path.into(), "file version path")?;
        if kind == FileKind::File && object_id.is_none() {
            return Err(LoomError::FileVersionMissingObject { path });
        }
        let captured_at = non_empty_message("captured_at", captured_at.into())?;

        Ok(Self {
            id,
            path,
            kind,
            object_id,
            size_bytes,
            captured_at,
        })
    }

    pub fn id(&self) -> &FileVersionId {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> &FileKind {
        &self.kind
    }

    pub fn object_id(&self) -> Option<&ObjectId> {
        self.object_id.as_ref()
    }

    pub fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }

    pub fn captured_at(&self) -> &str {
        &self.captured_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderEntry {
    path: PathBuf,
    file_version_id: FileVersionId,
}

impl FolderEntry {
    pub fn new(
        path: impl Into<PathBuf>,
        file_version_id: FileVersionId,
    ) -> Result<Self, LoomError> {
        Ok(Self {
            path: validate_relative_path(path.into(), "folder entry path")?,
            file_version_id,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_version_id(&self) -> &FileVersionId {
        &self.file_version_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionBoundary {
    DebounceWindow,
    LoomCommand,
    Sync,
    Restore,
    SandboxMerge,
    Checkpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRevision {
    id: FolderRevisionId,
    shared_folder_id: SharedFolderId,
    parent_id: Option<FolderRevisionId>,
    boundary: RevisionBoundary,
    entries: Vec<FolderEntry>,
    created_at: String,
}

impl FolderRevision {
    pub fn new(
        id: FolderRevisionId,
        shared_folder_id: SharedFolderId,
        parent_id: Option<FolderRevisionId>,
        boundary: RevisionBoundary,
        entries: Vec<FolderEntry>,
        created_at: impl Into<String>,
    ) -> Result<Self, LoomError> {
        Ok(Self {
            id,
            shared_folder_id,
            parent_id,
            boundary,
            entries,
            created_at: non_empty_message("created_at", created_at.into())?,
        })
    }

    pub fn id(&self) -> &FolderRevisionId {
        &self.id
    }

    pub fn shared_folder_id(&self) -> &SharedFolderId {
        &self.shared_folder_id
    }

    pub fn parent_id(&self) -> Option<&FolderRevisionId> {
        self.parent_id.as_ref()
    }

    pub fn boundary(&self) -> RevisionBoundary {
        self.boundary
    }

    pub fn entries(&self) -> &[FolderEntry] {
        &self.entries
    }

    pub fn created_at(&self) -> &str {
        &self.created_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    id: CheckpointId,
    revision_id: FolderRevisionId,
    message: String,
    created_at: String,
}

impl Checkpoint {
    pub fn new(
        id: CheckpointId,
        revision_id: FolderRevisionId,
        message: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Result<Self, LoomError> {
        Ok(Self {
            id,
            revision_id,
            message: non_empty_message("checkpoint message", message.into())?,
            created_at: non_empty_message("created_at", created_at.into())?,
        })
    }

    pub fn id(&self) -> &CheckpointId {
        &self.id
    }

    pub fn revision_id(&self) -> &FolderRevisionId {
        &self.revision_id
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn created_at(&self) -> &str {
        &self.created_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    id: PinId,
    revision_id: FolderRevisionId,
    reason: String,
    created_at: String,
}

impl Pin {
    pub fn new(
        id: PinId,
        revision_id: FolderRevisionId,
        reason: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Result<Self, LoomError> {
        Ok(Self {
            id,
            revision_id,
            reason: non_empty_message("pin reason", reason.into())?,
            created_at: non_empty_message("created_at", created_at.into())?,
        })
    }

    pub fn id(&self) -> &PinId {
        &self.id
    }

    pub fn revision_id(&self) -> &FolderRevisionId {
        &self.revision_id
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn created_at(&self) -> &str {
        &self.created_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorOwner {
    Machine(MachineId),
    Remote(String),
    MaterializedFolder(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    id: CursorId,
    shared_folder_id: SharedFolderId,
    owner: CursorOwner,
    revision_id: Option<FolderRevisionId>,
    updated_at: String,
}

impl Cursor {
    pub fn new(
        id: CursorId,
        shared_folder_id: SharedFolderId,
        owner: CursorOwner,
        revision_id: Option<FolderRevisionId>,
        updated_at: impl Into<String>,
    ) -> Result<Self, LoomError> {
        Ok(Self {
            id,
            shared_folder_id,
            owner,
            revision_id,
            updated_at: non_empty_message("updated_at", updated_at.into())?,
        })
    }

    pub fn id(&self) -> &CursorId {
        &self.id
    }

    pub fn shared_folder_id(&self) -> &SharedFolderId {
        &self.shared_folder_id
    }

    pub fn owner(&self) -> &CursorOwner {
        &self.owner
    }

    pub fn revision_id(&self) -> Option<&FolderRevisionId> {
        self.revision_id.as_ref()
    }

    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderScope {
    WholeFolder,
    Subtree(PathBuf),
}

impl FolderScope {
    pub fn subtree(path: impl Into<PathBuf>) -> Result<Self, LoomError> {
        Ok(Self::Subtree(validate_relative_path(
            path.into(),
            "folder scope path",
        )?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFolder {
    id: SharedFolderId,
    root: PathBuf,
    display_name: String,
    scope: FolderScope,
}

impl SharedFolder {
    pub fn new(
        id: SharedFolderId,
        root: impl Into<PathBuf>,
        display_name: impl Into<String>,
        scope: FolderScope,
    ) -> Result<Self, LoomError> {
        let root = root.into();
        if root.as_os_str().is_empty() {
            return Err(LoomError::EmptyPath {
                kind: "shared folder root",
            });
        }

        Ok(Self {
            id,
            root,
            display_name: non_empty_message("shared folder display name", display_name.into())?,
            scope,
        })
    }

    pub fn id(&self) -> &SharedFolderId {
        &self.id
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn scope(&self) -> &FolderScope {
        &self.scope
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceKind {
    AgentVirtual,
    MaterializedSandbox,
    OsFilesystemMount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSessionState {
    Open,
    Closed,
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSession {
    id: WorkspaceSessionId,
    workspace_id: WorkspaceId,
    shared_folder_id: SharedFolderId,
    base_revision_id: FolderRevisionId,
    kind: WorkspaceKind,
    state: WorkspaceSessionState,
    created_at: String,
}

impl WorkspaceSession {
    pub fn new(
        id: WorkspaceSessionId,
        workspace_id: WorkspaceId,
        shared_folder_id: SharedFolderId,
        base_revision_id: FolderRevisionId,
        kind: WorkspaceKind,
        state: WorkspaceSessionState,
        created_at: impl Into<String>,
    ) -> Result<Self, LoomError> {
        Ok(Self {
            id,
            workspace_id,
            shared_folder_id,
            base_revision_id,
            kind,
            state,
            created_at: non_empty_message("workspace session created_at", created_at.into())?,
        })
    }

    pub fn id(&self) -> &WorkspaceSessionId {
        &self.id
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.workspace_id
    }

    pub fn shared_folder_id(&self) -> &SharedFolderId {
        &self.shared_folder_id
    }

    pub fn base_revision_id(&self) -> &FolderRevisionId {
        &self.base_revision_id
    }

    pub fn kind(&self) -> WorkspaceKind {
        self.kind
    }

    pub fn state(&self) -> WorkspaceSessionState {
        self.state
    }

    pub fn created_at(&self) -> &str {
        &self.created_at
    }
}

fn non_empty_message(kind: &'static str, value: String) -> Result<String, LoomError> {
    if value.trim().is_empty() {
        return Err(LoomError::EmptyMessage { kind });
    }
    Ok(value)
}

fn validate_relative_path(path: PathBuf, kind: &'static str) -> Result<PathBuf, LoomError> {
    if path.as_os_str().is_empty() {
        return Err(LoomError::EmptyPath { kind });
    }

    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(LoomError::ParentPath { kind, path });
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(LoomError::AbsolutePath { kind, path });
            }
        }
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object_id() -> ObjectId {
        ObjectId::from_blake3_hex(
            "A3F35A5B6A1D118E4F9F4C23B77D982C84E4C3F4D53172AC89EACD1D29D98F03",
        )
        .expect("valid object id")
    }

    #[test]
    fn object_id_accepts_canonical_blake3_hex() {
        assert_eq!(
            object_id().as_str(),
            "a3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03"
        );
    }

    #[test]
    fn object_records_content_identity_and_size() {
        let id = object_id();
        let object = Object::new(id.clone(), 128);

        assert_eq!(object.id(), &id);
        assert_eq!(object.size_bytes(), 128);
    }

    #[test]
    fn chunks_record_content_identity_and_size() {
        let id = ChunkId::from_blake3_hex(
            "B3F35A5B6A1D118E4F9F4C23B77D982C84E4C3F4D53172AC89EACD1D29D98F03",
        )
        .expect("valid chunk id");
        let chunk = Chunk::new(id.clone(), 64 * 1024);

        assert_eq!(
            id.as_str(),
            "b3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03"
        );
        assert_eq!(chunk.id(), &id);
        assert_eq!(chunk.size_bytes(), 64 * 1024);
    }

    #[test]
    fn object_id_rejects_invalid_hashes() {
        assert_eq!(
            ObjectId::from_blake3_hex("abc"),
            Err(LoomError::InvalidObjectHashLength { actual: 3 })
        );
        assert_eq!(
            ObjectId::from_blake3_hex(
                "z3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03",
            ),
            Err(LoomError::InvalidObjectHashCharacter { character: 'z' })
        );
    }

    #[test]
    fn chunk_id_rejects_invalid_hashes() {
        assert_eq!(
            ChunkId::from_blake3_hex("abc"),
            Err(LoomError::InvalidChunkHashLength { actual: 3 })
        );
        assert_eq!(
            ChunkId::from_blake3_hex(
                "z3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03",
            ),
            Err(LoomError::InvalidChunkHashCharacter { character: 'z' })
        );
    }

    #[test]
    fn cache_entries_validate_hydration_state_and_local_paths() {
        let entry = CacheEntry::new(
            object_id(),
            HydrationState::Hydrated,
            "objects/b3/a3/f3/object",
            Some(128),
            "2026-06-19T12:00:00Z",
        )
        .expect("cache entry creates");

        assert_eq!(entry.hydration_state(), HydrationState::Hydrated);
        assert!(entry.hydration_state().has_local_bytes());
        assert!(entry.hydration_state().is_complete());
        assert_eq!(entry.local_path(), Path::new("objects/b3/a3/f3/object"));

        assert!(CacheEntry::new(
            object_id(),
            HydrationState::RemoteOnly,
            "../escape",
            None,
            "2026-06-19T12:00:00Z",
        )
        .is_err());
        assert!(!HydrationState::RemoteOnly.has_local_bytes());
        assert!(HydrationState::Partial.has_local_bytes());
        assert!(!HydrationState::Partial.is_complete());
    }

    #[test]
    fn ids_reject_empty_values() {
        assert_eq!(
            SharedFolderId::new(" "),
            Err(LoomError::EmptyId {
                kind: "shared folder id"
            })
        );
        assert_eq!(
            FolderRevisionId::new(""),
            Err(LoomError::EmptyId {
                kind: "folder revision id"
            })
        );
    }

    #[test]
    fn file_versions_are_path_and_object_focused() {
        let version = FileVersion::new(
            FileVersionId::new("file-version-1").expect("id"),
            "src/main.rs",
            FileKind::File,
            Some(object_id()),
            Some(42),
            "2026-06-19T12:00:00Z",
        )
        .expect("file version creates");

        assert_eq!(version.path(), Path::new("src/main.rs"));
        assert_eq!(version.size_bytes(), Some(42));
        assert!(version.object_id().is_some());

        let missing_object = FileVersion::new(
            FileVersionId::new("file-version-2").expect("id"),
            "src/lib.rs",
            FileKind::File,
            None,
            Some(10),
            "2026-06-19T12:00:00Z",
        )
        .expect_err("regular files require content object ids");
        assert!(matches!(
            missing_object,
            LoomError::FileVersionMissingObject { .. }
        ));
    }

    #[test]
    fn folder_revisions_are_coherent_sets_of_file_versions() {
        let folder_id = SharedFolderId::new("folder-bindhub").expect("folder id");
        let revision_id = FolderRevisionId::new("revision-1").expect("revision id");
        let file_version_id = FileVersionId::new("file-version-1").expect("file version id");
        let entry = FolderEntry::new("README.md", file_version_id.clone()).expect("entry");

        let revision = FolderRevision::new(
            revision_id.clone(),
            folder_id.clone(),
            None,
            RevisionBoundary::DebounceWindow,
            vec![entry],
            "2026-06-19T12:00:00Z",
        )
        .expect("revision creates");

        assert_eq!(revision.shared_folder_id(), &folder_id);
        assert_eq!(revision.entries()[0].file_version_id(), &file_version_id);
        assert_eq!(revision.boundary(), RevisionBoundary::DebounceWindow);
    }

    #[test]
    fn checkpoints_pins_and_cursors_reference_folder_revisions() {
        let folder_id = SharedFolderId::new("folder-bindhub").expect("folder id");
        let revision_id = FolderRevisionId::new("revision-1").expect("revision id");
        let checkpoint = Checkpoint::new(
            CheckpointId::new("checkpoint-1").expect("checkpoint id"),
            revision_id.clone(),
            "parser spike working",
            "2026-06-19T12:00:00Z",
        )
        .expect("checkpoint creates");
        let pin = Pin::new(
            PinId::new("pin-1").expect("pin id"),
            revision_id.clone(),
            "keep before agent merge",
            "2026-06-19T12:01:00Z",
        )
        .expect("pin creates");
        let cursor = Cursor::new(
            CursorId::new("cursor-machine").expect("cursor id"),
            folder_id.clone(),
            CursorOwner::Machine(MachineId::new("machine-desk").expect("machine id")),
            Some(revision_id.clone()),
            "2026-06-19T12:02:00Z",
        )
        .expect("cursor creates");

        assert_eq!(checkpoint.revision_id(), &revision_id);
        assert_eq!(pin.revision_id(), &revision_id);
        assert_eq!(cursor.shared_folder_id(), &folder_id);
        assert_eq!(cursor.revision_id(), Some(&revision_id));
    }

    #[test]
    fn shared_folder_scope_can_cover_a_subtree() {
        let folder = SharedFolder::new(
            SharedFolderId::new("folder-code").expect("folder id"),
            "/Users/me/Code",
            "Code",
            FolderScope::subtree("client/app").expect("subtree scope"),
        )
        .expect("shared folder creates");

        assert_eq!(folder.display_name(), "Code");
        assert_eq!(
            folder.scope(),
            &FolderScope::Subtree(PathBuf::from("client/app"))
        );
        assert!(FolderScope::subtree("../escape").is_err());
    }

    #[test]
    fn workspace_sessions_name_virtual_views_over_folder_revisions() {
        let session = WorkspaceSession::new(
            WorkspaceSessionId::new("agent-session-1").expect("session id"),
            WorkspaceId::new("workspace-bindhub").expect("workspace id"),
            SharedFolderId::new("folder-bindhub").expect("folder id"),
            FolderRevisionId::new("revision-1").expect("revision id"),
            WorkspaceKind::AgentVirtual,
            WorkspaceSessionState::Open,
            "2026-06-22T12:00:00Z",
        )
        .expect("workspace session creates");

        assert_eq!(session.id().as_str(), "agent-session-1");
        assert_eq!(session.workspace_id().as_str(), "workspace-bindhub");
        assert_eq!(session.shared_folder_id().as_str(), "folder-bindhub");
        assert_eq!(session.base_revision_id().as_str(), "revision-1");
        assert_eq!(session.kind(), WorkspaceKind::AgentVirtual);
        assert_eq!(session.state(), WorkspaceSessionState::Open);
    }

    #[test]
    fn workspace_ids_reject_empty_values() {
        assert_eq!(
            WorkspaceId::new(""),
            Err(LoomError::EmptyId {
                kind: "workspace id"
            })
        );
        assert_eq!(
            WorkspaceSessionId::new(" "),
            Err(LoomError::EmptyId {
                kind: "workspace session id"
            })
        );
    }
}
