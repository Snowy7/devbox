//! Shared-folder worktree boundary for Loom.
//!
//! Scanning, generated-file policy, materialization, restore safety, and
//! file-version capture belong here as the old snapshot crate is migrated.

use loom_core::{
    FileKind, FileVersion, FileVersionId, FolderRevision, FolderRevisionId, HydrationState,
    LoomError, ObjectId, RevisionBoundary, SharedFolder,
};
use loom_store::{path_to_store_string, LocalStore, StoreError, STORE_DIR};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
const OLD_SECRET_SCAN_PREFIX_BYTES: usize = 1024 * 1024;
const MAX_SECRET_FINDINGS: usize = 16;
pub const DEFAULT_PREFETCH_MAX_BYTES: u64 = 64 * 1024;
pub const DEFAULT_WARM_MAX_BYTES: u64 = DEFAULT_PREFETCH_MAX_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicyPreset {
    name: &'static str,
    intent: &'static str,
    warm_max_bytes: u64,
    prune_target: Option<u64>,
    pins_required: bool,
}

impl CachePolicyPreset {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn intent(&self) -> &'static str {
        self.intent
    }

    pub fn warm_max_bytes(&self) -> u64 {
        self.warm_max_bytes
    }

    pub fn prune_target(&self) -> Option<u64> {
        self.prune_target
    }

    pub fn pins_required(&self) -> bool {
        self.pins_required
    }
}

const CACHE_POLICY_PRESETS: &[CachePolicyPreset] = &[
    CachePolicyPreset {
        name: "online-first",
        intent: "keep metadata complete and hydrate useful small files on demand",
        warm_max_bytes: DEFAULT_WARM_MAX_BYTES,
        prune_target: None,
        pins_required: false,
    },
    CachePolicyPreset {
        name: "offline-pinned",
        intent: "keep pinned paths hydrated and protect them from free-space cleanup",
        warm_max_bytes: DEFAULT_WARM_MAX_BYTES,
        prune_target: None,
        pins_required: true,
    },
    CachePolicyPreset {
        name: "low-disk",
        intent:
            "prefer remote-only clean files and hydrate only manifests/config plus small source",
        warm_max_bytes: 16 * 1024,
        prune_target: Some(256 * 1024 * 1024),
        pins_required: true,
    },
    CachePolicyPreset {
        name: "agent-sandbox",
        intent: "warm source/config deterministically while avoiding generated output and secrets",
        warm_max_bytes: DEFAULT_WARM_MAX_BYTES,
        prune_target: None,
        pins_required: false,
    },
    CachePolicyPreset {
        name: "ci-ephemeral",
        intent: "hydrate exactly what the job asks for and leave cleanup explicit",
        warm_max_bytes: 32 * 1024,
        prune_target: Some(0),
        pins_required: false,
    },
];

pub fn cache_policy_presets() -> &'static [CachePolicyPreset] {
    CACHE_POLICY_PRESETS
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeDiff {
    created: Vec<PathBuf>,
    modified: Vec<PathBuf>,
    deleted: Vec<PathBuf>,
    unchanged: usize,
}

impl WorktreeDiff {
    fn new(
        created: Vec<PathBuf>,
        modified: Vec<PathBuf>,
        deleted: Vec<PathBuf>,
        unchanged: usize,
    ) -> Self {
        Self {
            created,
            modified,
            deleted,
            unchanged,
        }
    }

    pub fn created(&self) -> &[PathBuf] {
        &self.created
    }

    pub fn modified(&self) -> &[PathBuf] {
        &self.modified
    }

    pub fn deleted(&self) -> &[PathBuf] {
        &self.deleted
    }

    pub fn unchanged(&self) -> usize {
        self.unchanged
    }

    pub fn has_changes(&self) -> bool {
        !self.created.is_empty() || !self.modified.is_empty() || !self.deleted.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    revision_id: FolderRevisionId,
    diff: WorktreeDiff,
}

impl RestoreReport {
    pub fn revision_id(&self) -> &FolderRevisionId {
        &self.revision_id
    }

    pub fn diff(&self) -> &WorktreeDiff {
        &self.diff
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HydrateReport {
    materialized_files: usize,
    materialized_directories: usize,
    already_materialized_files: usize,
}

impl HydrateReport {
    pub fn materialized_files(&self) -> usize {
        self.materialized_files
    }

    pub fn materialized_directories(&self) -> usize {
        self.materialized_directories
    }

    pub fn already_materialized_files(&self) -> usize {
        self.already_materialized_files
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvictReport {
    evicted_files: usize,
    already_remote_files: usize,
    evicted_objects: usize,
}

impl EvictReport {
    pub fn evicted_files(&self) -> usize {
        self.evicted_files
    }

    pub fn already_remote_files(&self) -> usize {
        self.already_remote_files
    }

    pub fn evicted_objects(&self) -> usize {
        self.evicted_objects
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheStatusReport {
    hydrated_files: usize,
    remote_only_files: usize,
    partial_files: usize,
    total_files: usize,
    hydrated_bytes: u64,
    remote_only_bytes: u64,
    pinned_files: usize,
    pinned_bytes: u64,
    evictable_files: usize,
    evictable_bytes: u64,
}

impl CacheStatusReport {
    pub fn hydrated_files(&self) -> usize {
        self.hydrated_files
    }

    pub fn remote_only_files(&self) -> usize {
        self.remote_only_files
    }

    pub fn partial_files(&self) -> usize {
        self.partial_files
    }

    pub fn total_files(&self) -> usize {
        self.total_files
    }

    pub fn hydrated_bytes(&self) -> u64 {
        self.hydrated_bytes
    }

    pub fn remote_only_bytes(&self) -> u64 {
        self.remote_only_bytes
    }

    pub fn pinned_files(&self) -> usize {
        self.pinned_files
    }

    pub fn pinned_bytes(&self) -> u64 {
        self.pinned_bytes
    }

    pub fn evictable_files(&self) -> usize {
        self.evictable_files
    }

    pub fn evictable_bytes(&self) -> u64 {
        self.evictable_bytes
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CachePruneReport {
    limit_bytes: u64,
    hydrated_bytes_before: u64,
    hydrated_bytes_after: u64,
    evicted_files: usize,
    evicted_objects: usize,
    already_remote_files: usize,
    skipped_pinned_files: usize,
    skipped_dirty_files: usize,
    skipped_unsupported_files: usize,
}

impl CachePruneReport {
    pub fn limit_bytes(&self) -> u64 {
        self.limit_bytes
    }

    pub fn hydrated_bytes_before(&self) -> u64 {
        self.hydrated_bytes_before
    }

    pub fn hydrated_bytes_after(&self) -> u64 {
        self.hydrated_bytes_after
    }

    pub fn evicted_files(&self) -> usize {
        self.evicted_files
    }

    pub fn evicted_objects(&self) -> usize {
        self.evicted_objects
    }

    pub fn already_remote_files(&self) -> usize {
        self.already_remote_files
    }

    pub fn skipped_pinned_files(&self) -> usize {
        self.skipped_pinned_files
    }

    pub fn skipped_dirty_files(&self) -> usize {
        self.skipped_dirty_files
    }

    pub fn skipped_unsupported_files(&self) -> usize {
        self.skipped_unsupported_files
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WarmSelection {
    versions: Vec<FileVersion>,
    selected_files: usize,
    selected_manifest_files: usize,
    selected_source_files: usize,
    selected_small_files: usize,
    skipped_large_files: usize,
    skipped_non_manifest_files: usize,
}

impl WarmSelection {
    pub fn versions(&self) -> &[FileVersion] {
        &self.versions
    }

    pub fn selected_files(&self) -> usize {
        self.selected_files
    }

    pub fn selected_manifest_files(&self) -> usize {
        self.selected_manifest_files
    }

    pub fn selected_source_files(&self) -> usize {
        self.selected_source_files
    }

    pub fn selected_small_files(&self) -> usize {
        self.selected_small_files
    }

    pub fn skipped_large_files(&self) -> usize {
        self.skipped_large_files
    }

    pub fn skipped_non_manifest_files(&self) -> usize {
        self.skipped_non_manifest_files
    }
}

pub type PrefetchSelection = WarmSelection;

#[derive(Debug, Clone)]
pub struct RestoreEngine<'a> {
    store: &'a LocalStore,
}

#[derive(Debug, Clone)]
struct RestorePlan {
    removed: Vec<FileVersion>,
    materialized: Vec<PlannedMaterialization>,
}

#[derive(Debug, Clone)]
enum PlannedMaterialization {
    Directory {
        version: FileVersion,
    },
    File {
        version: FileVersion,
        bytes: Vec<u8>,
    },
}

impl<'a> RestoreEngine<'a> {
    pub fn new(store: &'a LocalStore) -> Self {
        Self { store }
    }

    pub fn restore(
        &self,
        revision: &FolderRevision,
        current: &WorktreeCapture,
    ) -> CaptureResult<RestoreReport> {
        if let Some(notice) = current.blocked().first() {
            return Err(CaptureError::UnsafeRestore {
                path: notice.relative_path().to_path_buf(),
                reason: "working folder contains a secret-blocked source file".to_string(),
            });
        }

        if let Some(notice) = current.deferred().first() {
            return Err(CaptureError::UnsafeRestore {
                path: notice.relative_path().to_path_buf(),
                reason: "working folder contains a deferred source entry".to_string(),
            });
        }

        let diff = diff_revision_to_capture(revision, current)?;
        let target_paths = revision
            .entries()
            .iter()
            .map(|entry| entry.path().to_path_buf())
            .collect::<BTreeSet<_>>();
        let file_versions = self
            .store
            .file_versions()
            .map_err(CaptureError::Store)?
            .into_iter()
            .map(|version| (version.id().clone(), version))
            .collect::<BTreeMap<_, _>>();

        validate_restore_entries(revision)?;
        let mut removed = current
            .file_versions()
            .iter()
            .filter(|version| !target_paths.contains(version.path()))
            .cloned()
            .collect::<Vec<_>>();
        removed.sort_by(|left, right| {
            path_to_store_string(right.path()).cmp(&path_to_store_string(left.path()))
        });
        let removed_paths = removed
            .iter()
            .map(|version| version.path().to_path_buf())
            .collect::<BTreeSet<_>>();

        let mut entries = revision.entries().to_vec();
        entries.sort_by(|left, right| {
            path_to_store_string(left.path()).cmp(&path_to_store_string(right.path()))
        });
        let mut target_versions = Vec::new();

        for entry in entries {
            let version = file_versions.get(entry.file_version_id()).ok_or_else(|| {
                CaptureError::MissingRevisionFileVersion {
                    revision_id: revision.id().clone(),
                    file_version_id: entry.file_version_id().clone(),
                }
            })?;
            if entry.path() != version.path() {
                return Err(CaptureError::UnsafeRestore {
                    path: entry.path().to_path_buf(),
                    reason: format!(
                        "revision entry points at file version for {}",
                        path_to_store_string(version.path())
                    ),
                });
            }
            target_versions.push(version.clone());
        }

        validate_restore_target_plan(&target_versions)?;

        let mut materialized = Vec::new();
        for version in &target_versions {
            materialized.push(preflight_materialization(
                self.store,
                current.root(),
                version,
                &removed_paths,
            )?);
        }

        for version in &removed {
            preflight_remove_current_entry(current.root(), version, &removed_paths)?;
        }

        let plan = RestorePlan {
            removed,
            materialized,
        };

        for version in &plan.removed {
            remove_current_entry(current.root(), version)?;
        }

        for materialization in &plan.materialized {
            materialize_planned_entry(current.root(), materialization)?;
        }

        Ok(RestoreReport {
            revision_id: revision.id().clone(),
            diff,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CaptureEngine<'a> {
    store: &'a LocalStore,
}

impl<'a> CaptureEngine<'a> {
    pub fn new(store: &'a LocalStore) -> Self {
        Self { store }
    }

    pub fn capture(&self, request: &CaptureRequest) -> CaptureResult<WorktreeCapture> {
        let root = request.shared_folder.root();
        if !root.exists() {
            return Err(CaptureError::RootNotFound {
                path: root.to_path_buf(),
            });
        }

        if !root.is_dir() {
            return Err(CaptureError::RootNotDirectory {
                path: root.to_path_buf(),
            });
        }

        let root = fs::canonicalize(root).map_err(|source| CaptureError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let captured_at = current_timestamp();
        let mut capture = WorktreeCapture::new(root.clone(), captured_at.clone());
        walk_directory(self.store, &root, &root, &captured_at, &mut capture)?;
        preserve_remote_only_latest_entries(self.store, &root, &mut capture)?;
        capture.file_versions.sort_by(|left, right| {
            path_to_store_string(left.path()).cmp(&path_to_store_string(right.path()))
        });

        Ok(capture)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCapture {
    root: PathBuf,
    captured_at: String,
    file_versions: Vec<FileVersion>,
    summary: CaptureSummary,
    ignored: Vec<CaptureNotice>,
    blocked: Vec<CaptureNotice>,
    deferred: Vec<CaptureNotice>,
}

impl WorktreeCapture {
    fn new(root: PathBuf, captured_at: String) -> Self {
        Self {
            root,
            captured_at,
            file_versions: Vec::new(),
            summary: CaptureSummary::default(),
            ignored: Vec::new(),
            blocked: Vec::new(),
            deferred: Vec::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn captured_at(&self) -> &str {
        &self.captured_at
    }

    pub fn file_versions(&self) -> &[FileVersion] {
        &self.file_versions
    }

    pub fn summary(&self) -> &CaptureSummary {
        &self.summary
    }

    pub fn ignored(&self) -> &[CaptureNotice] {
        &self.ignored
    }

    pub fn blocked(&self) -> &[CaptureNotice] {
        &self.blocked
    }

    pub fn deferred(&self) -> &[CaptureNotice] {
        &self.deferred
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureSummary {
    total_entries: usize,
    captured_files: usize,
    captured_directories: usize,
    ignored_entries: usize,
    blocked_secret_files: usize,
    deferred_entries: usize,
    total_file_bytes: u64,
}

impl CaptureSummary {
    pub fn total_entries(&self) -> usize {
        self.total_entries
    }

    pub fn captured_files(&self) -> usize {
        self.captured_files
    }

    pub fn captured_directories(&self) -> usize {
        self.captured_directories
    }

    pub fn ignored_entries(&self) -> usize {
        self.ignored_entries
    }

    pub fn blocked_secret_files(&self) -> usize {
        self.blocked_secret_files
    }

    pub fn deferred_entries(&self) -> usize {
        self.deferred_entries
    }

    pub fn total_file_bytes(&self) -> u64 {
        self.total_file_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureNotice {
    relative_path: PathBuf,
    reason: String,
}

impl CaptureNotice {
    fn new(relative_path: PathBuf, reason: impl Into<String>) -> Self {
        Self {
            relative_path,
            reason: reason.into(),
        }
    }

    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryPolicyDecision {
    Include,
    Ignore { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileCapturePolicyDecision {
    Capture,
    Ignore { path: PathBuf, reason: String },
    Block { path: PathBuf, reason: String },
}

#[derive(Debug)]
pub enum CaptureError {
    RootNotFound {
        path: PathBuf,
    },
    RootNotDirectory {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Store(StoreError),
    Loom(LoomError),
    UnsafeRestore {
        path: PathBuf,
        reason: String,
    },
    MissingRevisionFileVersion {
        revision_id: FolderRevisionId,
        file_version_id: FileVersionId,
    },
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootNotFound { path } => write!(f, "folder does not exist: {}", path.display()),
            Self::RootNotDirectory { path } => {
                write!(f, "path is not a folder: {}", path.display())
            }
            Self::Io { path, source } => {
                write!(f, "could not capture {}: {source}", path.display())
            }
            Self::Store(error) => write!(f, "{error}"),
            Self::Loom(error) => write!(f, "{error}"),
            Self::UnsafeRestore { path, reason } => {
                write!(f, "restore refused for {}: {reason}", path.display())
            }
            Self::MissingRevisionFileVersion {
                revision_id,
                file_version_id,
            } => write!(
                f,
                "revision {revision_id} references missing file version {file_version_id}"
            ),
        }
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Store(error) => Some(error),
            Self::Loom(error) => Some(error),
            Self::RootNotFound { .. }
            | Self::RootNotDirectory { .. }
            | Self::UnsafeRestore { .. }
            | Self::MissingRevisionFileVersion { .. } => None,
        }
    }
}

impl From<StoreError> for CaptureError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<LoomError> for CaptureError {
    fn from(error: LoomError) -> Self {
        Self::Loom(error)
    }
}

pub type CaptureResult<T> = Result<T, CaptureError>;

pub fn relative_scope_path(store: &LocalStore, path: &Path) -> CaptureResult<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| CaptureError::Io {
                path: PathBuf::from("."),
                source,
            })?
            .join(path)
    };
    let path = canonicalize_existing_prefix(&path)?;

    let relative = path
        .strip_prefix(store.folder_root())
        .map_err(|_| CaptureError::UnsafeRestore {
            path: path.clone(),
            reason: format!(
                "path is not inside shared folder {}",
                store.folder_root().display()
            ),
        })?
        .to_path_buf();

    validate_materialization_scope(&relative)?;
    Ok(relative)
}

pub fn tracked_versions_for_scope(
    store: &LocalStore,
    scope: &Path,
) -> CaptureResult<Vec<FileVersion>> {
    validate_materialization_scope(scope)?;
    let revision = store
        .latest_revision()
        .map_err(CaptureError::Store)?
        .ok_or_else(|| CaptureError::UnsafeRestore {
            path: scope.to_path_buf(),
            reason: "no folder revisions have been imported yet".to_string(),
        })?;
    let file_versions = store
        .file_versions()
        .map_err(CaptureError::Store)?
        .into_iter()
        .map(|version| (version.id().clone(), version))
        .collect::<BTreeMap<_, _>>();
    let mut selected = Vec::new();

    for entry in revision.entries() {
        if !path_is_in_scope(entry.path(), scope) {
            continue;
        }
        let version = file_versions.get(entry.file_version_id()).ok_or_else(|| {
            CaptureError::MissingRevisionFileVersion {
                revision_id: revision.id().clone(),
                file_version_id: entry.file_version_id().clone(),
            }
        })?;
        if version.path() != entry.path() {
            return Err(CaptureError::UnsafeRestore {
                path: entry.path().to_path_buf(),
                reason: format!(
                    "revision entry points at file version for {}",
                    path_to_store_string(version.path())
                ),
            });
        }
        validate_materialized_relative_path(version.path())?;
        selected.push(version.clone());
    }

    selected.sort_by(|left, right| {
        path_to_store_string(left.path()).cmp(&path_to_store_string(right.path()))
    });
    Ok(selected)
}

pub fn hydrate_versions(
    store: &LocalStore,
    versions: &[FileVersion],
) -> CaptureResult<HydrateReport> {
    validate_restore_target_plan(versions)?;
    let mut report = HydrateReport::default();
    let mut file_writes = Vec::new();
    let mut directories = Vec::new();

    for version in versions {
        let target = store.folder_root().join(version.path());
        match version.kind() {
            FileKind::Directory => {
                preflight_hydrate_directory(version.path(), &target)?;
                directories.push(version.path().to_path_buf());
            }
            FileKind::File => {
                let object_id = version
                    .object_id()
                    .ok_or_else(|| CaptureError::UnsafeRestore {
                        path: version.path().to_path_buf(),
                        reason: "file version has no content object".to_string(),
                    })?;
                let bytes = store
                    .object_cache()
                    .read(object_id)
                    .map_err(CaptureError::Store)?;
                match preflight_hydrate_file(version.path(), &target, &bytes)? {
                    HydrateFileAction::Write => {
                        file_writes.push((version.path().to_path_buf(), bytes))
                    }
                    HydrateFileAction::AlreadyMaterialized => {
                        report.already_materialized_files += 1;
                    }
                }
            }
            FileKind::Symlink | FileKind::Unsupported => {
                return Err(CaptureError::UnsafeRestore {
                    path: version.path().to_path_buf(),
                    reason: "only regular files and directories can be hydrated safely".to_string(),
                });
            }
        }
    }

    for path in directories {
        let target = store.folder_root().join(&path);
        fs::create_dir_all(&target).map_err(|source| CaptureError::Io {
            path: target,
            source,
        })?;
        report.materialized_directories += 1;
    }

    for (path, bytes) in file_writes {
        let target = store.folder_root().join(&path);
        prepare_hydrate_file_target(&path, &target)?;
        fs::write(&target, bytes).map_err(|source| CaptureError::Io {
            path: target,
            source,
        })?;
        report.materialized_files += 1;
    }

    Ok(report)
}

pub fn evict_versions(
    store: &LocalStore,
    selected_versions: &[FileVersion],
    pinned_scopes: &[PathBuf],
    remote_available_objects: &BTreeSet<ObjectId>,
) -> CaptureResult<EvictReport> {
    let mut report = EvictReport::default();
    let latest_versions = tracked_versions_for_scope(store, Path::new(""))?;
    let selected_paths = selected_versions
        .iter()
        .map(|version| version.path().to_path_buf())
        .collect::<BTreeSet<_>>();
    let mut selected_objects = BTreeSet::<ObjectId>::new();
    let mut removable_files = Vec::new();

    for version in selected_versions {
        if version.kind() != &FileKind::File {
            continue;
        }
        if pinned_scopes
            .iter()
            .any(|pinned_scope| paths_overlap(version.path(), pinned_scope))
        {
            return Err(CaptureError::UnsafeRestore {
                path: version.path().to_path_buf(),
                reason: "path is pinned for offline retention".to_string(),
            });
        }

        let object_id = version
            .object_id()
            .ok_or_else(|| CaptureError::UnsafeRestore {
                path: version.path().to_path_buf(),
                reason: "file version has no content object".to_string(),
            })?;
        if !remote_available_objects.contains(object_id) {
            return Err(CaptureError::UnsafeRestore {
                path: version.path().to_path_buf(),
                reason: "refusing to evict without a proven remote object copy".to_string(),
            });
        }
        let target = store.folder_root().join(version.path());
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CaptureError::UnsafeRestore {
                    path: version.path().to_path_buf(),
                    reason: "refusing to evict a symlink".to_string(),
                });
            }
            Ok(metadata) if metadata.is_file() => {
                ensure_file_matches_object(version.path(), &target, object_id)?;
                removable_files.push(version.path().to_path_buf());
                selected_objects.insert(object_id.clone());
            }
            Ok(_) => {
                return Err(CaptureError::UnsafeRestore {
                    path: version.path().to_path_buf(),
                    reason: "refusing to evict an unsupported local entry".to_string(),
                });
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                report.already_remote_files += 1;
                selected_objects.insert(object_id.clone());
            }
            Err(source) => {
                return Err(CaptureError::Io {
                    path: target,
                    source,
                });
            }
        }
    }

    for path in removable_files {
        let target = store.folder_root().join(&path);
        fs::remove_file(&target).map_err(|source| CaptureError::Io {
            path: target,
            source,
        })?;
        report.evicted_files += 1;
    }

    for object_id in selected_objects {
        let still_materialized = latest_versions.iter().any(|version| {
            version.object_id() == Some(&object_id)
                && !selected_paths.contains(version.path())
                && store.folder_root().join(version.path()).is_file()
        });
        if still_materialized {
            continue;
        }
        let size_bytes = latest_versions
            .iter()
            .find(|version| version.object_id() == Some(&object_id))
            .and_then(FileVersion::size_bytes);
        store
            .evict_cached_object(&object_id, size_bytes)
            .map_err(CaptureError::Store)?;
        report.evicted_objects += 1;
    }

    Ok(report)
}

pub fn cache_status_for_scope(
    store: &LocalStore,
    scope: &Path,
    remote_available_objects: &BTreeSet<ObjectId>,
) -> CaptureResult<CacheStatusReport> {
    let versions = tracked_versions_for_scope(store, scope)?;
    let pinned_scopes = local_materialization_pin_scopes(store)?;
    let mut report = CacheStatusReport::default();
    for version in versions {
        if version.kind() != &FileKind::File {
            continue;
        }
        let Some(object_id) = version.object_id() else {
            continue;
        };
        report.total_files += 1;
        let size_bytes = version.size_bytes().unwrap_or(0);
        let state = materialization_state_for_version(store, &version, object_id)?;
        match state {
            HydrationState::Hydrated => {
                report.hydrated_files += 1;
                report.hydrated_bytes += size_bytes;
            }
            HydrationState::RemoteOnly => {
                report.remote_only_files += 1;
                report.remote_only_bytes += size_bytes;
            }
            HydrationState::Partial => report.partial_files += 1,
        }

        let pinned = pinned_scopes
            .iter()
            .any(|pinned_scope| paths_overlap(version.path(), pinned_scope));
        if pinned {
            report.pinned_files += 1;
            report.pinned_bytes += size_bytes;
            continue;
        }

        if state == HydrationState::Hydrated
            && remote_available_objects.contains(object_id)
            && matches!(
                classify_evictability(store, &version, object_id)?,
                Evictability::Clean
            )
        {
            report.evictable_files += 1;
            report.evictable_bytes += size_bytes;
        }
    }

    Ok(report)
}

pub fn prune_cache_to_limit(
    store: &LocalStore,
    scope: &Path,
    max_bytes: u64,
    remote_available_objects: &BTreeSet<ObjectId>,
) -> CaptureResult<CachePruneReport> {
    let pinned_scopes = local_materialization_pin_scopes(store)?;
    let before = cache_status_for_scope(store, scope, remote_available_objects)?;
    let mut report = CachePruneReport {
        limit_bytes: max_bytes,
        hydrated_bytes_before: before.hydrated_bytes(),
        hydrated_bytes_after: before.hydrated_bytes(),
        ..CachePruneReport::default()
    };
    if before.hydrated_bytes() <= max_bytes {
        return Ok(report);
    }

    let mut versions = tracked_versions_for_scope(store, scope)?
        .into_iter()
        .filter(|version| version.kind() == &FileKind::File)
        .filter(|version| {
            version.object_id().is_some_and(|object_id| {
                remote_available_objects.contains(object_id)
                    && store.object_cache().exists(object_id)
            })
        })
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| {
        path_to_store_string(left.path()).cmp(&path_to_store_string(right.path()))
    });

    for version in versions {
        if report.hydrated_bytes_after <= max_bytes {
            break;
        }

        if pinned_scopes
            .iter()
            .any(|pinned_scope| paths_overlap(version.path(), pinned_scope))
        {
            report.skipped_pinned_files += 1;
            continue;
        }

        let object_id = version
            .object_id()
            .ok_or_else(|| CaptureError::UnsafeRestore {
                path: version.path().to_path_buf(),
                reason: "file version has no content object".to_string(),
            })?;
        match classify_evictability(store, &version, object_id)? {
            Evictability::Clean => {
                let evicted =
                    evict_versions(store, &[version], &pinned_scopes, remote_available_objects)?;
                report.evicted_files += evicted.evicted_files();
                report.evicted_objects += evicted.evicted_objects();
                report.already_remote_files += evicted.already_remote_files();
                report.hydrated_bytes_after =
                    cache_status_for_scope(store, scope, remote_available_objects)?
                        .hydrated_bytes();
            }
            Evictability::Dirty => report.skipped_dirty_files += 1,
            Evictability::Unsupported => report.skipped_unsupported_files += 1,
        }
    }

    Ok(report)
}

pub fn prefetch_versions_for_scope(
    store: &LocalStore,
    scope: &Path,
    max_bytes: u64,
) -> CaptureResult<PrefetchSelection> {
    warm_versions_for_scope(store, scope, max_bytes, false)
}

pub fn warm_versions_for_scope(
    store: &LocalStore,
    scope: &Path,
    max_bytes: u64,
    manifest_only: bool,
) -> CaptureResult<WarmSelection> {
    let mut selection = WarmSelection::default();
    let mut versions = tracked_versions_for_scope(store, scope)?
        .into_iter()
        .filter(|version| version.kind() == &FileKind::File)
        .collect::<Vec<_>>();
    versions.sort_by(|left, right| {
        path_to_store_string(left.path()).cmp(&path_to_store_string(right.path()))
    });

    for version in versions {
        let size_bytes = version.size_bytes().unwrap_or(u64::MAX);
        if size_bytes > max_bytes {
            selection.skipped_large_files += 1;
            continue;
        }
        if version.object_id().is_none() {
            continue;
        }

        let category = warm_category(version.path());
        if manifest_only && category != WarmCategory::Manifest {
            selection.skipped_non_manifest_files += 1;
            continue;
        }

        selection.selected_files += 1;
        match category {
            WarmCategory::Manifest => selection.selected_manifest_files += 1,
            WarmCategory::Source => selection.selected_source_files += 1,
            WarmCategory::Small => selection.selected_small_files += 1,
        }
        selection.versions.push(version);
    }

    Ok(selection)
}

pub fn diff_revision_to_capture(
    revision: &FolderRevision,
    capture: &WorktreeCapture,
) -> CaptureResult<WorktreeDiff> {
    validate_restore_entries(revision)?;

    let base_entries = revision
        .entries()
        .iter()
        .map(|entry| (entry.path().to_path_buf(), entry.file_version_id().clone()))
        .collect::<BTreeMap<_, _>>();
    let current_entries = capture
        .file_versions()
        .iter()
        .map(|version| (version.path().to_path_buf(), version.id().clone()))
        .collect::<BTreeMap<_, _>>();
    let mut created = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    let mut unchanged = 0;

    for (path, current_id) in &current_entries {
        match base_entries.get(path) {
            Some(base_id) if base_id == current_id => unchanged += 1,
            Some(_) => modified.push(path.clone()),
            None => created.push(path.clone()),
        }
    }

    for path in base_entries.keys() {
        if !current_entries.contains_key(path) {
            deleted.push(path.clone());
        }
    }

    Ok(WorktreeDiff::new(created, modified, deleted, unchanged))
}

fn validate_restore_entries(revision: &FolderRevision) -> CaptureResult<()> {
    for entry in revision.entries() {
        validate_materialized_relative_path(entry.path())?;
    }

    Ok(())
}

fn validate_restore_target_plan(file_versions: &[FileVersion]) -> CaptureResult<()> {
    let mut planned_paths = BTreeMap::new();

    for version in file_versions {
        validate_materialized_relative_path(version.path())?;
        if matches!(version.kind(), FileKind::Symlink | FileKind::Unsupported) {
            return Err(CaptureError::UnsafeRestore {
                path: version.path().to_path_buf(),
                reason: "only regular files and directories can be restored safely".to_string(),
            });
        }

        if planned_paths
            .insert(version.path().to_path_buf(), version.kind().clone())
            .is_some()
        {
            return Err(CaptureError::UnsafeRestore {
                path: version.path().to_path_buf(),
                reason: "target revision contains duplicate entries for one path".to_string(),
            });
        }
    }

    for (path, kind) in &planned_paths {
        if kind != &FileKind::File {
            continue;
        }

        if let Some(descendant) = planned_paths
            .keys()
            .find(|candidate| *candidate != path && candidate.starts_with(path))
        {
            return Err(CaptureError::UnsafeRestore {
                path: descendant.clone(),
                reason: format!(
                    "target revision would materialize {} as both a file and an ancestor",
                    path_to_store_string(path)
                ),
            });
        }
    }

    Ok(())
}

fn preflight_materialization(
    store: &LocalStore,
    root: &Path,
    version: &FileVersion,
    removed_paths: &BTreeSet<PathBuf>,
) -> CaptureResult<PlannedMaterialization> {
    let target =
        validate_materialized_relative_path(version.path()).map(|_| root.join(version.path()))?;

    match version.kind() {
        FileKind::Directory => {
            if let Ok(metadata) = fs::symlink_metadata(&target) {
                if metadata.file_type().is_symlink() {
                    return Err(CaptureError::UnsafeRestore {
                        path: version.path().to_path_buf(),
                        reason: "refusing to replace a symlink".to_string(),
                    });
                }
            }

            Ok(PlannedMaterialization::Directory {
                version: version.clone(),
            })
        }
        FileKind::File => {
            let object_id = version
                .object_id()
                .ok_or_else(|| CaptureError::UnsafeRestore {
                    path: version.path().to_path_buf(),
                    reason: "file version has no content object".to_string(),
                })?;
            let bytes = store
                .object_cache()
                .read(object_id)
                .map_err(CaptureError::Store)?;

            preflight_file_target(version.path(), &target, removed_paths)?;

            Ok(PlannedMaterialization::File {
                version: version.clone(),
                bytes,
            })
        }
        FileKind::Symlink | FileKind::Unsupported => Err(CaptureError::UnsafeRestore {
            path: version.path().to_path_buf(),
            reason: "only regular files and directories can be restored safely".to_string(),
        }),
    }
}

fn materialize_planned_entry(
    root: &Path,
    materialization: &PlannedMaterialization,
) -> CaptureResult<()> {
    match materialization {
        PlannedMaterialization::Directory { version } => {
            let target = validate_materialized_relative_path(version.path())
                .map(|_| root.join(version.path()))?;
            if let Ok(metadata) = fs::symlink_metadata(&target) {
                if metadata.file_type().is_symlink() {
                    return Err(CaptureError::UnsafeRestore {
                        path: version.path().to_path_buf(),
                        reason: "refusing to replace a symlink".to_string(),
                    });
                }
                if metadata.is_file() {
                    fs::remove_file(&target).map_err(|source| CaptureError::Io {
                        path: target.clone(),
                        source,
                    })?;
                }
            }

            fs::create_dir_all(&target).map_err(|source| CaptureError::Io {
                path: target,
                source,
            })
        }
        PlannedMaterialization::File { version, bytes } => {
            let target = validate_materialized_relative_path(version.path())
                .map(|_| root.join(version.path()))?;
            prepare_file_target(version.path(), &target)?;
            fs::write(&target, bytes).map_err(|source| CaptureError::Io {
                path: target,
                source,
            })
        }
    }
}

fn preflight_file_target(
    relative_path: &Path,
    target: &Path,
    removed_paths: &BTreeSet<PathBuf>,
) -> CaptureResult<()> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "refusing to overwrite a symlink".to_string(),
        }),
        Ok(metadata) if metadata.is_dir() => {
            ensure_directory_clear_after_planned_removals(relative_path, target, removed_paths)
        }
        Ok(_) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CaptureError::Io {
            path: target.to_path_buf(),
            source,
        }),
    }
}

fn prepare_file_target(relative_path: &Path, target: &Path) -> CaptureResult<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| CaptureError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "refusing to overwrite a symlink".to_string(),
        }),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir(target)
            .map_err(|source| restore_remove_dir_error(relative_path, target, source)),
        Ok(_) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CaptureError::Io {
            path: target.to_path_buf(),
            source,
        }),
    }
}

fn preflight_remove_current_entry(
    root: &Path,
    version: &FileVersion,
    removed_paths: &BTreeSet<PathBuf>,
) -> CaptureResult<()> {
    let target =
        validate_materialized_relative_path(version.path()).map(|_| root.join(version.path()))?;

    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CaptureError::UnsafeRestore {
            path: version.path().to_path_buf(),
            reason: "refusing to remove a symlink".to_string(),
        }),
        Ok(metadata) if metadata.is_dir() => {
            ensure_directory_clear_after_planned_removals(version.path(), &target, removed_paths)
        }
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(CaptureError::UnsafeRestore {
            path: version.path().to_path_buf(),
            reason: "unsupported filesystem entry".to_string(),
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CaptureError::Io {
            path: target,
            source,
        }),
    }
}

fn remove_current_entry(root: &Path, version: &FileVersion) -> CaptureResult<()> {
    let target =
        validate_materialized_relative_path(version.path()).map(|_| root.join(version.path()))?;

    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CaptureError::UnsafeRestore {
            path: version.path().to_path_buf(),
            reason: "refusing to remove a symlink".to_string(),
        }),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir(&target)
            .map_err(|source| restore_remove_dir_error(version.path(), &target, source)),
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(&target).map_err(|source| CaptureError::Io {
                path: target,
                source,
            })
        }
        Ok(_) => Err(CaptureError::UnsafeRestore {
            path: version.path().to_path_buf(),
            reason: "unsupported filesystem entry".to_string(),
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CaptureError::Io {
            path: target,
            source,
        }),
    }
}

fn ensure_directory_clear_after_planned_removals(
    relative_path: &Path,
    target: &Path,
    removed_paths: &BTreeSet<PathBuf>,
) -> CaptureResult<()> {
    for entry in fs::read_dir(target).map_err(|source| CaptureError::Io {
        path: target.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| CaptureError::Io {
            path: target.to_path_buf(),
            source,
        })?;
        let child_relative_path = relative_path.join(entry.file_name());
        if !removed_paths.contains(&child_relative_path) {
            return Err(CaptureError::UnsafeRestore {
                path: child_relative_path,
                reason:
                    "refusing to remove or replace a directory containing unplanned local entries"
                        .to_string(),
            });
        }
    }

    Ok(())
}

enum HydrateFileAction {
    Write,
    AlreadyMaterialized,
}

fn preflight_hydrate_directory(relative_path: &Path, target: &Path) -> CaptureResult<()> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "refusing to replace a symlink".to_string(),
        }),
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(metadata) if metadata.is_file() => Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "refusing to replace a local file with a directory".to_string(),
        }),
        Ok(_) => Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "unsupported filesystem entry".to_string(),
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CaptureError::Io {
            path: target.to_path_buf(),
            source,
        }),
    }
}

fn preflight_hydrate_file(
    relative_path: &Path,
    target: &Path,
    bytes: &[u8],
) -> CaptureResult<HydrateFileAction> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "refusing to overwrite a symlink".to_string(),
        }),
        Ok(metadata) if metadata.is_dir() => Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "refusing to overwrite a directory with a file".to_string(),
        }),
        Ok(metadata) if metadata.is_file() => {
            let current = fs::read(target).map_err(|source| CaptureError::Io {
                path: target.to_path_buf(),
                source,
            })?;
            if current == bytes {
                Ok(HydrateFileAction::AlreadyMaterialized)
            } else {
                Err(CaptureError::UnsafeRestore {
                    path: relative_path.to_path_buf(),
                    reason: "refusing to overwrite a dirty local file".to_string(),
                })
            }
        }
        Ok(_) => Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "unsupported filesystem entry".to_string(),
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(HydrateFileAction::Write),
        Err(source) => Err(CaptureError::Io {
            path: target.to_path_buf(),
            source,
        }),
    }
}

fn prepare_hydrate_file_target(relative_path: &Path, target: &Path) -> CaptureResult<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| CaptureError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    preflight_hydrate_file(relative_path, target, &[]).and_then(|action| match action {
        HydrateFileAction::Write => Ok(()),
        HydrateFileAction::AlreadyMaterialized => Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "internal hydrate preflight mismatch".to_string(),
        }),
    })
}

fn ensure_file_matches_object(
    relative_path: &Path,
    target: &Path,
    object_id: &ObjectId,
) -> CaptureResult<()> {
    let bytes = fs::read(target).map_err(|source| CaptureError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    let actual = ObjectId::from_blake3_hex(blake3::hash(&bytes).to_hex().to_string())?;
    if &actual != object_id {
        return Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "refusing to evict a dirty local file".to_string(),
        });
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evictability {
    Clean,
    Dirty,
    Unsupported,
}

fn classify_evictability(
    store: &LocalStore,
    version: &FileVersion,
    object_id: &ObjectId,
) -> CaptureResult<Evictability> {
    let target = store.folder_root().join(version.path());
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(Evictability::Unsupported),
        Ok(metadata) if metadata.is_file() => {
            match ensure_file_matches_object(version.path(), &target, object_id) {
                Ok(()) => Ok(Evictability::Clean),
                Err(CaptureError::UnsafeRestore { .. }) => Ok(Evictability::Dirty),
                Err(error) => Err(error),
            }
        }
        Ok(_) => Ok(Evictability::Unsupported),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(Evictability::Clean),
        Err(source) => Err(CaptureError::Io {
            path: target,
            source,
        }),
    }
}

fn materialization_state_for_version(
    store: &LocalStore,
    version: &FileVersion,
    object_id: &ObjectId,
) -> CaptureResult<HydrationState> {
    let target = store.folder_root().join(version.path());
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(HydrationState::Partial),
        Ok(metadata) if metadata.is_file() => {
            match ensure_file_matches_object(version.path(), &target, object_id) {
                Ok(()) => Ok(HydrationState::Hydrated),
                Err(CaptureError::UnsafeRestore { .. }) => Ok(HydrationState::Partial),
                Err(error) => Err(error),
            }
        }
        Ok(_) => Ok(HydrationState::Partial),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            if store.object_cache().exists(object_id) {
                Ok(HydrationState::Partial)
            } else {
                Ok(store
                    .cache_entry(object_id)
                    .map_err(CaptureError::Store)?
                    .map(|entry| entry.hydration_state())
                    .unwrap_or(HydrationState::RemoteOnly))
            }
        }
        Err(source) => Err(CaptureError::Io {
            path: target,
            source,
        }),
    }
}

fn local_materialization_pin_scopes(store: &LocalStore) -> CaptureResult<Vec<PathBuf>> {
    let mut scopes = Vec::new();
    for pin in store.pins().map_err(CaptureError::Store)? {
        let Some(path) = pin.reason().strip_prefix("materialization-pin path=") else {
            continue;
        };
        scopes.push(if path == "." {
            PathBuf::new()
        } else {
            path.split('/').collect()
        });
    }
    Ok(scopes)
}

fn validate_materialization_scope(relative_path: &Path) -> CaptureResult<()> {
    if relative_path.as_os_str().is_empty() {
        return Ok(());
    }
    validate_materialized_relative_path(relative_path)
}

fn canonicalize_existing_prefix(path: &Path) -> CaptureResult<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path).map_err(|source| CaptureError::Io {
            path: path.to_path_buf(),
            source,
        });
    }

    let mut missing = Vec::new();
    let mut existing = path.to_path_buf();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
            break;
        };
        missing.push(name);
        if !existing.pop() {
            break;
        }
    }

    let mut canonical = fs::canonicalize(&existing).map_err(|source| CaptureError::Io {
        path: existing,
        source,
    })?;
    for component in missing.into_iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

fn validate_materialized_relative_path(relative_path: &Path) -> CaptureResult<()> {
    if relative_path.as_os_str().is_empty() {
        return Err(CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "empty materialization path".to_string(),
        });
    }

    let mut policy_path = PathBuf::new();
    for component in relative_path.components() {
        match component {
            Component::Normal(part) => {
                policy_path.push(part);
                if let DirectoryPolicyDecision::Ignore { reason } =
                    evaluate_directory_policy(&policy_path)
                {
                    return Err(CaptureError::UnsafeRestore {
                        path: relative_path.to_path_buf(),
                        reason,
                    });
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => {
                return Err(CaptureError::UnsafeRestore {
                    path: relative_path.to_path_buf(),
                    reason: "path must stay inside the shared folder".to_string(),
                });
            }
        }
    }

    Ok(())
}

fn path_is_in_scope(path: &Path, scope: &Path) -> bool {
    scope.as_os_str().is_empty() || path == scope || path.starts_with(scope)
}

fn ancestors(path: &Path) -> Vec<PathBuf> {
    let mut ancestors = Vec::new();
    let mut current = PathBuf::new();
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        current.push(component);
        ancestors.push(current.clone());
    }
    ancestors
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.as_os_str().is_empty()
        || right.as_os_str().is_empty()
        || left == right
        || left.starts_with(right)
        || right.starts_with(left)
}

fn preserve_remote_only_latest_entries(
    store: &LocalStore,
    root: &Path,
    capture: &mut WorktreeCapture,
) -> CaptureResult<()> {
    let Some(revision) = store.latest_revision().map_err(CaptureError::Store)? else {
        return Ok(());
    };

    let file_versions = store
        .file_versions()
        .map_err(CaptureError::Store)?
        .into_iter()
        .map(|version| (version.id().clone(), version))
        .collect::<BTreeMap<_, _>>();
    let captured_paths = capture
        .file_versions
        .iter()
        .map(|version| version.path().to_path_buf())
        .collect::<BTreeSet<_>>();
    let mut latest_versions = Vec::new();

    for entry in revision.entries() {
        let version = file_versions.get(entry.file_version_id()).ok_or_else(|| {
            CaptureError::MissingRevisionFileVersion {
                revision_id: revision.id().clone(),
                file_version_id: entry.file_version_id().clone(),
            }
        })?;
        if version.path() != entry.path() {
            return Err(CaptureError::UnsafeRestore {
                path: entry.path().to_path_buf(),
                reason: format!(
                    "revision entry points at file version for {}",
                    path_to_store_string(version.path())
                ),
            });
        }
        latest_versions.push(version.clone());
    }

    let remote_only_files = latest_versions
        .iter()
        .filter(|version| version.kind() == &FileKind::File)
        .filter(|version| !captured_paths.contains(version.path()))
        .filter(|version| !root.join(version.path()).exists())
        .filter_map(|version| {
            let object_id = version.object_id()?;
            match store.cache_entry(object_id) {
                Ok(Some(entry)) if entry.hydration_state() == HydrationState::RemoteOnly => {
                    Some(version.path().to_path_buf())
                }
                _ => None,
            }
        })
        .collect::<BTreeSet<_>>();

    for version in latest_versions {
        if captured_paths.contains(version.path()) || root.join(version.path()).exists() {
            continue;
        }

        let preserve = match version.kind() {
            FileKind::File => remote_only_files.contains(version.path()),
            FileKind::Directory => remote_only_files
                .iter()
                .any(|file_path| file_path.starts_with(version.path())),
            FileKind::Symlink | FileKind::Unsupported => false,
        };

        if preserve {
            capture.file_versions.push(version);
        }
    }

    Ok(())
}

fn restore_remove_dir_error(
    relative_path: &Path,
    target: &Path,
    source: io::Error,
) -> CaptureError {
    if matches!(
        source.kind(),
        io::ErrorKind::DirectoryNotEmpty | io::ErrorKind::PermissionDenied
    ) {
        return CaptureError::UnsafeRestore {
            path: relative_path.to_path_buf(),
            reason: "refusing to remove a non-empty or protected directory".to_string(),
        };
    }

    CaptureError::Io {
        path: target.to_path_buf(),
        source,
    }
}

fn walk_directory(
    store: &LocalStore,
    root: &Path,
    path: &Path,
    captured_at: &str,
    capture: &mut WorktreeCapture,
) -> CaptureResult<()> {
    let mut entries = fs::read_dir(path)
        .map_err(|source| CaptureError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| CaptureError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path).map_err(|source| CaptureError::Io {
            path: entry_path.clone(),
            source,
        })?;
        let relative_path = relative_to(root, &entry_path);
        capture.summary.total_entries += 1;

        if metadata.is_dir() {
            match evaluate_directory_policy(&relative_path) {
                DirectoryPolicyDecision::Ignore { reason } => {
                    capture.summary.ignored_entries += 1;
                    capture
                        .ignored
                        .push(CaptureNotice::new(relative_path, reason));
                }
                DirectoryPolicyDecision::Include => {
                    capture_directory(&relative_path, captured_at, capture)?;
                    walk_directory(store, root, &entry_path, captured_at, capture)?;
                }
            }
        } else if metadata.is_file() {
            capture_file(store, &entry_path, &relative_path, captured_at, capture)?;
        } else if metadata.file_type().is_symlink() {
            capture.summary.deferred_entries += 1;
            capture.deferred.push(CaptureNotice::new(
                relative_path,
                "symlink capture is deferred until restore safety rules exist",
            ));
        } else {
            capture.summary.deferred_entries += 1;
            capture.deferred.push(CaptureNotice::new(
                relative_path,
                "unsupported file type is deferred",
            ));
        }
    }

    Ok(())
}

fn capture_directory(
    relative_path: &Path,
    captured_at: &str,
    capture: &mut WorktreeCapture,
) -> CaptureResult<()> {
    let version = FileVersion::new(
        stable_file_version_id(relative_path, FileKind::Directory, None, None)?,
        relative_path,
        FileKind::Directory,
        None,
        None,
        captured_at,
    )?;
    capture.summary.captured_directories += 1;
    capture.file_versions.push(version);
    Ok(())
}

fn capture_file(
    store: &LocalStore,
    path: &Path,
    relative_path: &Path,
    captured_at: &str,
    capture: &mut WorktreeCapture,
) -> CaptureResult<()> {
    let bytes = read_file_bytes_for_secret_check(path)?;
    match evaluate_file_capture_policy(relative_path, &bytes) {
        FileCapturePolicyDecision::Capture => {}
        FileCapturePolicyDecision::Ignore { path, reason } => {
            capture.summary.ignored_entries += 1;
            capture.ignored.push(CaptureNotice::new(path, reason));
            return Ok(());
        }
        FileCapturePolicyDecision::Block { path, reason } => {
            capture.summary.blocked_secret_files += 1;
            capture.blocked.push(CaptureNotice::new(path, reason));
            return Ok(());
        }
    }

    let object = store.write_object_bytes(&bytes)?;
    let size_bytes = object.size_bytes();
    let version = FileVersion::new(
        stable_file_version_id(
            relative_path,
            FileKind::File,
            Some(object.id().as_str()),
            Some(size_bytes),
        )?,
        relative_path,
        FileKind::File,
        Some(object.id().clone()),
        Some(size_bytes),
        captured_at,
    )?;
    capture.summary.captured_files += 1;
    capture.summary.total_file_bytes += size_bytes;
    capture.file_versions.push(version);
    Ok(())
}

pub fn evaluate_directory_policy(relative_path: &Path) -> DirectoryPolicyDecision {
    let Some(name) = relative_path.file_name().and_then(|name| name.to_str()) else {
        return DirectoryPolicyDecision::Include;
    };

    let reason = match name {
        ".git" => Some("Git metadata is developer folder context"),
        STORE_DIR => Some("Loom local store metadata"),
        "node_modules" => Some("generated Node dependency directory"),
        ".next" => Some("generated Next.js build directory"),
        "dist" => Some("generated distribution output"),
        "build" => Some("generated build output"),
        "target" => Some("generated Rust build output"),
        ".venv" | "venv" => Some("generated Python virtual environment"),
        "__pycache__" | ".pytest_cache" => Some("generated Python cache directory"),
        ".turbo" => Some("generated Turborepo cache directory"),
        ".gradle" => Some("generated Gradle cache directory"),
        ".cache" => Some("generated tool cache directory"),
        "coverage" => Some("generated coverage output"),
        _ => None,
    };

    match reason {
        Some(reason) => DirectoryPolicyDecision::Ignore {
            reason: reason.to_string(),
        },
        None => DirectoryPolicyDecision::Include,
    }
}

pub fn evaluate_file_capture_policy(
    relative_path: &Path,
    bytes: &[u8],
) -> FileCapturePolicyDecision {
    for ancestor in ancestors(relative_path) {
        match evaluate_directory_policy(&ancestor) {
            DirectoryPolicyDecision::Include => {}
            DirectoryPolicyDecision::Ignore { reason } => {
                return FileCapturePolicyDecision::Ignore {
                    path: ancestor,
                    reason,
                };
            }
        }
    }

    let findings = SecretDetector.scan_bytes(bytes);
    if let Some(finding) = findings.first() {
        return FileCapturePolicyDecision::Block {
            path: relative_path.to_path_buf(),
            reason: finding.policy_reason(),
        };
    }

    FileCapturePolicyDecision::Capture
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WarmCategory {
    Manifest,
    Source,
    Small,
}

fn warm_category(path: &Path) -> WarmCategory {
    if is_manifest_or_config_file(path) {
        return WarmCategory::Manifest;
    }

    if is_source_file(path) {
        return WarmCategory::Source;
    }

    WarmCategory::Small
}

fn is_manifest_or_config_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = file_name.to_ascii_lowercase();
    if path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| matches!(part, "config" | ".config"))
    }) {
        return true;
    }

    if lower.starts_with("readme")
        || lower == "cargo.toml"
        || lower == "cargo.lock"
        || lower == "package.json"
        || lower == "package-lock.json"
        || lower == "pnpm-lock.yaml"
        || lower == "yarn.lock"
        || lower == "bun.lockb"
        || lower == "pyproject.toml"
        || lower == "requirements.txt"
        || lower.starts_with("requirements-")
        || lower == "go.mod"
        || lower == "go.sum"
        || lower == "gemfile"
        || lower == "gemfile.lock"
        || lower == "dockerfile"
        || lower.starts_with("docker-compose")
        || lower == "makefile"
        || lower == ".env.example"
    {
        return true;
    }

    lower.ends_with(".config.js")
        || lower.ends_with(".config.cjs")
        || lower.ends_with(".config.mjs")
        || lower.ends_with(".config.ts")
        || lower.starts_with("tsconfig")
        || lower.starts_with("vite.config")
        || lower.starts_with("next.config")
}

fn is_source_file(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "rb"
            | "php"
            | "cs"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "swift"
            | "scala"
            | "clj"
            | "ex"
            | "exs"
            | "css"
            | "scss"
            | "sass"
            | "html"
            | "md"
    )
}

fn stable_file_version_id(
    relative_path: &Path,
    kind: FileKind,
    object_id: Option<&str>,
    size_bytes: Option<u64>,
) -> CaptureResult<FileVersionId> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"loom-file-version-v1\n");
    hasher.update(path_to_store_string(relative_path).as_bytes());
    hasher.update(b"\n");
    hasher.update(file_kind_to_store(kind).as_bytes());
    hasher.update(b"\n");
    hasher.update(object_id.unwrap_or("-").as_bytes());
    hasher.update(b"\n");
    hasher.update(
        size_bytes
            .map(|size| size.to_string())
            .unwrap_or_else(|| "-".to_string())
            .as_bytes(),
    );

    FileVersionId::new(format!("file-version-b3-{}", hasher.finalize().to_hex()))
        .map_err(Into::into)
}

fn file_kind_to_store(kind: FileKind) -> &'static str {
    match kind {
        FileKind::File => "file",
        FileKind::Directory => "directory",
        FileKind::Symlink => "symlink",
        FileKind::Unsupported => "unsupported",
    }
}

fn read_file_bytes_for_secret_check(path: &Path) -> CaptureResult<Vec<u8>> {
    fs::read(path).map_err(|source| CaptureError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn relative_to(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn current_timestamp() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    format!("unix:{}", duration.as_secs())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SecretDetector;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SecretFinding {
    rule_id: &'static str,
    line_number: usize,
    redacted_evidence: String,
}

impl SecretFinding {
    fn policy_reason(&self) -> String {
        format!(
            "secret blocked by Loom policy rule {} at line {}; evidence: {}",
            self.rule_id, self.line_number, self.redacted_evidence
        )
    }
}

impl SecretDetector {
    fn scan_bytes(&self, bytes: &[u8]) -> Vec<SecretFinding> {
        if !looks_like_text(bytes) {
            return Vec::new();
        }

        let text = String::from_utf8_lossy(bytes);
        let mut findings = Vec::new();
        for (line_index, line) in text.lines().enumerate() {
            scan_secret_line(line.trim(), line_index + 1, &mut findings);
            if findings.len() >= MAX_SECRET_FINDINGS {
                findings.truncate(MAX_SECRET_FINDINGS);
                break;
            }
        }

        findings
    }
}

fn scan_secret_line(line: &str, line_number: usize, findings: &mut Vec<SecretFinding>) {
    if findings.len() >= MAX_SECRET_FINDINGS {
        return;
    }

    if line.starts_with("-----BEGIN ")
        && line.ends_with(" PRIVATE KEY-----")
        && !line.contains("PUBLIC KEY")
    {
        findings.push(SecretFinding {
            rule_id: "private_key_pem",
            line_number,
            redacted_evidence: "-----BEGIN <redacted> PRIVATE KEY-----".to_string(),
        });
        return;
    }

    for (rule_id, prefix, min_tail, evidence) in [
        ("aws_access_key_id", "AKIA", 16, "AKIA<redacted>"),
        ("aws_access_key_id", "ASIA", 16, "ASIA<redacted>"),
        ("github_token", "ghp_", 30, "ghp_<redacted>"),
        ("github_token", "github_pat_", 30, "github_pat_<redacted>"),
        ("openai_api_key", "sk-", 32, "sk-<redacted>"),
        ("stripe_secret_key", "sk_live_", 16, "sk_live_<redacted>"),
        ("stripe_secret_key", "sk_test_", 16, "sk_test_<redacted>"),
    ] {
        if contains_prefixed_token(line, prefix, min_tail) {
            findings.push(SecretFinding {
                rule_id,
                line_number,
                redacted_evidence: evidence.to_string(),
            });
            return;
        }
    }

    if let Some(evidence) = dotenv_high_entropy_secret(line) {
        findings.push(SecretFinding {
            rule_id: "dotenv_high_entropy_secret",
            line_number,
            redacted_evidence: evidence,
        });
    }
}

fn contains_prefixed_token(line: &str, prefix: &str, min_tail_len: usize) -> bool {
    let mut search_start = 0;
    while let Some(offset) = line[search_start..].find(prefix) {
        let start = search_start + offset;
        let tail_start = start + prefix.len();
        let tail_len = line[tail_start..]
            .chars()
            .take_while(|character| is_token_char(*character))
            .count();
        let end = tail_start + tail_len;
        if tail_len >= min_tail_len && has_token_boundary(line, start, end) {
            return true;
        }

        search_start = tail_start;
        if search_start >= line.len() {
            return false;
        }
    }

    false
}

fn dotenv_high_entropy_secret(line: &str) -> Option<String> {
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if !is_dotenv_secret_key(key) {
        return None;
    }

    let value = trim_value(value);
    if is_placeholder_value(value) || !looks_high_entropy(value) {
        return None;
    }

    Some(format!("{key}=<redacted>"))
}

fn trim_value(value: &str) -> &str {
    value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .split(" #")
        .next()
        .unwrap_or("")
        .trim()
}

fn is_dotenv_secret_key(key: &str) -> bool {
    if key.is_empty()
        || !key.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return false;
    }

    [
        "SECRET",
        "TOKEN",
        "API_KEY",
        "ACCESS_KEY",
        "PRIVATE_KEY",
        "PASSWORD",
    ]
    .iter()
    .any(|marker| key.contains(marker))
}

fn looks_high_entropy(value: &str) -> bool {
    if value.len() < 24 || value.split_whitespace().count() > 1 {
        return false;
    }

    let has_lower = value
        .chars()
        .any(|character| character.is_ascii_lowercase());
    let has_upper = value
        .chars()
        .any(|character| character.is_ascii_uppercase());
    let has_digit = value.chars().any(|character| character.is_ascii_digit());
    let has_symbol = value
        .chars()
        .any(|character| matches!(character, '_' | '-' | '.' | '/' | '+' | '='));
    let classes = [has_lower, has_upper, has_digit, has_symbol]
        .into_iter()
        .filter(|present| *present)
        .count();

    classes >= 3
}

fn is_placeholder_value(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "example",
        "placeholder",
        "changeme",
        "change_me",
        "dummy",
        "redacted",
        "not-a-secret",
        "test",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn is_token_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
}

fn has_token_boundary(line: &str, start: usize, end: usize) -> bool {
    let before_ok = line[..start]
        .chars()
        .next_back()
        .map_or(true, |character| !is_token_char(character));
    let after_ok = line[end..]
        .chars()
        .next()
        .map_or(true, |character| !is_token_char(character));
    before_ok && after_ok
}

fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }

    if bytes.contains(&0) {
        return false;
    }

    let control = bytes
        .iter()
        .filter(|byte| **byte < 0x20 && !matches!(**byte, b'\n' | b'\r' | b'\t'))
        .count();
    control * 100 / bytes.len() <= 5
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::{FolderScope, HydrationState, SharedFolderId};
    use loom_store::LocalStore;
    use std::fs;

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

    #[test]
    fn cache_policy_presets_are_deterministic_internal_data() {
        let presets = cache_policy_presets();
        let names = presets
            .iter()
            .map(CachePolicyPreset::name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "online-first",
                "offline-pinned",
                "low-disk",
                "agent-sandbox",
                "ci-ephemeral"
            ]
        );
        assert_eq!(presets[0].warm_max_bytes(), DEFAULT_WARM_MAX_BYTES);
        assert!(presets
            .iter()
            .find(|preset| preset.name() == "low-disk")
            .expect("low-disk preset exists")
            .pins_required());
        assert_eq!(
            presets
                .iter()
                .find(|preset| preset.name() == "ci-ephemeral")
                .expect("ci preset exists")
                .prune_target(),
            Some(0)
        );
    }

    #[test]
    fn captures_files_and_ignores_generated_directories() {
        let fixture = TestFolder::new();
        fixture.write("src/main.rs", "fn main() {}\n");
        fixture.write("node_modules/left-pad/index.js", "module.exports = true;\n");
        fixture.write(".git/objects/ignored", "git object\n");

        let capture = fixture.capture();
        let paths = capture
            .file_versions()
            .iter()
            .map(|version| path_to_store_string(version.path()))
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["src", "src/main.rs"]);
        assert_eq!(capture.summary().captured_files(), 1);
        assert_eq!(capture.summary().ignored_entries(), 3);
        assert!(capture
            .ignored()
            .iter()
            .any(|notice| notice.relative_path() == Path::new(".loom")));
        assert!(capture
            .ignored()
            .iter()
            .any(|notice| notice.relative_path() == Path::new(".git")));
        assert!(capture
            .ignored()
            .iter()
            .any(|notice| notice.relative_path() == Path::new("node_modules")));
    }

    #[test]
    fn normal_capture_records_hydrated_cache_metadata() {
        let fixture = TestFolder::new();
        fixture.write("src/main.rs", "fn main() {}\n");

        let capture = fixture.capture();
        let file = capture
            .file_versions()
            .iter()
            .find(|version| version.path() == Path::new("src/main.rs"))
            .expect("captured file exists");
        let object_id = file.object_id().expect("captured file has object id");
        let reopened = LocalStore::open(&fixture.root).expect("store reopens");
        let entry = reopened
            .cache_entry(object_id)
            .expect("cache metadata reads")
            .expect("cache metadata exists");

        assert_eq!(entry.object_id(), object_id);
        assert_eq!(entry.hydration_state(), HydrationState::Hydrated);
        assert_eq!(entry.size_bytes(), file.size_bytes());
    }

    #[test]
    fn blocks_secret_files_before_object_cache_write() {
        let fixture = TestFolder::new();
        let raw_secret = ["sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456"].concat();
        fixture.write("safe.txt", "safe\n");
        fixture.write("secrets.env", &format!("OPENAI_API_KEY={raw_secret}\n"));

        let capture = fixture.capture();
        let paths = capture
            .file_versions()
            .iter()
            .map(|version| path_to_store_string(version.path()))
            .collect::<Vec<_>>();

        assert_eq!(paths, vec!["safe.txt"]);
        assert_eq!(capture.summary().blocked_secret_files(), 1);
        assert_eq!(fixture.object_file_count(), 1);
        let reason = capture.blocked()[0].reason();
        assert!(reason.contains("openai_api_key"));
        assert!(reason.contains("sk-<redacted>"));
        assert!(!reason.contains(&raw_secret));
    }

    #[test]
    fn blocks_late_secret_past_old_prefix_before_object_cache_write() {
        let fixture = TestFolder::new();
        let raw_secret = ["sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456"].concat();
        let mut content = "a".repeat(OLD_SECRET_SCAN_PREFIX_BYTES + 32);
        content.push_str("\nOPENAI_API_KEY=");
        content.push_str(&raw_secret);
        content.push('\n');
        fixture.write("late-secret.env", &content);

        let capture = fixture.capture();

        assert!(capture.file_versions().is_empty());
        assert_eq!(capture.summary().captured_files(), 0);
        assert_eq!(capture.summary().blocked_secret_files(), 1);
        assert_eq!(fixture.object_file_count(), 0);
        assert!(!fixture.object_cache_contains(raw_secret.as_bytes()));
        let reason = capture.blocked()[0].reason();
        assert!(reason.contains("openai_api_key"));
        assert!(reason.contains("sk-<redacted>"));
        assert!(!reason.contains(&raw_secret));
    }

    #[test]
    fn file_capture_policy_matches_generated_folder_and_secret_rules() {
        match evaluate_file_capture_policy(Path::new("node_modules/pkg/index.js"), b"module\n") {
            FileCapturePolicyDecision::Ignore { path, reason } => {
                assert_eq!(path, Path::new("node_modules"));
                assert!(reason.contains("generated Node"));
            }
            other => panic!("expected generated folder ignore, got {other:?}"),
        }

        let raw_secret = ["sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456"].concat();
        match evaluate_file_capture_policy(
            Path::new("secrets.env"),
            format!("OPENAI_API_KEY={raw_secret}\n").as_bytes(),
        ) {
            FileCapturePolicyDecision::Block { path, reason } => {
                assert_eq!(path, Path::new("secrets.env"));
                assert!(reason.contains("openai_api_key"));
                assert!(!reason.contains(&raw_secret));
            }
            other => panic!("expected secret block, got {other:?}"),
        }

        assert_eq!(
            evaluate_file_capture_policy(Path::new("src/main.rs"), b"fn main() {}\n"),
            FileCapturePolicyDecision::Capture
        );
    }

    #[test]
    fn diffs_current_worktree_against_a_folder_revision() {
        let fixture = TestFolder::new();
        fixture.write("a.txt", "one\n");
        fixture.write("b.txt", "two\n");
        let first_capture = fixture.capture();
        let first_revision = fixture
            .store
            .coalesce_folder_revision(RevisionBoundary::LoomCommand, first_capture.file_versions())
            .expect("revision creates")
            .revision()
            .clone();

        fixture.write("a.txt", "changed\n");
        fixture.remove("b.txt");
        fixture.write("c.txt", "three\n");
        let current = fixture.capture();

        let diff = diff_revision_to_capture(&first_revision, &current).expect("diff creates");

        assert_eq!(store_paths(diff.created()), vec!["c.txt"]);
        assert_eq!(store_paths(diff.modified()), vec!["a.txt"]);
        assert_eq!(store_paths(diff.deleted()), vec!["b.txt"]);
        assert_eq!(diff.unchanged(), 0);
    }

    #[test]
    fn restore_materializes_tracked_source_and_preserves_local_context() {
        let fixture = TestFolder::new();
        fixture.write("README.md", "before\n");
        fixture.write(".git/config", "local git metadata\n");
        fixture.write("node_modules/pkg/index.js", "module.exports = true;\n");
        let first_capture = fixture.capture();
        let first_revision = fixture
            .store
            .coalesce_folder_revision(RevisionBoundary::LoomCommand, first_capture.file_versions())
            .expect("revision creates")
            .revision()
            .clone();

        fixture.write("README.md", "after\n");
        fixture.write("new.txt", "temporary\n");
        let current = fixture.capture();
        let report = RestoreEngine::new(&fixture.store)
            .restore(&first_revision, &current)
            .expect("restore applies");

        assert_eq!(report.revision_id(), first_revision.id());
        assert_eq!(fixture.read("README.md"), "before\n");
        assert!(!fixture.root.join("new.txt").exists());
        assert_eq!(fixture.read(".git/config"), "local git metadata\n");
        assert_eq!(
            fixture.read("node_modules/pkg/index.js"),
            "module.exports = true;\n"
        );
    }

    #[test]
    fn restore_missing_target_object_leaves_worktree_unchanged() {
        let fixture = TestFolder::new();
        fixture.write("README.md", "before\n");
        let first_capture = fixture.capture();
        let readme_object_id = first_capture
            .file_versions()
            .iter()
            .find(|version| version.path() == Path::new("README.md"))
            .and_then(FileVersion::object_id)
            .cloned()
            .expect("readme has an object");
        let first_revision = fixture
            .store
            .coalesce_folder_revision(RevisionBoundary::LoomCommand, first_capture.file_versions())
            .expect("revision creates")
            .revision()
            .clone();

        fs::remove_file(fixture.store.object_cache().path_for(&readme_object_id))
            .expect("checkpoint object removes");
        fixture.write("README.md", "after\n");
        fixture.write("new.txt", "temporary\n");
        let current = fixture.capture();

        let error = RestoreEngine::new(&fixture.store)
            .restore(&first_revision, &current)
            .expect_err("restore refuses missing object before mutation");

        assert!(matches!(
            error,
            CaptureError::Store(StoreError::MissingObject { .. })
        ));
        assert_eq!(fixture.read("README.md"), "after\n");
        assert_eq!(fixture.read("new.txt"), "temporary\n");
    }

    #[test]
    fn restore_target_file_child_conflict_leaves_worktree_unchanged() {
        let fixture = TestFolder::new();
        let foo_object = fixture
            .store
            .object_cache()
            .write_bytes(b"foo\n")
            .expect("foo object writes");
        let child_object = fixture
            .store
            .object_cache()
            .write_bytes(b"child\n")
            .expect("child object writes");
        let foo_version = FileVersion::new(
            FileVersionId::new("file-version-foo").expect("file version id"),
            "foo",
            FileKind::File,
            Some(foo_object.id().clone()),
            Some(foo_object.size_bytes()),
            "unix:1",
        )
        .expect("foo file version creates");
        let child_version = FileVersion::new(
            FileVersionId::new("file-version-foo-child").expect("file version id"),
            "foo/bar.txt",
            FileKind::File,
            Some(child_object.id().clone()),
            Some(child_object.size_bytes()),
            "unix:1",
        )
        .expect("child file version creates");
        let conflicting_revision = fixture
            .store
            .coalesce_folder_revision(RevisionBoundary::LoomCommand, &[foo_version, child_version])
            .expect("conflicting revision creates")
            .revision()
            .clone();

        fixture.write("new.txt", "temporary\n");
        let current = fixture.capture();

        let error = RestoreEngine::new(&fixture.store)
            .restore(&conflicting_revision, &current)
            .expect_err("restore refuses target plan conflicts before mutation");

        assert!(matches!(error, CaptureError::UnsafeRestore { .. }));
        assert_eq!(fixture.read("new.txt"), "temporary\n");
        assert!(!fixture.root.join("foo").exists());
    }

    #[test]
    fn restore_refuses_secret_blocked_working_entries() {
        let fixture = TestFolder::new();
        fixture.write("README.md", "before\n");
        let first_capture = fixture.capture();
        let first_revision = fixture
            .store
            .coalesce_folder_revision(RevisionBoundary::LoomCommand, first_capture.file_versions())
            .expect("revision creates")
            .revision()
            .clone();
        let raw_secret = ["sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456"].concat();
        fixture.write("secrets.env", &format!("OPENAI_API_KEY={raw_secret}\n"));
        let current = fixture.capture();

        let error = RestoreEngine::new(&fixture.store)
            .restore(&first_revision, &current)
            .expect_err("restore refuses blocked files");

        assert!(matches!(error, CaptureError::UnsafeRestore { .. }));
        assert!(fixture.root.join("secrets.env").exists());
    }

    #[test]
    fn restore_rejects_revisions_that_try_to_materialize_git_metadata() {
        let fixture = TestFolder::new();
        let object = fixture
            .store
            .object_cache()
            .write_bytes(b"[core]\n")
            .expect("object writes");
        let version = FileVersion::new(
            FileVersionId::new("file-version-git-config").expect("file version id"),
            ".git/config",
            FileKind::File,
            Some(object.id().clone()),
            Some(object.size_bytes()),
            "unix:1",
        )
        .expect("file version creates");
        let protected_revision = fixture
            .store
            .coalesce_folder_revision(RevisionBoundary::LoomCommand, &[version])
            .expect("revision creates")
            .revision()
            .clone();
        let current = fixture.capture();

        let error = RestoreEngine::new(&fixture.store)
            .restore(&protected_revision, &current)
            .expect_err("restore rejects protected paths");

        assert!(matches!(error, CaptureError::UnsafeRestore { .. }));
    }

    struct TestFolder {
        _dir: tempfile::TempDir,
        root: PathBuf,
        store: LocalStore,
    }

    impl TestFolder {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let root = dir.path().join("shared");
            fs::create_dir_all(&root).expect("root creates");
            let store = LocalStore::open_or_init(&root)
                .expect("store initializes")
                .into_store();

            Self {
                _dir: dir,
                root,
                store,
            }
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.root.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent creates");
            }
            fs::write(path, content).expect("file writes");
        }

        fn read(&self, path: &str) -> String {
            fs::read_to_string(self.root.join(path)).expect("file reads")
        }

        fn remove(&self, path: &str) {
            fs::remove_file(self.root.join(path)).expect("file removes");
        }

        fn capture(&self) -> WorktreeCapture {
            let request = CaptureRequest::new(
                self.store.shared_folder().clone(),
                RevisionBoundary::LoomCommand,
            );
            CaptureEngine::new(&self.store)
                .capture(&request)
                .expect("capture succeeds")
        }

        fn object_file_count(&self) -> usize {
            let objects = self.store.store_root().join("objects");
            let mut count = 0;
            let mut stack = vec![objects];
            while let Some(path) = stack.pop() {
                for entry in fs::read_dir(path).expect("directory reads") {
                    let entry = entry.expect("entry reads");
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

        fn object_cache_contains(&self, needle: &[u8]) -> bool {
            let objects = self.store.store_root().join("objects");
            let mut stack = vec![objects];
            while let Some(path) = stack.pop() {
                for entry in fs::read_dir(path).expect("directory reads") {
                    let entry = entry.expect("entry reads");
                    let entry_path = entry.path();
                    if entry_path.is_dir() {
                        stack.push(entry_path);
                    } else {
                        let bytes = fs::read(&entry_path).expect("object bytes read");
                        if bytes.windows(needle.len()).any(|window| window == needle) {
                            return true;
                        }
                    }
                }
            }

            false
        }
    }

    fn store_paths(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| path_to_store_string(path))
            .collect()
    }
}
