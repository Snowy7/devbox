//! Local Loom persistence boundary.
//!
//! Loom stores folder history beside the shared folder for this local-engine
//! milestone. The layout is intentionally small and explicit:
//!
//! - `.loom/objects/b3/<prefix>/<prefix>/<object>` stores content-addressed bytes.
//! - `.loom/metadata/cache_entries.tsv` records local object-byte hydration state.
//! - `.loom/metadata/file_versions.tsv` is an append-only file-version catalog.
//! - `.loom/metadata/revisions.tsv` is an append-only folder-revision index.
//! - `.loom/metadata/revisions/<revision>.tsv` stores revision entries.
//!
//! The old `devbox-store` crate is still compiled for alpha compatibility while
//! these responsibilities migrate into Loom-owned crates.

use loom_core::{
    CacheEntry, Checkpoint, CheckpointId, Cursor, FileKind, FileVersion, FileVersionId,
    FolderEntry, FolderRevision, FolderRevisionId, FolderScope, HydrationState, LoomError,
    ObjectId, Pin, PinId, RevisionBoundary, SharedFolder, SharedFolderId,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CRATE_ROLE: &str = "local Loom object and metadata store for shared-folder history";
pub const STORE_DIR: &str = ".loom";

const HASH_ALGORITHM_DIR: &str = "b3";
const OBJECTS_DIR: &str = "objects";
const TEMP_DIR: &str = "tmp";
const METADATA_DIR: &str = "metadata";
const SHARED_FOLDER_FILE: &str = "shared_folder.tsv";
const CACHE_ENTRIES_FILE: &str = "cache_entries.tsv";
const FILE_VERSIONS_FILE: &str = "file_versions.tsv";
const REVISIONS_FILE: &str = "revisions.tsv";
const REVISIONS_DIR: &str = "revisions";
const CHECKPOINTS_FILE: &str = "checkpoints.tsv";
const PINS_FILE: &str = "pins.tsv";
const REMOTES_FILE: &str = "remotes.tsv";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreBoundary {
    pub stores_objects: bool,
    pub stores_cache_metadata: bool,
    pub stores_file_versions: bool,
    pub stores_folder_revisions: bool,
    pub stores_cursors: bool,
}

impl StoreBoundary {
    pub fn loom_owned() -> Self {
        Self {
            stores_objects: true,
            stores_cache_metadata: true,
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
pub struct ObjectRef {
    id: ObjectId,
    path: PathBuf,
    size_bytes: u64,
}

impl ObjectRef {
    pub fn id(&self) -> &ObjectId {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn object_ref(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            OBJECTS_DIR,
            HASH_ALGORITHM_DIR,
            &self.id.as_str()[0..2],
            &self.id.as_str()[2..4],
            self.id
        )
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

#[derive(Debug, Clone)]
pub struct ObjectCache {
    root: PathBuf,
}

impl ObjectCache {
    pub fn open(root: impl AsRef<Path>) -> StoreResult<Self> {
        let root = root.as_ref().to_path_buf();
        create_dir_all(root.join(OBJECTS_DIR).join(HASH_ALGORITHM_DIR))?;
        create_dir_all(root.join(TEMP_DIR))?;

        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write_bytes(&self, bytes: impl AsRef<[u8]>) -> StoreResult<ObjectRef> {
        self.write_reader(bytes.as_ref())
    }

    pub fn import_bytes(
        &self,
        expected_id: &ObjectId,
        bytes: impl AsRef<[u8]>,
    ) -> StoreResult<ObjectRef> {
        let object = self.write_bytes(bytes)?;
        if object.id() != expected_id {
            return Err(StoreError::ObjectHashMismatch {
                expected: expected_id.clone(),
                actual: object.id().clone(),
            });
        }

        Ok(object)
    }

    pub fn write_file(&self, path: impl AsRef<Path>) -> StoreResult<ObjectRef> {
        let path = path.as_ref();
        let file = File::open(path).map_err(|source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        self.write_reader(BufReader::new(file))
    }

    pub fn read(&self, id: &ObjectId) -> StoreResult<Vec<u8>> {
        let path = self.path_for(id);
        match fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                Err(StoreError::MissingObject {
                    id: id.clone(),
                    path,
                })
            }
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }

    pub fn exists(&self, id: &ObjectId) -> bool {
        self.path_for(id).is_file()
    }

    pub fn path_for(&self, id: &ObjectId) -> PathBuf {
        self.root
            .join(OBJECTS_DIR)
            .join(HASH_ALGORITHM_DIR)
            .join(&id.as_str()[0..2])
            .join(&id.as_str()[2..4])
            .join(id.as_str())
    }

    fn write_reader(&self, mut reader: impl Read) -> StoreResult<ObjectRef> {
        let (mut temp_file, temp_path) = self.create_temp_file()?;
        let mut hasher = blake3::Hasher::new();
        let mut size_bytes = 0;
        let mut buffer = [0; 64 * 1024];

        loop {
            let bytes_read = match reader.read(&mut buffer) {
                Ok(bytes_read) => bytes_read,
                Err(source) => {
                    cleanup_temp_file(&temp_path);
                    return Err(StoreError::Io {
                        path: temp_path,
                        source,
                    });
                }
            };

            if bytes_read == 0 {
                break;
            }

            hasher.update(&buffer[..bytes_read]);
            size_bytes += bytes_read as u64;

            if let Err(source) = temp_file.write_all(&buffer[..bytes_read]) {
                cleanup_temp_file(&temp_path);
                return Err(StoreError::Io {
                    path: temp_path,
                    source,
                });
            }
        }

        if let Err(source) = temp_file.flush().and_then(|_| temp_file.sync_all()) {
            cleanup_temp_file(&temp_path);
            return Err(StoreError::Io {
                path: temp_path,
                source,
            });
        }

        drop(temp_file);

        let id = ObjectId::from_blake3_hex(hasher.finalize().to_hex().to_string())
            .expect("BLAKE3 returns a 64-character hex digest");
        let final_path = self.path_for(&id);
        create_dir_all(
            final_path
                .parent()
                .expect("object paths are always nested below cache root"),
        )?;

        if final_path.exists() {
            cleanup_temp_file(&temp_path);
        } else if let Err(source) = fs::rename(&temp_path, &final_path) {
            if source.kind() == io::ErrorKind::AlreadyExists && final_path.exists() {
                cleanup_temp_file(&temp_path);
            } else {
                cleanup_temp_file(&temp_path);
                return Err(StoreError::Io {
                    path: final_path,
                    source,
                });
            }
        }

        Ok(ObjectRef {
            id,
            path: final_path,
            size_bytes,
        })
    }

    fn create_temp_file(&self) -> StoreResult<(File, PathBuf)> {
        let temp_dir = self.root.join(TEMP_DIR);
        create_dir_all(&temp_dir)?;

        for _ in 0..100 {
            let path = temp_dir.join(temp_file_name());
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((file, path)),
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(StoreError::Io { path, source });
                }
            }
        }

        Err(StoreError::CorruptMetadata {
            path: temp_dir,
            message: "could not create a unique object cache temp file".to_string(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct LocalStore {
    folder_root: PathBuf,
    store_root: PathBuf,
    shared_folder: SharedFolder,
    object_cache: ObjectCache,
}

#[derive(Debug, Clone)]
pub struct StoreOpen {
    store: LocalStore,
    initialized: bool,
}

impl StoreOpen {
    pub fn store(&self) -> &LocalStore {
        &self.store
    }

    pub fn into_store(self) -> LocalStore {
        self.store
    }

    pub fn initialized(&self) -> bool {
        self.initialized
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoalescedRevision {
    revision: FolderRevision,
    diff: RevisionDiffSummary,
    new_file_versions: usize,
    created: bool,
}

impl CoalescedRevision {
    pub fn revision(&self) -> &FolderRevision {
        &self.revision
    }

    pub fn diff(&self) -> &RevisionDiffSummary {
        &self.diff
    }

    pub fn new_file_versions(&self) -> usize {
        self.new_file_versions
    }

    pub fn created(&self) -> bool {
        self.created
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RevisionDiffSummary {
    created: usize,
    modified: usize,
    deleted: usize,
    unchanged: usize,
}

impl RevisionDiffSummary {
    pub fn created(&self) -> usize {
        self.created
    }

    pub fn modified(&self) -> usize {
        self.modified
    }

    pub fn deleted(&self) -> usize {
        self.deleted
    }

    pub fn unchanged(&self) -> usize {
        self.unchanged
    }

    pub fn has_changes(&self) -> bool {
        self.created > 0 || self.modified > 0 || self.deleted > 0
    }

    fn compare(base: Option<&[FolderEntry]>, current: &[FolderEntry]) -> Self {
        let base_entries = base
            .unwrap_or_default()
            .iter()
            .map(|entry| (entry.path().to_path_buf(), entry.file_version_id().clone()))
            .collect::<BTreeMap<_, _>>();
        let current_entries = current
            .iter()
            .map(|entry| (entry.path().to_path_buf(), entry.file_version_id().clone()))
            .collect::<BTreeMap<_, _>>();
        let mut summary = Self::default();

        for (path, current_id) in &current_entries {
            match base_entries.get(path) {
                Some(base_id) if base_id == current_id => summary.unchanged += 1,
                Some(_) => summary.modified += 1,
                None => summary.created += 1,
            }
        }

        for path in base_entries.keys() {
            if !current_entries.contains_key(path) {
                summary.deleted += 1;
            }
        }

        summary
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFolderState {
    pub revision: FolderRevision,
    pub file_versions: Vec<FileVersion>,
    pub cursors: Vec<Cursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreExport {
    pub shared_folder_id: SharedFolderId,
    pub display_name: String,
    pub file_versions: Vec<FileVersion>,
    pub revisions: Vec<FolderRevision>,
    pub checkpoints: Vec<Checkpoint>,
    pub pins: Vec<Pin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConfig {
    name: String,
    kind: String,
    location: String,
}

impl RemoteConfig {
    pub fn new(
        name: impl Into<String>,
        kind: impl Into<String>,
        location: impl Into<String>,
    ) -> StoreResult<Self> {
        let name = non_empty_metadata_value("remote name", name.into())?;
        let kind = non_empty_metadata_value("remote kind", kind.into())?;
        let location = non_empty_metadata_value("remote location", location.into())?;

        Ok(Self {
            name,
            kind,
            location,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn location(&self) -> &str {
        &self.location
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedRevisionTarget {
    Revision(FolderRevision),
    Checkpoint {
        checkpoint: Checkpoint,
        revision: FolderRevision,
    },
}

impl ResolvedRevisionTarget {
    pub fn revision(&self) -> &FolderRevision {
        match self {
            Self::Revision(revision) => revision,
            Self::Checkpoint { revision, .. } => revision,
        }
    }

    pub fn checkpoint(&self) -> Option<&Checkpoint> {
        match self {
            Self::Revision(_) => None,
            Self::Checkpoint { checkpoint, .. } => Some(checkpoint),
        }
    }
}

#[derive(Debug)]
pub enum StoreError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Loom(LoomError),
    MissingStore {
        folder: PathBuf,
    },
    MissingObject {
        id: ObjectId,
        path: PathBuf,
    },
    ObjectHashMismatch {
        expected: ObjectId,
        actual: ObjectId,
    },
    MissingRevisionTarget {
        target: String,
    },
    AmbiguousRevisionTarget {
        target: String,
        candidates: Vec<String>,
    },
    CorruptMetadata {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not access {}: {source}", path.display()),
            Self::Loom(error) => write!(f, "{error}"),
            Self::MissingStore { folder } => write!(
                f,
                "{} is not tracked by Loom yet; run 'loom track {}'",
                folder.display(),
                folder.display()
            ),
            Self::MissingObject { id, path } => {
                write!(f, "object {id} is missing at {}", path.display())
            }
            Self::ObjectHashMismatch { expected, actual } => {
                write!(f, "object hash mismatch: expected {expected}, got {actual}")
            }
            Self::MissingRevisionTarget { target } => write!(
                f,
                "revision or checkpoint '{target}' does not match local Loom history"
            ),
            Self::AmbiguousRevisionTarget { target, candidates } => write!(
                f,
                "revision or checkpoint '{target}' is ambiguous: {}",
                candidates.join(", ")
            ),
            Self::CorruptMetadata { path, message } => {
                write!(
                    f,
                    "could not read Loom metadata {}: {message}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Loom(error) => Some(error),
            Self::MissingStore { .. }
            | Self::MissingObject { .. }
            | Self::ObjectHashMismatch { .. }
            | Self::MissingRevisionTarget { .. }
            | Self::AmbiguousRevisionTarget { .. }
            | Self::CorruptMetadata { .. } => None,
        }
    }
}

impl From<LoomError> for StoreError {
    fn from(error: LoomError) -> Self {
        Self::Loom(error)
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

impl LocalStore {
    pub fn open_or_init(folder: impl AsRef<Path>) -> StoreResult<StoreOpen> {
        let folder_root = canonical_folder(folder.as_ref())?;
        let store_root = folder_root.join(STORE_DIR);
        let initialized = !store_root
            .join(METADATA_DIR)
            .join(SHARED_FOLDER_FILE)
            .is_file();

        create_dir_all(&store_root)?;
        create_dir_all(store_root.join(METADATA_DIR))?;
        create_dir_all(store_root.join(METADATA_DIR).join(REVISIONS_DIR))?;
        let object_cache = ObjectCache::open(&store_root)?;

        let shared_folder = if initialized {
            let shared_folder = default_shared_folder(&folder_root)?;
            write_shared_folder_metadata(&store_root, &shared_folder)?;
            shared_folder
        } else {
            read_shared_folder_metadata(&store_root, &folder_root)?
        };

        Ok(StoreOpen {
            store: Self {
                folder_root,
                store_root,
                shared_folder,
                object_cache,
            },
            initialized,
        })
    }

    pub fn init_clone(
        folder: impl AsRef<Path>,
        shared_folder_id: SharedFolderId,
        display_name: impl Into<String>,
    ) -> StoreResult<Self> {
        let folder = folder.as_ref();
        if !folder.exists() {
            create_dir_all(folder)?;
        }
        let folder_root = canonical_folder(folder)?;
        let store_root = folder_root.join(STORE_DIR);
        create_dir_all(&store_root)?;
        create_dir_all(store_root.join(METADATA_DIR))?;
        create_dir_all(store_root.join(METADATA_DIR).join(REVISIONS_DIR))?;
        let object_cache = ObjectCache::open(&store_root)?;
        let shared_folder = SharedFolder::new(
            shared_folder_id,
            &folder_root,
            display_name,
            FolderScope::WholeFolder,
        )?;
        write_shared_folder_metadata(&store_root, &shared_folder)?;

        Ok(Self {
            folder_root,
            store_root,
            shared_folder,
            object_cache,
        })
    }

    pub fn open(folder: impl AsRef<Path>) -> StoreResult<Self> {
        let folder_root = canonical_folder(folder.as_ref())?;
        let store_root = folder_root.join(STORE_DIR);
        let metadata = store_root.join(METADATA_DIR).join(SHARED_FOLDER_FILE);
        if !metadata.is_file() {
            return Err(StoreError::MissingStore {
                folder: folder_root,
            });
        }

        let object_cache = ObjectCache::open(&store_root)?;
        let shared_folder = read_shared_folder_metadata(&store_root, &folder_root)?;

        Ok(Self {
            folder_root,
            store_root,
            shared_folder,
            object_cache,
        })
    }

    pub fn discover_from(start: impl AsRef<Path>) -> StoreResult<Self> {
        let mut current = if start.as_ref().is_dir() {
            canonical_folder(start.as_ref())?
        } else {
            canonical_folder(start.as_ref().parent().unwrap_or_else(|| Path::new(".")))?
        };

        loop {
            if current
                .join(STORE_DIR)
                .join(METADATA_DIR)
                .join(SHARED_FOLDER_FILE)
                .is_file()
            {
                return Self::open(&current);
            }
            if !current.pop() {
                return Err(StoreError::MissingStore {
                    folder: canonical_folder(start.as_ref())?,
                });
            }
        }
    }

    pub fn folder_root(&self) -> &Path {
        &self.folder_root
    }

    pub fn store_root(&self) -> &Path {
        &self.store_root
    }

    pub fn shared_folder(&self) -> &SharedFolder {
        &self.shared_folder
    }

    pub fn object_cache(&self) -> &ObjectCache {
        &self.object_cache
    }

    pub fn cache_entries(&self) -> StoreResult<Vec<CacheEntry>> {
        Ok(self.load_cache_entries()?.into_values().collect())
    }

    pub fn cache_entry(&self, object_id: &ObjectId) -> StoreResult<Option<CacheEntry>> {
        Ok(self.load_cache_entries()?.remove(object_id))
    }

    pub fn record_object_hydrated(&self, object: &ObjectRef) -> StoreResult<CacheEntry> {
        let entry = CacheEntry::new(
            object.id().clone(),
            HydrationState::Hydrated,
            object.object_ref(),
            Some(object.size_bytes()),
            current_timestamp(),
        )?;
        self.upsert_cache_entry(entry.clone())?;
        Ok(entry)
    }

    pub fn upsert_cache_entry(&self, entry: CacheEntry) -> StoreResult<()> {
        let mut entries = self.load_cache_entries()?;
        entries.insert(entry.object_id().clone(), entry);
        let entries = entries.into_values().collect::<Vec<_>>();
        self.write_cache_entries(&entries)
    }

    pub fn file_versions(&self) -> StoreResult<Vec<FileVersion>> {
        Ok(self.load_file_versions()?.into_values().collect())
    }

    pub fn revisions(&self) -> StoreResult<Vec<FolderRevision>> {
        let headers = self.load_revision_headers()?;
        headers
            .into_iter()
            .map(|header| self.read_revision(&header))
            .collect()
    }

    pub fn revision_by_id(&self, id: &FolderRevisionId) -> StoreResult<Option<FolderRevision>> {
        for header in self.load_revision_headers()? {
            if &header.id == id {
                return self.read_revision(&header).map(Some);
            }
        }

        Ok(None)
    }

    pub fn latest_revision(&self) -> StoreResult<Option<FolderRevision>> {
        let Some(header) = self.load_revision_headers()?.into_iter().last() else {
            return Ok(None);
        };

        self.read_revision(&header).map(Some)
    }

    pub fn checkpoints(&self) -> StoreResult<Vec<Checkpoint>> {
        self.load_checkpoints()
    }

    pub fn pins(&self) -> StoreResult<Vec<Pin>> {
        self.load_pins()
    }

    pub fn remotes(&self) -> StoreResult<Vec<RemoteConfig>> {
        self.load_remotes()
    }

    pub fn remote(&self, name: &str) -> StoreResult<Option<RemoteConfig>> {
        Ok(self
            .load_remotes()?
            .into_iter()
            .find(|remote| remote.name() == name))
    }

    pub fn upsert_remote(&self, remote: RemoteConfig) -> StoreResult<()> {
        let mut remotes = self
            .load_remotes()?
            .into_iter()
            .filter(|existing| existing.name() != remote.name())
            .collect::<Vec<_>>();
        remotes.push(remote);
        remotes.sort_by(|left, right| left.name().cmp(right.name()));
        self.write_remotes(&remotes)
    }

    pub fn export_state(&self) -> StoreResult<StoreExport> {
        Ok(StoreExport {
            shared_folder_id: self.shared_folder.id().clone(),
            display_name: self.shared_folder.display_name().to_string(),
            file_versions: self.file_versions()?,
            revisions: self.revisions()?,
            checkpoints: self.checkpoints()?,
            pins: self.pins()?,
        })
    }

    pub fn import_file_versions(&self, file_versions: &[FileVersion]) -> StoreResult<usize> {
        self.append_file_versions(file_versions)
    }

    pub fn import_revision(&self, revision: &FolderRevision) -> StoreResult<bool> {
        if let Some(existing) = self.revision_by_id(revision.id())? {
            if existing.entries() == revision.entries()
                && existing.parent_id() == revision.parent_id()
                && existing.boundary() == revision.boundary()
                && existing.created_at() == revision.created_at()
            {
                return Ok(false);
            }

            return Err(StoreError::CorruptMetadata {
                path: self
                    .metadata_path(REVISIONS_DIR)
                    .join(revision_entries_file_name(revision.id())),
                message: format!(
                    "revision {} already exists with different metadata",
                    revision.id()
                ),
            });
        }

        self.write_revision(revision)?;
        self.append_revision_header(revision)?;
        Ok(true)
    }

    pub fn import_checkpoint(&self, checkpoint: &Checkpoint) -> StoreResult<bool> {
        if self
            .checkpoints()?
            .iter()
            .any(|existing| existing.id() == checkpoint.id())
        {
            return Ok(false);
        }

        self.append_checkpoint(checkpoint)?;
        Ok(true)
    }

    pub fn import_pin(&self, pin: &Pin) -> StoreResult<bool> {
        if self
            .pins()?
            .iter()
            .any(|existing| existing.id() == pin.id())
        {
            return Ok(false);
        }

        self.append_pin(pin)?;
        Ok(true)
    }

    pub fn create_checkpoint(
        &self,
        revision: &FolderRevision,
        message: impl Into<String>,
    ) -> StoreResult<Checkpoint> {
        let message = message.into();
        let created_at = current_timestamp();
        let checkpoint = Checkpoint::new(
            checkpoint_id(revision.id().as_str(), &message, &created_at)?,
            revision.id().clone(),
            message,
            created_at.clone(),
        )?;

        self.append_checkpoint(&checkpoint)?;
        self.pin_revision(
            revision.id(),
            format!("checkpoint {}", checkpoint.id().as_str()),
        )?;

        Ok(checkpoint)
    }

    pub fn pin_revision(
        &self,
        revision_id: &FolderRevisionId,
        reason: impl Into<String>,
    ) -> StoreResult<Pin> {
        let reason = reason.into();
        let created_at = current_timestamp();
        let pin = Pin::new(
            pin_id(revision_id.as_str(), &reason, &created_at)?,
            revision_id.clone(),
            reason,
            created_at,
        )?;

        self.append_pin(&pin)?;
        Ok(pin)
    }

    pub fn resolve_revision_target(&self, target: &str) -> StoreResult<ResolvedRevisionTarget> {
        let target = target.trim();
        let revisions = self.revisions()?;
        let checkpoints = self.checkpoints()?;
        let mut matches = Vec::new();

        for revision in &revisions {
            if revision.id().as_str() == target {
                return Ok(ResolvedRevisionTarget::Revision(revision.clone()));
            }
        }

        for checkpoint in &checkpoints {
            if checkpoint.id().as_str() == target {
                return self.resolved_checkpoint(checkpoint.clone());
            }
        }

        let message_matches = checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.message() == target)
            .cloned()
            .collect::<Vec<_>>();
        match message_matches.len() {
            0 => {}
            1 => return self.resolved_checkpoint(message_matches[0].clone()),
            _ => {
                return Err(StoreError::AmbiguousRevisionTarget {
                    target: target.to_string(),
                    candidates: message_matches
                        .iter()
                        .map(|checkpoint| checkpoint.id().to_string())
                        .collect(),
                });
            }
        }

        for revision in revisions {
            if revision.id().as_str().starts_with(target) {
                matches.push(ResolvedRevisionTarget::Revision(revision));
            }
        }

        for checkpoint in checkpoints {
            if checkpoint.id().as_str().starts_with(target) {
                matches.push(self.resolved_checkpoint(checkpoint)?);
            }
        }

        match matches.len() {
            0 => Err(StoreError::MissingRevisionTarget {
                target: target.to_string(),
            }),
            1 => Ok(matches.remove(0)),
            _ => Err(StoreError::AmbiguousRevisionTarget {
                target: target.to_string(),
                candidates: matches
                    .iter()
                    .map(|candidate| match candidate {
                        ResolvedRevisionTarget::Revision(revision) => revision.id().to_string(),
                        ResolvedRevisionTarget::Checkpoint { checkpoint, .. } => {
                            checkpoint.id().to_string()
                        }
                    })
                    .collect(),
            }),
        }
    }

    pub fn coalesce_folder_revision(
        &self,
        boundary: RevisionBoundary,
        file_versions: &[FileVersion],
    ) -> StoreResult<CoalescedRevision> {
        let new_file_versions = self.append_file_versions(file_versions)?;
        let latest = self.latest_revision()?;
        let entries = entries_from_file_versions(file_versions)?;
        let diff = RevisionDiffSummary::compare(
            latest.as_ref().map(|revision| revision.entries()),
            &entries,
        );

        if let Some(latest) = latest {
            if same_entries(latest.entries(), &entries) {
                return Ok(CoalescedRevision {
                    revision: latest,
                    diff,
                    new_file_versions,
                    created: false,
                });
            }

            return self.create_revision(
                Some(latest.id().clone()),
                boundary,
                entries,
                diff,
                new_file_versions,
            );
        }

        self.create_revision(None, boundary, entries, diff, new_file_versions)
    }

    fn create_revision(
        &self,
        parent_id: Option<FolderRevisionId>,
        boundary: RevisionBoundary,
        entries: Vec<FolderEntry>,
        diff: RevisionDiffSummary,
        new_file_versions: usize,
    ) -> StoreResult<CoalescedRevision> {
        let created_at = current_timestamp();
        let id = folder_revision_id(
            self.shared_folder.id().as_str(),
            parent_id.as_ref().map(FolderRevisionId::as_str),
            boundary,
            &entries,
            &created_at,
        )?;
        let revision = FolderRevision::new(
            id,
            self.shared_folder.id().clone(),
            parent_id,
            boundary,
            entries,
            created_at,
        )?;

        self.write_revision(&revision)?;
        self.append_revision_header(&revision)?;

        Ok(CoalescedRevision {
            revision,
            diff,
            new_file_versions,
            created: true,
        })
    }

    fn append_file_versions(&self, file_versions: &[FileVersion]) -> StoreResult<usize> {
        let existing = self
            .load_file_versions()?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let path = self.metadata_path(FILE_VERSIONS_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;
        let mut written = 0;

        for version in file_versions {
            if existing.contains(version.id()) {
                continue;
            }
            file.write_all(file_version_row(version).as_bytes())
                .map_err(|source| StoreError::Io {
                    path: path.clone(),
                    source,
                })?;
            written += 1;
        }

        Ok(written)
    }

    fn resolved_checkpoint(&self, checkpoint: Checkpoint) -> StoreResult<ResolvedRevisionTarget> {
        let revision = self
            .revision_by_id(checkpoint.revision_id())?
            .ok_or_else(|| StoreError::CorruptMetadata {
                path: self.metadata_path(CHECKPOINTS_FILE),
                message: format!(
                    "checkpoint {} references missing revision {}",
                    checkpoint.id(),
                    checkpoint.revision_id()
                ),
            })?;

        Ok(ResolvedRevisionTarget::Checkpoint {
            checkpoint,
            revision,
        })
    }

    fn load_cache_entries(&self) -> StoreResult<BTreeMap<ObjectId, CacheEntry>> {
        let path = self.metadata_path(CACHE_ENTRIES_FILE);
        let Some(contents) = read_optional_to_string(&path)? else {
            return Ok(BTreeMap::new());
        };
        let mut entries = BTreeMap::new();

        for (line_index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = split_fields(&path, line_index + 1, line, 5)?;
            let object_id = ObjectId::from_blake3_hex(decode_field(&fields[0])?)?;
            let hydration_state = hydration_state_from_store(&decode_field(&fields[1])?)
                .ok_or_else(|| StoreError::CorruptMetadata {
                    path: path.clone(),
                    message: format!("line {} has unknown hydration state", line_index + 1),
                })?;
            let local_path = store_string_to_path(&decode_field(&fields[2])?);
            let size_bytes = match decode_field(&fields[3])?.as_str() {
                "-" => None,
                value => Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| StoreError::CorruptMetadata {
                            path: path.clone(),
                            message: format!("line {} has invalid cache size", line_index + 1),
                        })?,
                ),
            };
            let updated_at = decode_field(&fields[4])?;
            let entry = CacheEntry::new(
                object_id.clone(),
                hydration_state,
                local_path,
                size_bytes,
                updated_at,
            )?;
            entries.insert(object_id, entry);
        }

        Ok(entries)
    }

    fn write_cache_entries(&self, entries: &[CacheEntry]) -> StoreResult<()> {
        let path = self.metadata_path(CACHE_ENTRIES_FILE);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;

        for entry in entries {
            file.write_all(cache_entry_row(entry).as_bytes())
                .map_err(|source| StoreError::Io {
                    path: path.clone(),
                    source,
                })?;
        }

        Ok(())
    }

    fn load_checkpoints(&self) -> StoreResult<Vec<Checkpoint>> {
        let path = self.metadata_path(CHECKPOINTS_FILE);
        let Some(contents) = read_optional_to_string(&path)? else {
            return Ok(Vec::new());
        };
        let mut checkpoints = Vec::new();

        for (line_index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = split_fields(&path, line_index + 1, line, 4)?;
            checkpoints.push(Checkpoint::new(
                CheckpointId::new(decode_field(&fields[0])?)?,
                FolderRevisionId::new(decode_field(&fields[1])?)?,
                decode_field(&fields[2])?,
                decode_field(&fields[3])?,
            )?);
        }

        Ok(checkpoints)
    }

    fn load_pins(&self) -> StoreResult<Vec<Pin>> {
        let path = self.metadata_path(PINS_FILE);
        let Some(contents) = read_optional_to_string(&path)? else {
            return Ok(Vec::new());
        };
        let mut pins = Vec::new();

        for (line_index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = split_fields(&path, line_index + 1, line, 4)?;
            pins.push(Pin::new(
                PinId::new(decode_field(&fields[0])?)?,
                FolderRevisionId::new(decode_field(&fields[1])?)?,
                decode_field(&fields[2])?,
                decode_field(&fields[3])?,
            )?);
        }

        Ok(pins)
    }

    fn load_remotes(&self) -> StoreResult<Vec<RemoteConfig>> {
        let path = self.metadata_path(REMOTES_FILE);
        let Some(contents) = read_optional_to_string(&path)? else {
            return Ok(Vec::new());
        };
        let mut remotes = Vec::new();

        for (line_index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = split_fields(&path, line_index + 1, line, 3)?;
            remotes.push(RemoteConfig::new(
                decode_field(&fields[0])?,
                decode_field(&fields[1])?,
                decode_field(&fields[2])?,
            )?);
        }

        Ok(remotes)
    }

    fn write_remotes(&self, remotes: &[RemoteConfig]) -> StoreResult<()> {
        let path = self.metadata_path(REMOTES_FILE);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;

        for remote in remotes {
            file.write_all(
                format!(
                    "{}\t{}\t{}\n",
                    encode_field(remote.name()),
                    encode_field(remote.kind()),
                    encode_field(remote.location())
                )
                .as_bytes(),
            )
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;
        }

        Ok(())
    }

    fn append_checkpoint(&self, checkpoint: &Checkpoint) -> StoreResult<()> {
        let path = self.metadata_path(CHECKPOINTS_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;

        file.write_all(
            format!(
                "{}\t{}\t{}\t{}\n",
                encode_field(checkpoint.id().as_str()),
                encode_field(checkpoint.revision_id().as_str()),
                encode_field(checkpoint.message()),
                encode_field(checkpoint.created_at()),
            )
            .as_bytes(),
        )
        .map_err(|source| StoreError::Io { path, source })
    }

    fn append_pin(&self, pin: &Pin) -> StoreResult<()> {
        let path = self.metadata_path(PINS_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;

        file.write_all(
            format!(
                "{}\t{}\t{}\t{}\n",
                encode_field(pin.id().as_str()),
                encode_field(pin.revision_id().as_str()),
                encode_field(pin.reason()),
                encode_field(pin.created_at()),
            )
            .as_bytes(),
        )
        .map_err(|source| StoreError::Io { path, source })
    }

    fn load_file_versions(&self) -> StoreResult<BTreeMap<FileVersionId, FileVersion>> {
        let path = self.metadata_path(FILE_VERSIONS_FILE);
        let Some(contents) = read_optional_to_string(&path)? else {
            return Ok(BTreeMap::new());
        };
        let mut versions = BTreeMap::new();

        for (line_index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = split_fields(&path, line_index + 1, line, 6)?;
            let id = FileVersionId::new(decode_field(&fields[0])?)?;
            let relative_path = store_string_to_path(&decode_field(&fields[1])?);
            let kind = file_kind_from_store(&decode_field(&fields[2])?).ok_or_else(|| {
                StoreError::CorruptMetadata {
                    path: path.clone(),
                    message: format!("line {} has unknown file kind", line_index + 1),
                }
            })?;
            let object_id = match decode_field(&fields[3])?.as_str() {
                "-" => None,
                value => Some(ObjectId::from_blake3_hex(value.to_string())?),
            };
            let size_bytes = match decode_field(&fields[4])?.as_str() {
                "-" => None,
                value => Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| StoreError::CorruptMetadata {
                            path: path.clone(),
                            message: format!("line {} has invalid size", line_index + 1),
                        })?,
                ),
            };
            let captured_at = decode_field(&fields[5])?;
            let version = FileVersion::new(
                id.clone(),
                relative_path,
                kind,
                object_id,
                size_bytes,
                captured_at,
            )?;
            versions.insert(id, version);
        }

        Ok(versions)
    }

    fn load_revision_headers(&self) -> StoreResult<Vec<RevisionHeader>> {
        let path = self.metadata_path(REVISIONS_FILE);
        let Some(contents) = read_optional_to_string(&path)? else {
            return Ok(Vec::new());
        };
        let mut headers = Vec::new();

        for (line_index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = split_fields(&path, line_index + 1, line, 4)?;
            let id = FolderRevisionId::new(decode_field(&fields[0])?)?;
            let parent_id = match decode_field(&fields[1])?.as_str() {
                "-" => None,
                value => Some(FolderRevisionId::new(value.to_string())?),
            };
            let boundary =
                revision_boundary_from_store(&decode_field(&fields[2])?).ok_or_else(|| {
                    StoreError::CorruptMetadata {
                        path: path.clone(),
                        message: format!("line {} has unknown revision boundary", line_index + 1),
                    }
                })?;
            let created_at = decode_field(&fields[3])?;
            headers.push(RevisionHeader {
                id,
                parent_id,
                boundary,
                created_at,
            });
        }

        Ok(headers)
    }

    fn read_revision(&self, header: &RevisionHeader) -> StoreResult<FolderRevision> {
        let path = self
            .metadata_path(REVISIONS_DIR)
            .join(revision_entries_file_name(&header.id));
        let contents = fs::read_to_string(&path).map_err(|source| StoreError::Io {
            path: path.clone(),
            source,
        })?;
        let mut entries = Vec::new();

        for (line_index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = split_fields(&path, line_index + 1, line, 2)?;
            let relative_path = store_string_to_path(&decode_field(&fields[0])?);
            let file_version_id = FileVersionId::new(decode_field(&fields[1])?)?;
            entries.push(FolderEntry::new(relative_path, file_version_id)?);
        }

        FolderRevision::new(
            header.id.clone(),
            self.shared_folder.id().clone(),
            header.parent_id.clone(),
            header.boundary,
            entries,
            header.created_at.clone(),
        )
        .map_err(StoreError::from)
    }

    fn write_revision(&self, revision: &FolderRevision) -> StoreResult<()> {
        let path = self
            .metadata_path(REVISIONS_DIR)
            .join(revision_entries_file_name(revision.id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;

        for entry in revision.entries() {
            file.write_all(
                format!(
                    "{}\t{}\n",
                    encode_field(&path_to_store_string(entry.path())),
                    encode_field(entry.file_version_id().as_str())
                )
                .as_bytes(),
            )
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;
        }

        Ok(())
    }

    fn append_revision_header(&self, revision: &FolderRevision) -> StoreResult<()> {
        let path = self.metadata_path(REVISIONS_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;

        file.write_all(
            format!(
                "{}\t{}\t{}\t{}\n",
                encode_field(revision.id().as_str()),
                encode_field(
                    revision
                        .parent_id()
                        .map(FolderRevisionId::as_str)
                        .unwrap_or("-")
                ),
                encode_field(revision_boundary_to_store(revision.boundary())),
                encode_field(revision.created_at()),
            )
            .as_bytes(),
        )
        .map_err(|source| StoreError::Io { path, source })
    }

    fn metadata_path(&self, name: &str) -> PathBuf {
        self.store_root.join(METADATA_DIR).join(name)
    }
}

#[derive(Debug, Clone)]
struct RevisionHeader {
    id: FolderRevisionId,
    parent_id: Option<FolderRevisionId>,
    boundary: RevisionBoundary,
    created_at: String,
}

fn default_shared_folder(folder_root: &Path) -> StoreResult<SharedFolder> {
    let root_key = path_to_store_string(folder_root);
    let digest = blake3::hash(root_key.as_bytes()).to_hex().to_string();
    let id = SharedFolderId::new(format!("shared-folder-b3-{digest}"))?;
    let display_name = folder_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| folder_root.display().to_string());

    SharedFolder::new(id, folder_root, display_name, FolderScope::WholeFolder).map_err(Into::into)
}

fn non_empty_metadata_value(kind: &'static str, value: String) -> StoreResult<String> {
    if value.trim().is_empty() {
        return Err(StoreError::CorruptMetadata {
            path: PathBuf::from("<metadata>"),
            message: format!("{kind} cannot be empty"),
        });
    }

    Ok(value)
}

fn write_shared_folder_metadata(
    store_root: &Path,
    shared_folder: &SharedFolder,
) -> StoreResult<()> {
    let path = store_root.join(METADATA_DIR).join(SHARED_FOLDER_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .map_err(|source| StoreError::Io {
            path: path.clone(),
            source,
        })?;

    file.write_all(
        format!(
            "version\t1\nid\t{}\ndisplay_name\t{}\n",
            encode_field(shared_folder.id().as_str()),
            encode_field(shared_folder.display_name()),
        )
        .as_bytes(),
    )
    .map_err(|source| StoreError::Io { path, source })
}

fn read_shared_folder_metadata(store_root: &Path, folder_root: &Path) -> StoreResult<SharedFolder> {
    let path = store_root.join(METADATA_DIR).join(SHARED_FOLDER_FILE);
    let contents = fs::read_to_string(&path).map_err(|source| StoreError::Io {
        path: path.clone(),
        source,
    })?;
    let mut id = None;
    let mut display_name = None;

    for (line_index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields = split_fields(&path, line_index + 1, line, 2)?;
        let key = decode_field(&fields[0])?;
        let value = decode_field(&fields[1])?;
        match key.as_str() {
            "id" => id = Some(SharedFolderId::new(value)?),
            "display_name" => display_name = Some(value),
            "version" => {}
            _ => {}
        }
    }

    let id = id.ok_or_else(|| StoreError::CorruptMetadata {
        path: path.clone(),
        message: "missing shared folder id".to_string(),
    })?;
    let display_name = display_name.ok_or_else(|| StoreError::CorruptMetadata {
        path,
        message: "missing shared folder display name".to_string(),
    })?;

    SharedFolder::new(id, folder_root, display_name, FolderScope::WholeFolder).map_err(Into::into)
}

fn entries_from_file_versions(file_versions: &[FileVersion]) -> StoreResult<Vec<FolderEntry>> {
    let mut entries = file_versions
        .iter()
        .map(|version| FolderEntry::new(version.path().to_path_buf(), version.id().clone()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|left, right| {
        path_to_store_string(left.path()).cmp(&path_to_store_string(right.path()))
    });
    Ok(entries)
}

fn same_entries(left: &[FolderEntry], right: &[FolderEntry]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.path() == right.path() && left.file_version_id() == right.file_version_id()
        })
}

fn folder_revision_id(
    shared_folder_id: &str,
    parent_id: Option<&str>,
    boundary: RevisionBoundary,
    entries: &[FolderEntry],
    created_at: &str,
) -> StoreResult<FolderRevisionId> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"loom-folder-revision-v1\n");
    hasher.update(shared_folder_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(parent_id.unwrap_or("-").as_bytes());
    hasher.update(b"\n");
    hasher.update(revision_boundary_to_store(boundary).as_bytes());
    hasher.update(b"\n");
    hasher.update(created_at.as_bytes());
    for entry in entries {
        hasher.update(b"\n");
        hasher.update(path_to_store_string(entry.path()).as_bytes());
        hasher.update(b"\t");
        hasher.update(entry.file_version_id().as_str().as_bytes());
    }

    FolderRevisionId::new(format!("folder-revision-b3-{}", hasher.finalize().to_hex()))
        .map_err(Into::into)
}

fn checkpoint_id(revision_id: &str, message: &str, created_at: &str) -> StoreResult<CheckpointId> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"loom-checkpoint-v1\n");
    hasher.update(revision_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(message.as_bytes());
    hasher.update(b"\n");
    hasher.update(created_at.as_bytes());

    CheckpointId::new(format!("checkpoint-b3-{}", hasher.finalize().to_hex())).map_err(Into::into)
}

fn pin_id(revision_id: &str, reason: &str, created_at: &str) -> StoreResult<PinId> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"loom-pin-v1\n");
    hasher.update(revision_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(reason.as_bytes());
    hasher.update(b"\n");
    hasher.update(created_at.as_bytes());

    PinId::new(format!("pin-b3-{}", hasher.finalize().to_hex())).map_err(Into::into)
}

fn file_version_row(version: &FileVersion) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        encode_field(version.id().as_str()),
        encode_field(&path_to_store_string(version.path())),
        encode_field(file_kind_to_store(version.kind())),
        encode_field(version.object_id().map(ObjectId::as_str).unwrap_or("-")),
        encode_field(
            &version
                .size_bytes()
                .map(|size| size.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
        encode_field(version.captured_at()),
    )
}

fn cache_entry_row(entry: &CacheEntry) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\n",
        encode_field(entry.object_id().as_str()),
        encode_field(hydration_state_to_store(entry.hydration_state())),
        encode_field(&path_to_store_string(entry.local_path())),
        encode_field(
            &entry
                .size_bytes()
                .map(|size| size.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
        encode_field(entry.updated_at()),
    )
}

fn file_kind_to_store(kind: &FileKind) -> &'static str {
    match kind {
        FileKind::File => "file",
        FileKind::Directory => "directory",
        FileKind::Symlink => "symlink",
        FileKind::Unsupported => "unsupported",
    }
}

fn file_kind_from_store(value: &str) -> Option<FileKind> {
    match value {
        "file" => Some(FileKind::File),
        "directory" => Some(FileKind::Directory),
        "symlink" => Some(FileKind::Symlink),
        "unsupported" => Some(FileKind::Unsupported),
        _ => None,
    }
}

fn hydration_state_to_store(state: HydrationState) -> &'static str {
    match state {
        HydrationState::RemoteOnly => "remote-only",
        HydrationState::Partial => "partial",
        HydrationState::Hydrated => "hydrated",
    }
}

fn hydration_state_from_store(value: &str) -> Option<HydrationState> {
    match value {
        "remote-only" => Some(HydrationState::RemoteOnly),
        "partial" => Some(HydrationState::Partial),
        "hydrated" => Some(HydrationState::Hydrated),
        _ => None,
    }
}

pub fn revision_boundary_to_store(boundary: RevisionBoundary) -> &'static str {
    match boundary {
        RevisionBoundary::DebounceWindow => "debounce-window",
        RevisionBoundary::LoomCommand => "loom-command",
        RevisionBoundary::Sync => "sync",
        RevisionBoundary::Restore => "restore",
        RevisionBoundary::SandboxMerge => "sandbox-merge",
        RevisionBoundary::Checkpoint => "checkpoint",
    }
}

fn revision_boundary_from_store(value: &str) -> Option<RevisionBoundary> {
    match value {
        "debounce-window" => Some(RevisionBoundary::DebounceWindow),
        "loom-command" => Some(RevisionBoundary::LoomCommand),
        "sync" => Some(RevisionBoundary::Sync),
        "restore" => Some(RevisionBoundary::Restore),
        "sandbox-merge" => Some(RevisionBoundary::SandboxMerge),
        "checkpoint" => Some(RevisionBoundary::Checkpoint),
        _ => None,
    }
}

pub fn path_to_store_string(path: &Path) -> String {
    let parts = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::CurDir => Some(".".to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();

    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn store_string_to_path(value: &str) -> PathBuf {
    if value == "." {
        return PathBuf::new();
    }

    value.split('/').collect()
}

fn split_fields(
    path: &Path,
    line_number: usize,
    line: &str,
    expected: usize,
) -> StoreResult<Vec<String>> {
    let fields = line
        .split('\t')
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if fields.len() != expected {
        return Err(StoreError::CorruptMetadata {
            path: path.to_path_buf(),
            message: format!(
                "line {line_number} has {} fields, expected {expected}",
                fields.len()
            ),
        });
    }

    Ok(fields)
}

fn encode_field(value: &str) -> String {
    let mut encoded = String::new();
    for character in value.chars() {
        match character {
            '%' => encoded.push_str("%25"),
            '\t' => encoded.push_str("%09"),
            '\n' => encoded.push_str("%0A"),
            '\r' => encoded.push_str("%0D"),
            _ => encoded.push(character),
        }
    }
    encoded
}

fn decode_field(value: &str) -> StoreResult<String> {
    let mut decoded = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(StoreError::CorruptMetadata {
                    path: PathBuf::from("<field>"),
                    message: "truncated percent escape".to_string(),
                });
            }
            let hex = &value[index + 1..index + 3];
            let byte = u8::from_str_radix(hex, 16).map_err(|_| StoreError::CorruptMetadata {
                path: PathBuf::from("<field>"),
                message: "invalid percent escape".to_string(),
            })?;
            decoded.push(byte as char);
            index += 3;
        } else {
            let character = value[index..]
                .chars()
                .next()
                .expect("index is inside the string");
            decoded.push(character);
            index += character.len_utf8();
        }
    }
    Ok(decoded)
}

fn read_optional_to_string(path: &Path) -> StoreResult<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StoreError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn create_dir_all(path: impl AsRef<Path>) -> StoreResult<()> {
    let path = path.as_ref();
    fs::create_dir_all(path).map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn canonical_folder(folder: &Path) -> StoreResult<PathBuf> {
    if !folder.exists() {
        return Err(StoreError::Io {
            path: folder.to_path_buf(),
            source: io::Error::new(io::ErrorKind::NotFound, "folder does not exist"),
        });
    }
    if !folder.is_dir() {
        return Err(StoreError::Io {
            path: folder.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "path is not a folder"),
        });
    }

    fs::canonicalize(folder).map_err(|source| StoreError::Io {
        path: folder.to_path_buf(),
        source,
    })
}

fn current_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    format!("unix:{}", duration.as_secs())
}

fn revision_entries_file_name(id: &FolderRevisionId) -> String {
    format!("{}.tsv", id.as_str())
}

fn temp_file_name() -> String {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    format!("object-{}-{nanos}-{counter}.tmp", process::id())
}

fn cleanup_temp_file(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn store_boundary_names_loom_owned_state() {
        let boundary = StoreBoundary::loom_owned();

        assert!(boundary.stores_objects);
        assert!(boundary.stores_cache_metadata);
        assert!(boundary.stores_file_versions);
        assert!(boundary.stores_folder_revisions);
        assert!(boundary.stores_cursors);
        assert!(CRATE_ROLE.contains("Loom"));
    }

    #[test]
    fn object_cache_writes_bytes_by_content_identity() {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = ObjectCache::open(dir.path()).expect("cache opens");
        let content = b"hello from Loom objects";

        let object = cache.write_bytes(content).expect("object writes");
        let duplicate = cache.write_bytes(content).expect("duplicate object writes");

        assert_eq!(object.id(), duplicate.id());
        assert_eq!(object.size_bytes(), content.len() as u64);
        assert!(object.object_ref().starts_with("objects/b3/"));
        assert_eq!(cache.read(object.id()).expect("object reads"), content);
        assert_eq!(count_files(&dir.path().join(OBJECTS_DIR)), 1);
    }

    #[test]
    fn object_bytes_can_have_separate_hydrated_cache_metadata() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("shared");
        fs::create_dir_all(&folder).expect("folder creates");
        let store = LocalStore::open_or_init(&folder)
            .expect("store initializes")
            .into_store();

        let object = store
            .object_cache()
            .write_bytes(b"cached source bytes")
            .expect("object writes");
        let entry = store
            .record_object_hydrated(&object)
            .expect("cache entry records");

        assert!(store.object_cache().exists(object.id()));
        assert_eq!(entry.object_id(), object.id());
        assert_eq!(entry.hydration_state(), HydrationState::Hydrated);
        assert_eq!(entry.size_bytes(), Some(object.size_bytes()));
        let object_ref = object.object_ref();
        assert_eq!(entry.local_path(), Path::new(object_ref.as_str()));

        let entries = store.cache_entries().expect("cache entries load");
        assert_eq!(entries, vec![entry]);
        assert!(store
            .file_versions()
            .expect("file versions load")
            .is_empty());
        assert!(store.revisions().expect("revisions load").is_empty());
    }

    #[test]
    fn cache_metadata_survives_reopen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("shared");
        fs::create_dir_all(&folder).expect("folder creates");
        let store = LocalStore::open_or_init(&folder)
            .expect("store initializes")
            .into_store();
        let object = store
            .object_cache()
            .write_bytes(b"durable cache metadata")
            .expect("object writes");
        let entry = store
            .record_object_hydrated(&object)
            .expect("cache entry records");

        let reopened = LocalStore::open(&folder).expect("store reopens");

        assert_eq!(
            reopened
                .cache_entry(object.id())
                .expect("cache entry loads after reopen"),
            Some(entry)
        );
        assert_eq!(
            reopened
                .object_cache()
                .read(object.id())
                .expect("object reads after reopen"),
            b"durable cache metadata"
        );
    }

    #[test]
    fn cache_metadata_rejects_unknown_hydration_states() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("shared");
        fs::create_dir_all(&folder).expect("folder creates");
        let store = LocalStore::open_or_init(&folder)
            .expect("store initializes")
            .into_store();
        let object = store
            .object_cache()
            .write_bytes(b"invalid cache state")
            .expect("object writes");
        fs::write(
            store.metadata_path(CACHE_ENTRIES_FILE),
            format!(
                "{}\tnot-a-state\t{}\t{}\tunix:1\n",
                object.id(),
                object.object_ref(),
                object.size_bytes()
            ),
        )
        .expect("cache metadata writes");

        let error = store
            .cache_entries()
            .expect_err("unknown hydration state should fail");
        assert!(matches!(error, StoreError::CorruptMetadata { .. }));
    }

    #[test]
    fn local_store_initializes_metadata_and_reopens() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("shared");
        fs::create_dir_all(&folder).expect("folder creates");

        let opened = LocalStore::open_or_init(&folder).expect("store initializes");
        assert!(opened.initialized());
        assert!(opened.store().store_root().join(METADATA_DIR).is_dir());

        let reopened = LocalStore::open_or_init(&folder).expect("store reopens");
        assert!(!reopened.initialized());
        assert_eq!(
            opened.store().shared_folder().id(),
            reopened.store().shared_folder().id()
        );
    }

    #[test]
    fn coalesces_file_versions_into_folder_revisions() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("shared");
        fs::create_dir_all(&folder).expect("folder creates");
        let store = LocalStore::open_or_init(&folder)
            .expect("store initializes")
            .into_store();
        let object = store
            .object_cache()
            .write_bytes(b"readme")
            .expect("object writes");
        let version = FileVersion::new(
            FileVersionId::new("file-version-1").expect("file version id"),
            "README.md",
            FileKind::File,
            Some(object.id().clone()),
            Some(object.size_bytes()),
            "unix:1",
        )
        .expect("file version creates");

        let first = store
            .coalesce_folder_revision(RevisionBoundary::LoomCommand, &[version.clone()])
            .expect("revision creates");
        let second = store
            .coalesce_folder_revision(RevisionBoundary::LoomCommand, &[version])
            .expect("unchanged revision reuses latest");

        assert!(first.created());
        assert_eq!(first.diff().created(), 1);
        assert!(!second.created());
        assert_eq!(store.revisions().expect("revisions list").len(), 1);
        assert_eq!(store.file_versions().expect("versions list").len(), 1);
    }

    #[test]
    fn checkpoints_are_durable_pins_on_folder_revisions() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("shared");
        fs::create_dir_all(&folder).expect("folder creates");
        let store = LocalStore::open_or_init(&folder)
            .expect("store initializes")
            .into_store();
        let object = store
            .object_cache()
            .write_bytes(b"readme")
            .expect("object writes");
        let version = FileVersion::new(
            FileVersionId::new("file-version-1").expect("file version id"),
            "README.md",
            FileKind::File,
            Some(object.id().clone()),
            Some(object.size_bytes()),
            "unix:1",
        )
        .expect("file version creates");
        let coalesced = store
            .coalesce_folder_revision(RevisionBoundary::LoomCommand, &[version])
            .expect("revision creates");

        let checkpoint = store
            .create_checkpoint(coalesced.revision(), "before change")
            .expect("checkpoint creates");
        let reopened = LocalStore::open(&folder).expect("store reopens");
        let checkpoints = reopened.checkpoints().expect("checkpoints load");
        let pins = reopened.pins().expect("pins load");

        assert_eq!(checkpoints, vec![checkpoint.clone()]);
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].revision_id(), coalesced.revision().id());

        match reopened
            .resolve_revision_target(checkpoint.id().as_str())
            .expect("checkpoint resolves")
        {
            ResolvedRevisionTarget::Checkpoint {
                checkpoint: resolved,
                revision,
            } => {
                assert_eq!(resolved, checkpoint);
                assert_eq!(revision.id(), coalesced.revision().id());
            }
            ResolvedRevisionTarget::Revision(_) => panic!("expected checkpoint target"),
        }

        match reopened
            .resolve_revision_target("before change")
            .expect("message resolves")
        {
            ResolvedRevisionTarget::Checkpoint { revision, .. } => {
                assert_eq!(revision.id(), coalesced.revision().id());
            }
            ResolvedRevisionTarget::Revision(_) => panic!("expected checkpoint target"),
        }
    }

    fn count_files(path: &Path) -> usize {
        let mut count = 0;
        let mut stack = vec![path.to_path_buf()];

        while let Some(path) = stack.pop() {
            for entry in fs::read_dir(path).expect("directory reads") {
                let entry = entry.expect("directory entry reads");
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    stack.push(entry_path);
                } else {
                    count += 1;
                }
            }
        }

        count
    }
}
