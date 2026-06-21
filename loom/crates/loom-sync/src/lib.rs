//! Loom sync and remote protocol boundary.
//!
//! Human Loom commands use `sync` and `clone`; this crate deliberately uses
//! folder-continuity vocabulary instead of Git-shaped transport commands.

use loom_core::{FolderRevision, FolderRevisionId, ObjectId, SharedFolderId};
use loom_pack::{
    LoomPack, PackCompression, PackError, PackManifest, PackObject, PackObjectPayload,
    PackPayloadAvailability,
};
use loom_store::{LocalStore, ObjectCache, StoreError};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const LOCAL_FILESYSTEM_REMOTE_KIND: &str = "local-fs";
pub const DEFAULT_REMOTE_NAME: &str = "local";
pub const DEFAULT_CURSOR_ID: &str = "shared-folder";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub latest_revision_id: FolderRevisionId,
    pub previous_remote_revision_id: Option<FolderRevisionId>,
    pub uploaded_objects: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub latest_revision_id: FolderRevisionId,
    pub imported_file_versions: usize,
    pub imported_revisions: usize,
    pub imported_checkpoints: usize,
    pub imported_pins: usize,
    pub imported_objects: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectTransferCapability {
    InlinePackObjects,
    SeparateObjects,
}

pub trait LoomRemote {
    fn object_transfer_capability(&self) -> ObjectTransferCapability {
        ObjectTransferCapability::InlinePackObjects
    }
    fn get_cursor(&self, cursor_id: &str) -> SyncResult<Option<FolderRevisionId>>;
    fn compare_and_set_cursor(
        &self,
        cursor_id: &str,
        expected: Option<&FolderRevisionId>,
        next: &FolderRevisionId,
    ) -> SyncResult<()>;
    fn put_pack(&self, pack: &LoomPack) -> SyncResult<()>;
    fn get_pack(&self, revision_id: &FolderRevisionId) -> SyncResult<LoomPack>;
    fn has_object(&self, object_id: &ObjectId) -> SyncResult<bool>;
    fn put_object(&self, object_id: &ObjectId, bytes: &[u8]) -> SyncResult<()>;
    fn get_object(&self, object_id: &ObjectId) -> SyncResult<Vec<u8>>;
}

#[derive(Debug, Clone)]
pub struct LocalFilesystemRemote {
    root: PathBuf,
}

#[derive(Debug)]
struct CursorLock {
    path: PathBuf,
}

impl Drop for CursorLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl LocalFilesystemRemote {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn packs_dir(&self) -> PathBuf {
        self.root.join("packs")
    }

    fn cursors_dir(&self) -> PathBuf {
        self.root.join("cursors")
    }

    fn object_cache_root(&self) -> PathBuf {
        self.root.join("object-cache")
    }

    fn object_cache(&self) -> SyncResult<ObjectCache> {
        ObjectCache::open(self.object_cache_root()).map_err(SyncError::Store)
    }

    fn marker_path(&self) -> PathBuf {
        self.root.join("loom-remote-v1")
    }

    fn ensure_layout(&self) -> SyncResult<()> {
        create_dir_all(&self.root)?;
        create_dir_all(self.packs_dir())?;
        create_dir_all(self.cursors_dir())?;
        self.object_cache()?;
        fs::write(self.marker_path(), b"loom local filesystem remote\n").map_err(|source| {
            SyncError::Io {
                path: self.marker_path(),
                source,
            }
        })?;
        Ok(())
    }

    fn pack_path(&self, revision_id: &FolderRevisionId) -> PathBuf {
        self.packs_dir()
            .join(format!("{}.loompack", revision_id.as_str()))
    }

    fn cursor_path(&self, cursor_id: &str) -> SyncResult<PathBuf> {
        if cursor_id.trim().is_empty()
            || cursor_id.contains('/')
            || cursor_id.contains('\\')
            || cursor_id.contains("..")
        {
            return Err(SyncError::InvalidCursor(cursor_id.to_string()));
        }

        Ok(self.cursors_dir().join(format!("{cursor_id}.txt")))
    }

    fn cursor_lock_path(&self, cursor_id: &str) -> SyncResult<PathBuf> {
        if cursor_id.trim().is_empty()
            || cursor_id.contains('/')
            || cursor_id.contains('\\')
            || cursor_id.contains("..")
        {
            return Err(SyncError::InvalidCursor(cursor_id.to_string()));
        }

        Ok(self.cursors_dir().join(format!("{cursor_id}.lock")))
    }

    fn acquire_cursor_lock(&self, cursor_id: &str) -> SyncResult<CursorLock> {
        let path = self.cursor_lock_path(cursor_id)?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(source) =
                    file.write_all(format!("pid={}\n", std::process::id()).as_bytes())
                {
                    let _ = fs::remove_file(&path);
                    return Err(SyncError::Io {
                        path: path.clone(),
                        source,
                    });
                }
                Ok(CursorLock { path })
            }
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                Err(SyncError::CursorLockBusy {
                    cursor_id: cursor_id.to_string(),
                    path,
                })
            }
            Err(source) => Err(SyncError::Io { path, source }),
        }
    }
}

impl LoomRemote for LocalFilesystemRemote {
    fn object_transfer_capability(&self) -> ObjectTransferCapability {
        ObjectTransferCapability::SeparateObjects
    }

    fn get_cursor(&self, cursor_id: &str) -> SyncResult<Option<FolderRevisionId>> {
        let path = self.cursor_path(cursor_id)?;
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let value = contents.trim();
                if value.is_empty() {
                    return Ok(None);
                }
                Ok(Some(FolderRevisionId::new(value.to_string())?))
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(SyncError::Io { path, source }),
        }
    }

    fn compare_and_set_cursor(
        &self,
        cursor_id: &str,
        expected: Option<&FolderRevisionId>,
        next: &FolderRevisionId,
    ) -> SyncResult<()> {
        self.ensure_layout()?;
        let _lock = self.acquire_cursor_lock(cursor_id)?;
        let current = self.get_cursor(cursor_id)?;
        if current.as_ref() != expected {
            return Err(SyncError::CursorConflict {
                cursor_id: cursor_id.to_string(),
                expected: expected.cloned(),
                actual: current,
                attempted: next.clone(),
            });
        }

        let path = self.cursor_path(cursor_id)?;
        fs::write(&path, format!("{next}\n")).map_err(|source| SyncError::Io { path, source })
    }

    fn put_pack(&self, pack: &LoomPack) -> SyncResult<()> {
        self.ensure_layout()?;
        let path = self.pack_path(&pack.manifest.latest_revision_id);
        fs::write(&path, pack.encode()).map_err(|source| SyncError::Io { path, source })
    }

    fn get_pack(&self, revision_id: &FolderRevisionId) -> SyncResult<LoomPack> {
        let path = self.pack_path(revision_id);
        let bytes = fs::read(&path).map_err(|source| SyncError::Io { path, source })?;
        LoomPack::decode(&bytes).map_err(SyncError::Pack)
    }

    fn has_object(&self, object_id: &ObjectId) -> SyncResult<bool> {
        Ok(self.object_cache()?.exists(object_id))
    }

    fn put_object(&self, object_id: &ObjectId, bytes: &[u8]) -> SyncResult<()> {
        self.ensure_layout()?;
        self.object_cache()?.import_bytes(object_id, bytes)?;
        Ok(())
    }

    fn get_object(&self, object_id: &ObjectId) -> SyncResult<Vec<u8>> {
        self.object_cache()?
            .read(object_id)
            .map_err(SyncError::Store)
    }
}

pub fn sync_store_to_remote(store: &LocalStore, remote: &dyn LoomRemote) -> SyncResult<SyncReport> {
    let latest = store.latest_revision()?.ok_or(SyncError::NoLocalRevision)?;
    let previous_remote_revision_id = remote.get_cursor(DEFAULT_CURSOR_ID)?;
    if let Some(remote_revision_id) = &previous_remote_revision_id {
        if remote_revision_id == latest.id() {
            return Ok(SyncReport {
                latest_revision_id: latest.id().clone(),
                previous_remote_revision_id,
                uploaded_objects: 0,
            });
        }
        if !is_ancestor(store, remote_revision_id, latest.id())? {
            return Err(SyncError::DivergentState {
                remote_revision_id: remote_revision_id.clone(),
                local_revision_id: latest.id().clone(),
            });
        }
    }

    let (pack, uploaded_objects) = match remote.object_transfer_capability() {
        ObjectTransferCapability::SeparateObjects => {
            let pack = build_metadata_pack(store, latest.id())?;
            let mut uploaded_objects = 0;
            for object in &pack.manifest.objects {
                if !remote.has_object(&object.object_id)? {
                    let payload = store.object_cache().read(&object.object_id)?;
                    remote.put_object(&object.object_id, &payload)?;
                    uploaded_objects += 1;
                }
            }
            (pack, uploaded_objects)
        }
        ObjectTransferCapability::InlinePackObjects => {
            let pack = build_pack(store, latest.id())?;
            let uploaded_objects = pack.manifest.objects.len();
            (pack, uploaded_objects)
        }
    };
    remote.put_pack(&pack)?;
    remote.compare_and_set_cursor(
        DEFAULT_CURSOR_ID,
        previous_remote_revision_id.as_ref(),
        latest.id(),
    )?;

    Ok(SyncReport {
        latest_revision_id: latest.id().clone(),
        previous_remote_revision_id,
        uploaded_objects,
    })
}

pub fn import_pack(store: &LocalStore, pack: &LoomPack) -> SyncResult<ImportReport> {
    import_pack_with_remote(store, pack, None)
}

pub fn import_pack_from_remote(
    store: &LocalStore,
    pack: &LoomPack,
    remote: &dyn LoomRemote,
) -> SyncResult<ImportReport> {
    import_pack_with_remote(store, pack, Some(remote))
}

fn import_pack_with_remote(
    store: &LocalStore,
    pack: &LoomPack,
    remote: Option<&dyn LoomRemote>,
) -> SyncResult<ImportReport> {
    let mut imported_objects = 0;
    for object in &pack.manifest.objects {
        if !store.object_cache().exists(&object.object_id) {
            let payload = if let Some(payload) = pack.object_payload(&object.object_id) {
                payload.payload.clone()
            } else if let Some(remote) = remote {
                remote.get_object(&object.object_id)?
            } else {
                return Err(SyncError::MissingObjectPayload(object.object_id.clone()));
            };
            store.import_object_bytes(&object.object_id, &payload)?;
            imported_objects += 1;
        } else {
            store.record_cached_object_hydrated(&object.object_id, object.size_bytes)?;
        }
    }

    let imported_file_versions = store.import_file_versions(&pack.file_versions)?;
    let mut imported_revisions = 0;
    for revision in &pack.revisions {
        if store.import_revision(revision)? {
            imported_revisions += 1;
        }
    }

    let mut imported_checkpoints = 0;
    for checkpoint in &pack.checkpoints {
        if store.import_checkpoint(checkpoint)? {
            imported_checkpoints += 1;
        }
    }

    let mut imported_pins = 0;
    for pin in &pack.pins {
        if store.import_pin(pin)? {
            imported_pins += 1;
        }
    }

    Ok(ImportReport {
        latest_revision_id: pack.manifest.latest_revision_id.clone(),
        imported_file_versions,
        imported_revisions,
        imported_checkpoints,
        imported_pins,
        imported_objects,
    })
}

pub fn build_pack(
    store: &LocalStore,
    latest_revision_id: &FolderRevisionId,
) -> SyncResult<LoomPack> {
    build_pack_with_payload_mode(store, latest_revision_id, true)
}

pub fn build_metadata_pack(
    store: &LocalStore,
    latest_revision_id: &FolderRevisionId,
) -> SyncResult<LoomPack> {
    build_pack_with_payload_mode(store, latest_revision_id, false)
}

fn build_pack_with_payload_mode(
    store: &LocalStore,
    latest_revision_id: &FolderRevisionId,
    include_payloads: bool,
) -> SyncResult<LoomPack> {
    let export = store.export_state()?;
    let file_versions = export.file_versions;
    let revisions = export.revisions;
    if !revisions
        .iter()
        .any(|revision| revision.id() == latest_revision_id)
    {
        return Err(SyncError::MissingRevision(latest_revision_id.clone()));
    }

    let mut object_ids = BTreeSet::new();
    for version in &file_versions {
        if let Some(object_id) = version.object_id() {
            object_ids.insert(object_id.clone());
        }
    }

    let mut objects = Vec::new();
    let mut object_payloads = Vec::new();
    for object_id in object_ids {
        let payload = store.object_cache().read(&object_id)?;
        objects.push(PackObject {
            object_id: object_id.clone(),
            size_bytes: payload.len() as u64,
            compression: PackCompression::None,
            availability: if include_payloads {
                PackPayloadAvailability::Inline
            } else {
                PackPayloadAvailability::Remote
            },
        });
        if include_payloads {
            object_payloads.push(PackObjectPayload {
                object_id,
                compression: PackCompression::None,
                payload,
            });
        }
    }

    LoomPack::new(
        export.shared_folder_id,
        export.display_name,
        latest_revision_id.clone(),
        file_versions,
        revisions,
        export.checkpoints,
        export.pins,
        objects,
        object_payloads,
    )
    .map_err(SyncError::Pack)
}

fn is_ancestor(
    store: &LocalStore,
    possible_ancestor: &FolderRevisionId,
    revision_id: &FolderRevisionId,
) -> SyncResult<bool> {
    let revisions = store
        .revisions()?
        .into_iter()
        .map(|revision| (revision.id().clone(), revision))
        .collect::<BTreeMap<_, _>>();
    let mut current = Some(revision_id.clone());

    while let Some(current_id) = current {
        if &current_id == possible_ancestor {
            return Ok(true);
        }
        current = revisions
            .get(&current_id)
            .and_then(FolderRevision::parent_id)
            .cloned();
    }

    Ok(false)
}

#[derive(Debug)]
pub enum SyncError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Store(StoreError),
    Pack(PackError),
    Loom(loom_core::LoomError),
    NoLocalRevision,
    MissingRevision(FolderRevisionId),
    MissingObjectPayload(ObjectId),
    InvalidCursor(String),
    CursorLockBusy {
        cursor_id: String,
        path: PathBuf,
    },
    CursorConflict {
        cursor_id: String,
        expected: Option<FolderRevisionId>,
        actual: Option<FolderRevisionId>,
        attempted: FolderRevisionId,
    },
    DivergentState {
        remote_revision_id: FolderRevisionId,
        local_revision_id: FolderRevisionId,
    },
    MissingRemotePack(FolderRevisionId),
    RemoteConfig(String),
    RemoteAuth(String),
    RemoteTransport(String),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not access {}: {source}", path.display()),
            Self::Store(error) => write!(f, "{error}"),
            Self::Pack(error) => write!(f, "{error}"),
            Self::Loom(error) => write!(f, "{error}"),
            Self::NoLocalRevision => {
                write!(f, "no local folder revisions yet; run 'loom status' first")
            }
            Self::MissingRevision(revision_id) => {
                write!(f, "missing local folder revision {revision_id}")
            }
            Self::MissingObjectPayload(object_id) => {
                write!(f, "pack does not include payload bytes for object {object_id}")
            }
            Self::InvalidCursor(cursor_id) => {
                write!(f, "invalid cursor id '{cursor_id}'")
            }
            Self::CursorLockBusy { cursor_id, path } => write!(
                f,
                "cursor {cursor_id} compare-and-set is already in progress at {}",
                path.display()
            ),
            Self::CursorConflict {
                cursor_id,
                expected,
                actual,
                attempted,
            } => write!(
                f,
                "cursor {cursor_id} compare-and-set refused: expected {}, found {}, attempted {}",
                format_revision(expected.as_ref()),
                format_revision(actual.as_ref()),
                attempted
            ),
            Self::DivergentState {
                remote_revision_id,
                local_revision_id,
            } => write!(
                f,
                "sync refused because remote revision {remote_revision_id} and local revision {local_revision_id} diverged"
            ),
            Self::MissingRemotePack(revision_id) => {
                write!(f, "remote pack for folder revision {revision_id} was not found")
            }
            Self::RemoteConfig(message) => write!(f, "remote configuration error: {message}"),
            Self::RemoteAuth(message) => write!(f, "remote authentication failed: {message}"),
            Self::RemoteTransport(message) => write!(f, "remote transport error: {message}"),
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Store(error) => Some(error),
            Self::Pack(error) => Some(error),
            Self::Loom(error) => Some(error),
            Self::NoLocalRevision
            | Self::MissingRevision(_)
            | Self::MissingObjectPayload(_)
            | Self::InvalidCursor(_)
            | Self::CursorLockBusy { .. }
            | Self::CursorConflict { .. }
            | Self::DivergentState { .. }
            | Self::MissingRemotePack(_)
            | Self::RemoteConfig(_)
            | Self::RemoteAuth(_)
            | Self::RemoteTransport(_) => None,
        }
    }
}

impl From<StoreError> for SyncError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<loom_core::LoomError> for SyncError {
    fn from(error: loom_core::LoomError) -> Self {
        Self::Loom(error)
    }
}

pub type SyncResult<T> = Result<T, SyncError>;

fn format_revision(revision_id: Option<&FolderRevisionId>) -> String {
    revision_id
        .map(ToString::to_string)
        .unwrap_or_else(|| "-".to_string())
}

fn create_dir_all(path: impl AsRef<Path>) -> SyncResult<()> {
    let path = path.as_ref();
    fs::create_dir_all(path).map_err(|source| SyncError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::{HydrationState, RevisionBoundary};
    use loom_store::LocalStore;
    use std::fs;

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

    #[test]
    fn local_filesystem_remote_moves_pack_and_cursor() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("source");
        fs::create_dir_all(&folder).expect("folder creates");
        let store = LocalStore::open_or_init(&folder)
            .expect("store initializes")
            .into_store();
        let object = store
            .object_cache()
            .write_bytes(b"hello\n")
            .expect("object writes");
        let version = loom_core::FileVersion::new(
            loom_core::FileVersionId::new("file-version-1").expect("file version id"),
            "README.md",
            loom_core::FileKind::File,
            Some(object.id().clone()),
            Some(object.size_bytes()),
            "unix:1",
        )
        .expect("file version");
        let revision = store
            .coalesce_folder_revision(RevisionBoundary::Sync, &[version])
            .expect("revision")
            .revision()
            .clone();
        let remote = LocalFilesystemRemote::new(dir.path().join("remote"));

        let report = sync_store_to_remote(&store, &remote).expect("sync succeeds");
        let pack = remote
            .get_pack(&revision.id().clone())
            .expect("pack exists");

        assert_eq!(report.latest_revision_id, *revision.id());
        assert_eq!(report.uploaded_objects, 1);
        assert_eq!(
            remote
                .get_cursor(DEFAULT_CURSOR_ID)
                .expect("cursor reads")
                .as_ref(),
            Some(revision.id())
        );
        assert_eq!(pack.manifest.latest_revision_id, *revision.id());
        assert_eq!(pack.manifest.object_count(), 1);
        assert_eq!(pack.inline_object_count(), 0);
        assert!(remote
            .has_object(object.id())
            .expect("remote object metadata reads"));
    }

    #[test]
    fn pack_import_records_hydrated_cache_metadata() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source_folder = dir.path().join("source");
        let target_folder = dir.path().join("target");
        fs::create_dir_all(&source_folder).expect("source folder creates");
        let source_store = LocalStore::open_or_init(&source_folder)
            .expect("source store initializes")
            .into_store();
        let object = source_store
            .object_cache()
            .write_bytes(b"hello from pack\n")
            .expect("source object writes");
        let version = loom_core::FileVersion::new(
            loom_core::FileVersionId::new("file-version-pack").expect("file version id"),
            "README.md",
            loom_core::FileKind::File,
            Some(object.id().clone()),
            Some(object.size_bytes()),
            "unix:1",
        )
        .expect("file version");
        let revision = source_store
            .coalesce_folder_revision(RevisionBoundary::Sync, &[version])
            .expect("revision")
            .revision()
            .clone();
        let pack = build_pack(&source_store, revision.id()).expect("pack builds");
        let target_store = LocalStore::init_clone(
            &target_folder,
            pack.manifest.shared_folder_id.clone(),
            pack.manifest.display_name.clone(),
        )
        .expect("target store initializes");

        let report = import_pack(&target_store, &pack).expect("pack imports");
        let entry = target_store
            .cache_entry(object.id())
            .expect("cache metadata reads")
            .expect("cache metadata exists");

        assert_eq!(report.imported_objects, 1);
        assert_eq!(entry.object_id(), object.id());
        assert_eq!(entry.hydration_state(), HydrationState::Hydrated);
        assert_eq!(entry.size_bytes(), Some(object.size_bytes()));
        assert_eq!(
            target_store
                .object_cache()
                .read(object.id())
                .expect("target object reads"),
            b"hello from pack\n"
        );
    }

    #[test]
    fn pack_import_can_fetch_metadata_only_objects_from_remote() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source_folder = dir.path().join("source");
        let target_folder = dir.path().join("target");
        fs::create_dir_all(&source_folder).expect("source folder creates");
        let source_store = LocalStore::open_or_init(&source_folder)
            .expect("source store initializes")
            .into_store();
        let object = source_store
            .object_cache()
            .write_bytes(b"hello from remote object\n")
            .expect("source object writes");
        let version = loom_core::FileVersion::new(
            loom_core::FileVersionId::new("file-version-remote-object").expect("file version id"),
            "README.md",
            loom_core::FileKind::File,
            Some(object.id().clone()),
            Some(object.size_bytes()),
            "unix:1",
        )
        .expect("file version");
        let revision = source_store
            .coalesce_folder_revision(RevisionBoundary::Sync, &[version])
            .expect("revision")
            .revision()
            .clone();
        let remote = LocalFilesystemRemote::new(dir.path().join("remote"));
        sync_store_to_remote(&source_store, &remote).expect("sync succeeds");
        let pack = remote.get_pack(revision.id()).expect("pack reads");
        let target_store = LocalStore::init_clone(
            &target_folder,
            pack.manifest.shared_folder_id.clone(),
            pack.manifest.display_name.clone(),
        )
        .expect("target store initializes");

        let report = import_pack_from_remote(&target_store, &pack, &remote).expect("pack imports");

        assert_eq!(report.imported_objects, 1);
        assert_eq!(pack.inline_object_count(), 0);
        assert_eq!(
            target_store
                .object_cache()
                .read(object.id())
                .expect("target object reads"),
            b"hello from remote object\n"
        );
    }

    #[test]
    fn divergent_remote_cursor_is_refused() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("source");
        fs::create_dir_all(&folder).expect("folder creates");
        let store = LocalStore::open_or_init(&folder)
            .expect("store initializes")
            .into_store();
        let object = store.object_cache().write_bytes(b"one\n").expect("object");
        let version = loom_core::FileVersion::new(
            loom_core::FileVersionId::new("file-version-1").expect("file version id"),
            "one.txt",
            loom_core::FileKind::File,
            Some(object.id().clone()),
            Some(object.size_bytes()),
            "unix:1",
        )
        .expect("file version");
        let local_revision = store
            .coalesce_folder_revision(RevisionBoundary::Sync, &[version])
            .expect("revision")
            .revision()
            .clone();
        let remote = LocalFilesystemRemote::new(dir.path().join("remote"));
        let other_revision =
            FolderRevisionId::new("folder-revision-b3-divergent").expect("revision id");
        remote
            .compare_and_set_cursor(DEFAULT_CURSOR_ID, None, &other_revision)
            .expect("cursor writes");

        let error = sync_store_to_remote(&store, &remote).expect_err("sync refuses");

        assert!(matches!(
            error,
            SyncError::DivergentState {
                remote_revision_id,
                local_revision_id
            } if remote_revision_id == other_revision && local_revision_id == *local_revision.id()
        ));
    }

    #[test]
    fn cursor_compare_and_set_refuses_when_lock_exists() {
        let dir = tempfile::tempdir().expect("temp dir");
        let remote = LocalFilesystemRemote::new(dir.path().join("remote"));
        remote.ensure_layout().expect("layout creates");
        let lock_path = remote
            .cursor_lock_path(DEFAULT_CURSOR_ID)
            .expect("lock path creates");
        fs::write(&lock_path, "held by test\n").expect("lock writes");
        let next = FolderRevisionId::new("folder-revision-b3-next").expect("revision id");

        let error = remote
            .compare_and_set_cursor(DEFAULT_CURSOR_ID, None, &next)
            .expect_err("locked cursor refuses compare-and-set");

        assert!(matches!(error, SyncError::CursorLockBusy { .. }));
        assert_eq!(
            remote
                .get_cursor(DEFAULT_CURSOR_ID)
                .expect("cursor remains readable"),
            None
        );
        assert!(lock_path.exists());
    }

    #[test]
    fn cursor_compare_and_set_rechecks_current_value() {
        let dir = tempfile::tempdir().expect("temp dir");
        let remote = LocalFilesystemRemote::new(dir.path().join("remote"));
        let first = FolderRevisionId::new("folder-revision-b3-first").expect("revision id");
        let second = FolderRevisionId::new("folder-revision-b3-second").expect("revision id");
        remote
            .compare_and_set_cursor(DEFAULT_CURSOR_ID, None, &first)
            .expect("first cursor writes");

        let error = remote
            .compare_and_set_cursor(DEFAULT_CURSOR_ID, None, &second)
            .expect_err("stale expectation refuses");

        assert!(matches!(
            error,
            SyncError::CursorConflict {
                expected,
                actual,
                attempted,
                ..
            } if expected.is_none() && actual == Some(first.clone()) && attempted == second
        ));
        assert_eq!(
            remote
                .get_cursor(DEFAULT_CURSOR_ID)
                .expect("cursor reads")
                .as_ref(),
            Some(&first)
        );
    }
}
