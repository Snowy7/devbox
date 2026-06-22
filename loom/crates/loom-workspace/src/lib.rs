//! Loom workspace adapter boundary.
//!
//! Workspace adapters expose a folder revision as a session view. The agent
//! virtual adapter keeps source files out of the materialized folder and writes
//! session overlays under `.loom/workspaces` until an explicit checkpoint
//! coalesces them into Loom file versions and a folder revision.

use loom_core::{
    Checkpoint, FileKind, FileVersion, FileVersionId, FolderRevision, FolderRevisionId,
    HydrationState, LoomError, ObjectId, RevisionBoundary, WorkspaceId, WorkspaceKind,
    WorkspaceSession, WorkspaceSessionId, WorkspaceSessionState,
};
use loom_store::{path_to_store_string, CoalescedRevision, LocalStore, StoreError};
use loom_sync::{hydrate_object_from_remote, LoomRemote, SyncError};
use loom_worktree::{evaluate_file_capture_policy, FileCapturePolicyDecision};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CRATE_ROLE: &str = "Loom workspace adapter core for virtual agent sessions";

const WORKSPACES_DIR: &str = "workspaces";
const SESSIONS_DIR: &str = "sessions";
const SESSION_FILE: &str = "session.tsv";
const OVERLAY_FILE: &str = "overlay.tsv";

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAdapterKind {
    AgentVirtual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAdapterCapabilities {
    kind: WorkspaceAdapterKind,
    lists_metadata: bool,
    reads_files: bool,
    writes_overlay: bool,
    hydrates_cache_on_read: bool,
    hydrates_path_to_cache: bool,
    dehydrates_path: bool,
    pins_path: bool,
    checkpoints_overlay: bool,
}

impl WorkspaceAdapterCapabilities {
    pub fn agent_virtual() -> Self {
        Self {
            kind: WorkspaceAdapterKind::AgentVirtual,
            lists_metadata: true,
            reads_files: true,
            writes_overlay: true,
            hydrates_cache_on_read: true,
            hydrates_path_to_cache: true,
            dehydrates_path: false,
            pins_path: false,
            checkpoints_overlay: true,
        }
    }

    pub fn kind(&self) -> WorkspaceAdapterKind {
        self.kind
    }

    pub fn lists_metadata(&self) -> bool {
        self.lists_metadata
    }

    pub fn reads_files(&self) -> bool {
        self.reads_files
    }

    pub fn writes_overlay(&self) -> bool {
        self.writes_overlay
    }

    pub fn hydrates_cache_on_read(&self) -> bool {
        self.hydrates_cache_on_read
    }

    pub fn hydrates_path_to_cache(&self) -> bool {
        self.hydrates_path_to_cache
    }

    pub fn dehydrates_path(&self) -> bool {
        self.dehydrates_path
    }

    pub fn pins_path(&self) -> bool {
        self.pins_path
    }

    pub fn checkpoints_overlay(&self) -> bool {
        self.checkpoints_overlay
    }
}

pub trait WorkspaceView {
    fn session(&self) -> &WorkspaceSession;
    fn capabilities(&self) -> WorkspaceAdapterCapabilities;
    fn list_metadata(&self, scope: &Path) -> WorkspaceResult<Vec<WorkspaceEntryMetadata>>;
    fn read_file(&self, path: &Path) -> WorkspaceResult<Vec<u8>>;
    fn write_file(&mut self, path: &Path, bytes: &[u8]) -> WorkspaceResult<()>;
    fn hydrate_path(&self, scope: &Path) -> WorkspaceResult<WorkspaceHydrationReport>;
    fn dehydrate_path(&self, scope: &Path) -> WorkspaceResult<WorkspaceHydrationReport>;
    fn pin_path(&self, scope: &Path) -> WorkspaceResult<WorkspacePinReport>;
    fn diff_overlay(&self) -> WorkspaceResult<WorkspaceOverlayDiff>;
    fn checkpoint_overlay(&mut self, message: &str) -> WorkspaceResult<WorkspaceCheckpoint>;
    fn close(self) -> WorkspaceResult<WorkspaceCloseReport>
    where
        Self: Sized;
    fn discard(self) -> WorkspaceResult<WorkspaceCloseReport>
    where
        Self: Sized;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSessionRequest {
    pub session_id: Option<WorkspaceSessionId>,
    pub base_revision_id: Option<FolderRevisionId>,
}

impl WorkspaceSessionRequest {
    pub fn new() -> Self {
        Self {
            session_id: None,
            base_revision_id: None,
        }
    }
}

impl Default for WorkspaceSessionRequest {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEntrySource {
    BaseRevision,
    Overlay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntryMetadata {
    path: PathBuf,
    kind: FileKind,
    size_bytes: Option<u64>,
    hydration_state: HydrationState,
    source: WorkspaceEntrySource,
}

impl WorkspaceEntryMetadata {
    fn new(
        path: PathBuf,
        kind: FileKind,
        size_bytes: Option<u64>,
        hydration_state: HydrationState,
        source: WorkspaceEntrySource,
    ) -> Self {
        Self {
            path,
            kind,
            size_bytes,
            hydration_state,
            source,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> &FileKind {
        &self.kind
    }

    pub fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }

    pub fn hydration_state(&self) -> HydrationState {
        self.hydration_state
    }

    pub fn source(&self) -> WorkspaceEntrySource {
        self.source
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceOverlayDiff {
    created: Vec<PathBuf>,
    modified: Vec<PathBuf>,
    deleted: Vec<PathBuf>,
    unchanged: usize,
}

impl WorkspaceOverlayDiff {
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceHydrationReport {
    fetched_objects: usize,
    already_cached_objects: usize,
    overlay_files: usize,
    directories: usize,
}

impl WorkspaceHydrationReport {
    pub fn fetched_objects(&self) -> usize {
        self.fetched_objects
    }

    pub fn already_cached_objects(&self) -> usize {
        self.already_cached_objects
    }

    pub fn overlay_files(&self) -> usize {
        self.overlay_files
    }

    pub fn directories(&self) -> usize {
        self.directories
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePinReport {
    path: PathBuf,
}

impl WorkspacePinReport {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCheckpoint {
    coalesced: CoalescedRevision,
    checkpoint: Checkpoint,
    overlay_files: usize,
}

impl WorkspaceCheckpoint {
    pub fn coalesced(&self) -> &CoalescedRevision {
        &self.coalesced
    }

    pub fn checkpoint(&self) -> &Checkpoint {
        &self.checkpoint
    }

    pub fn overlay_files(&self) -> usize {
        self.overlay_files
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCloseReport {
    session_id: WorkspaceSessionId,
    state: WorkspaceSessionState,
    discarded_overlay_files: usize,
}

impl WorkspaceCloseReport {
    pub fn session_id(&self) -> &WorkspaceSessionId {
        &self.session_id
    }

    pub fn state(&self) -> WorkspaceSessionState {
        self.state
    }

    pub fn discarded_overlay_files(&self) -> usize {
        self.discarded_overlay_files
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OverlayFile {
    path: PathBuf,
    object_id: ObjectId,
    size_bytes: u64,
}

#[derive(Clone)]
pub struct AgentWorkspaceAdapter<'a> {
    store: LocalStore,
    remote: Option<&'a dyn LoomRemote>,
}

impl<'a> AgentWorkspaceAdapter<'a> {
    pub fn new(store: LocalStore) -> Self {
        Self {
            store,
            remote: None,
        }
    }

    pub fn with_remote(store: LocalStore, remote: &'a dyn LoomRemote) -> Self {
        Self {
            store,
            remote: Some(remote),
        }
    }

    pub fn capabilities(&self) -> WorkspaceAdapterCapabilities {
        WorkspaceAdapterCapabilities::agent_virtual()
    }

    pub fn create_session(
        &self,
        request: WorkspaceSessionRequest,
    ) -> WorkspaceResult<AgentWorkspaceSession<'a>> {
        let revision = match request.base_revision_id {
            Some(id) => self.revision_by_id(&id)?,
            None => self
                .store
                .latest_revision()?
                .ok_or(WorkspaceError::NoBaseRevision)?,
        };
        let session_id = request
            .session_id
            .unwrap_or_else(|| generated_session_id(self.store.shared_folder().id().as_str()));
        validate_session_id(session_id.as_str())?;
        let session_dir = self.session_dir(&session_id);
        if session_dir.exists() {
            return Err(WorkspaceError::SessionAlreadyExists(session_id));
        }

        let session = WorkspaceSession::new(
            session_id,
            workspace_id_for(&self.store)?,
            self.store.shared_folder().id().clone(),
            revision.id().clone(),
            WorkspaceKind::AgentVirtual,
            WorkspaceSessionState::Open,
            current_timestamp(),
        )?;
        create_dir_all(&session_dir)?;
        write_session_file(&session_dir, &session)?;
        write_overlay_file(&session_dir, &BTreeMap::new())?;
        self.load_session_from_record(session)
    }

    pub fn open_session(
        &self,
        session_id: &WorkspaceSessionId,
    ) -> WorkspaceResult<AgentWorkspaceSession<'a>> {
        validate_session_id(session_id.as_str())?;
        let session_dir = self.session_dir(session_id);
        if !session_dir.is_dir() {
            return Err(WorkspaceError::MissingSession(session_id.clone()));
        }
        let session = read_session_file(&session_dir)?;
        if session.state() != WorkspaceSessionState::Open {
            return Err(WorkspaceError::ClosedSession(session.id().clone()));
        }
        self.load_session_from_record(session)
    }

    pub fn list_sessions(&self) -> WorkspaceResult<Vec<WorkspaceSession>> {
        let root = self.sessions_root();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        for entry in fs::read_dir(&root).map_err(|source| WorkspaceError::Io {
            path: root.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| WorkspaceError::Io {
                path: root.clone(),
                source,
            })?;
            let entry_path = entry.path();
            if entry_path.is_dir() {
                sessions.push(read_session_file(&entry_path)?);
            }
        }
        sessions.sort_by(|left, right| left.id().cmp(right.id()));
        Ok(sessions)
    }

    fn load_session_from_record(
        &self,
        session: WorkspaceSession,
    ) -> WorkspaceResult<AgentWorkspaceSession<'a>> {
        let base_revision = self.revision_by_id(session.base_revision_id())?;
        let base_versions = versions_for_revision(&self.store, &base_revision)?;
        let overlay = read_overlay_file(&self.session_dir(session.id()))?;
        Ok(AgentWorkspaceSession {
            store: self.store.clone(),
            remote: self.remote,
            session,
            base_revision,
            base_versions,
            overlay,
        })
    }

    fn revision_by_id(&self, id: &FolderRevisionId) -> WorkspaceResult<FolderRevision> {
        self.store
            .revision_by_id(id)?
            .ok_or_else(|| WorkspaceError::MissingBaseRevision(id.clone()))
    }

    fn sessions_root(&self) -> PathBuf {
        self.store
            .store_root()
            .join(WORKSPACES_DIR)
            .join(SESSIONS_DIR)
    }

    fn session_dir(&self, session_id: &WorkspaceSessionId) -> PathBuf {
        self.sessions_root().join(session_id.as_str())
    }
}

pub struct AgentWorkspaceSession<'a> {
    store: LocalStore,
    remote: Option<&'a dyn LoomRemote>,
    session: WorkspaceSession,
    base_revision: FolderRevision,
    base_versions: BTreeMap<PathBuf, FileVersion>,
    overlay: BTreeMap<PathBuf, OverlayFile>,
}

impl<'a> AgentWorkspaceSession<'a> {
    pub fn base_revision(&self) -> &FolderRevision {
        &self.base_revision
    }

    pub fn overlay_file_count(&self) -> usize {
        self.overlay.len()
    }

    fn session_dir(&self) -> PathBuf {
        self.store
            .store_root()
            .join(WORKSPACES_DIR)
            .join(SESSIONS_DIR)
            .join(self.session.id().as_str())
    }

    fn persist_overlay(&self) -> WorkspaceResult<()> {
        write_overlay_file(&self.session_dir(), &self.overlay)
    }

    fn persist_session(&self) -> WorkspaceResult<()> {
        write_session_file(&self.session_dir(), &self.session)
    }

    fn base_metadata_for(&self, version: &FileVersion) -> WorkspaceResult<WorkspaceEntryMetadata> {
        let hydration_state = match version.kind() {
            FileKind::File => {
                let Some(object_id) = version.object_id() else {
                    return Err(WorkspaceError::PathConflict {
                        path: version.path().to_path_buf(),
                        reason: "file version has no content object".to_string(),
                    });
                };
                self.hydration_state_for_object(object_id)?
            }
            FileKind::Directory => HydrationState::Hydrated,
            FileKind::Symlink | FileKind::Unsupported => HydrationState::RemoteOnly,
        };
        Ok(WorkspaceEntryMetadata::new(
            version.path().to_path_buf(),
            version.kind().clone(),
            version.size_bytes(),
            hydration_state,
            WorkspaceEntrySource::BaseRevision,
        ))
    }

    fn hydration_state_for_object(&self, object_id: &ObjectId) -> WorkspaceResult<HydrationState> {
        if self.store.object_cache().exists(object_id) {
            return Ok(HydrationState::Hydrated);
        }
        Ok(self
            .store
            .cache_entry(object_id)?
            .map(|entry| entry.hydration_state())
            .unwrap_or(HydrationState::RemoteOnly))
    }

    fn read_overlay_file(&self, overlay: &OverlayFile) -> WorkspaceResult<Vec<u8>> {
        self.store
            .object_cache()
            .read(&overlay.object_id)
            .map_err(WorkspaceError::Store)
    }

    fn read_base_file(&self, path: &Path, version: &FileVersion) -> WorkspaceResult<Vec<u8>> {
        if version.kind() != &FileKind::File {
            return Err(WorkspaceError::NotAFile(path.to_path_buf()));
        }
        let object_id = version
            .object_id()
            .ok_or_else(|| WorkspaceError::PathConflict {
                path: path.to_path_buf(),
                reason: "file version has no content object".to_string(),
            })?;
        if !self.store.object_cache().exists(object_id) {
            let remote = self
                .remote
                .ok_or_else(|| WorkspaceError::ObjectUnavailable {
                    path: path.to_path_buf(),
                    object_id: object_id.clone(),
                })?;
            hydrate_object_from_remote(&self.store, remote, object_id, version.size_bytes())?;
        }
        self.store
            .object_cache()
            .read(object_id)
            .map_err(WorkspaceError::Store)
    }

    fn hydrate_base_version(
        &self,
        version: &FileVersion,
        report: &mut WorkspaceHydrationReport,
    ) -> WorkspaceResult<()> {
        match version.kind() {
            FileKind::Directory => {
                report.directories += 1;
                Ok(())
            }
            FileKind::File => {
                let object_id =
                    version
                        .object_id()
                        .ok_or_else(|| WorkspaceError::PathConflict {
                            path: version.path().to_path_buf(),
                            reason: "file version has no content object".to_string(),
                        })?;
                if self.store.object_cache().exists(object_id) {
                    report.already_cached_objects += 1;
                    return Ok(());
                }
                let remote = self
                    .remote
                    .ok_or_else(|| WorkspaceError::ObjectUnavailable {
                        path: version.path().to_path_buf(),
                        object_id: object_id.clone(),
                    })?;
                if hydrate_object_from_remote(&self.store, remote, object_id, version.size_bytes())?
                {
                    report.fetched_objects += 1;
                } else {
                    report.already_cached_objects += 1;
                }
                Ok(())
            }
            FileKind::Symlink | FileKind::Unsupported => {
                Err(WorkspaceError::UnsupportedOperation {
                    operation: "hydrate symlink or unsupported entry",
                    adapter: "agent virtual workspace",
                })
            }
        }
    }

    fn overlay_diff(&self) -> WorkspaceResult<WorkspaceOverlayDiff> {
        let mut created = Vec::new();
        let mut modified = Vec::new();
        let deleted = Vec::new();
        let mut unchanged = self.base_versions.len();

        for overlay in self.overlay.values() {
            match self.base_versions.get(&overlay.path) {
                Some(base) => {
                    if base.kind() == &FileKind::File
                        && base.object_id() == Some(&overlay.object_id)
                        && base.size_bytes() == Some(overlay.size_bytes)
                    {
                        continue;
                    }
                    modified.push(overlay.path.clone());
                    unchanged = unchanged.saturating_sub(1);
                }
                None => created.push(overlay.path.clone()),
            }
        }

        Ok(WorkspaceOverlayDiff::new(
            created, modified, deleted, unchanged,
        ))
    }

    fn versions_with_overlay(&self) -> WorkspaceResult<Vec<FileVersion>> {
        let captured_at = current_timestamp();
        let mut versions = self.base_versions.clone();

        for overlay in self.overlay.values() {
            validate_overlay_write_target(&versions, &self.overlay, &overlay.path)?;
            ensure_parent_directories(&mut versions, &overlay.path, &captured_at)?;
            let version = FileVersion::new(
                stable_file_version_id(
                    &overlay.path,
                    FileKind::File,
                    Some(overlay.object_id.as_str()),
                    Some(overlay.size_bytes),
                )?,
                overlay.path.clone(),
                FileKind::File,
                Some(overlay.object_id.clone()),
                Some(overlay.size_bytes),
                captured_at.clone(),
            )?;
            versions.insert(overlay.path.clone(), version);
        }

        let mut versions = versions.into_values().collect::<Vec<_>>();
        versions.sort_by(|left, right| {
            path_to_store_string(left.path()).cmp(&path_to_store_string(right.path()))
        });
        Ok(versions)
    }

    fn ensure_latest_is_base(&self) -> WorkspaceResult<()> {
        let latest = self
            .store
            .latest_revision()?
            .ok_or(WorkspaceError::NoBaseRevision)?;
        if latest.id() != self.base_revision.id() {
            return Err(WorkspaceError::StaleBaseRevision {
                session_id: self.session.id().clone(),
                expected: self.base_revision.id().clone(),
                actual: latest.id().clone(),
            });
        }
        Ok(())
    }

    fn validate_overlay_capture_policy(&self) -> WorkspaceResult<()> {
        for overlay in self.overlay.values() {
            let bytes = self.read_overlay_file(overlay)?;
            enforce_capture_policy(&overlay.path, &bytes)?;
        }
        Ok(())
    }

    fn remove_session_dir(
        &self,
        state: WorkspaceSessionState,
        discarded_overlay_files: usize,
    ) -> WorkspaceResult<WorkspaceCloseReport> {
        let session_id = self.session.id().clone();
        let dir = self.session_dir();
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(WorkspaceError::Io { path: dir, source }),
        }
        Ok(WorkspaceCloseReport {
            session_id,
            state,
            discarded_overlay_files,
        })
    }
}

impl<'a> WorkspaceView for AgentWorkspaceSession<'a> {
    fn session(&self) -> &WorkspaceSession {
        &self.session
    }

    fn capabilities(&self) -> WorkspaceAdapterCapabilities {
        WorkspaceAdapterCapabilities::agent_virtual()
    }

    fn list_metadata(&self, scope: &Path) -> WorkspaceResult<Vec<WorkspaceEntryMetadata>> {
        let scope = normalize_relative_path(scope, true)?;
        let mut entries = BTreeMap::new();
        for version in self.base_versions.values() {
            if path_is_in_scope(version.path(), &scope) {
                entries.insert(
                    version.path().to_path_buf(),
                    self.base_metadata_for(version)?,
                );
            }
        }
        for overlay in self.overlay.values() {
            if path_is_in_scope(&overlay.path, &scope) {
                entries.insert(
                    overlay.path.clone(),
                    WorkspaceEntryMetadata::new(
                        overlay.path.clone(),
                        FileKind::File,
                        Some(overlay.size_bytes),
                        HydrationState::Hydrated,
                        WorkspaceEntrySource::Overlay,
                    ),
                );
            }
        }
        Ok(entries.into_values().collect())
    }

    fn read_file(&self, path: &Path) -> WorkspaceResult<Vec<u8>> {
        let path = normalize_relative_path(path, false)?;
        if let Some(overlay) = self.overlay.get(&path) {
            return self.read_overlay_file(overlay);
        }
        let version = self
            .base_versions
            .get(&path)
            .ok_or_else(|| WorkspaceError::MissingPath(path.clone()))?;
        self.read_base_file(&path, version)
    }

    fn write_file(&mut self, path: &Path, bytes: &[u8]) -> WorkspaceResult<()> {
        let path = normalize_relative_path(path, false)?;
        validate_overlay_write_target(&self.base_versions, &self.overlay, &path)?;
        enforce_capture_policy(&path, bytes)?;
        let object = self.store.write_object_bytes(bytes)?;
        self.overlay.insert(
            path.clone(),
            OverlayFile {
                path,
                object_id: object.id().clone(),
                size_bytes: object.size_bytes(),
            },
        );
        self.persist_overlay()
    }

    fn hydrate_path(&self, scope: &Path) -> WorkspaceResult<WorkspaceHydrationReport> {
        let scope = normalize_relative_path(scope, true)?;
        let mut report = WorkspaceHydrationReport::default();
        let mut matched = false;

        for version in self.base_versions.values() {
            if path_is_in_scope(version.path(), &scope) {
                matched = true;
                self.hydrate_base_version(version, &mut report)?;
            }
        }
        for overlay in self.overlay.values() {
            if path_is_in_scope(&overlay.path, &scope) {
                matched = true;
                report.overlay_files += 1;
            }
        }
        if !matched {
            return Err(WorkspaceError::MissingPath(scope));
        }
        Ok(report)
    }

    fn dehydrate_path(&self, _scope: &Path) -> WorkspaceResult<WorkspaceHydrationReport> {
        Err(WorkspaceError::UnsupportedOperation {
            operation: "dehydrate path",
            adapter: "agent virtual workspace",
        })
    }

    fn pin_path(&self, scope: &Path) -> WorkspaceResult<WorkspacePinReport> {
        let _ = normalize_relative_path(scope, true)?;
        Err(WorkspaceError::UnsupportedOperation {
            operation: "pin path",
            adapter: "agent virtual workspace",
        })
    }

    fn diff_overlay(&self) -> WorkspaceResult<WorkspaceOverlayDiff> {
        self.overlay_diff()
    }

    fn checkpoint_overlay(&mut self, message: &str) -> WorkspaceResult<WorkspaceCheckpoint> {
        if message.trim().is_empty() {
            return Err(WorkspaceError::EmptyMessage("workspace checkpoint message"));
        }
        self.validate_overlay_capture_policy()?;
        self.ensure_latest_is_base()?;
        let overlay_files = self.overlay.len();
        let versions = self.versions_with_overlay()?;
        let coalesced = self
            .store
            .coalesce_folder_revision(RevisionBoundary::SandboxMerge, &versions)?;
        let checkpoint = self
            .store
            .create_checkpoint(coalesced.revision(), message)?;
        self.overlay.clear();
        self.base_revision = coalesced.revision().clone();
        self.base_versions = versions_for_revision(&self.store, &self.base_revision)?;
        self.session = WorkspaceSession::new(
            self.session.id().clone(),
            self.session.workspace_id().clone(),
            self.session.shared_folder_id().clone(),
            self.base_revision.id().clone(),
            self.session.kind(),
            WorkspaceSessionState::Open,
            self.session.created_at().to_string(),
        )?;
        self.persist_session()?;
        self.persist_overlay()?;
        Ok(WorkspaceCheckpoint {
            coalesced,
            checkpoint,
            overlay_files,
        })
    }

    fn close(self) -> WorkspaceResult<WorkspaceCloseReport> {
        let diff = self.overlay_diff()?;
        if diff.has_changes() {
            return Err(WorkspaceError::UncommittedOverlay {
                session_id: self.session.id().clone(),
                changed_files: diff.created().len() + diff.modified().len() + diff.deleted().len(),
            });
        }
        self.remove_session_dir(WorkspaceSessionState::Closed, self.overlay.len())
    }

    fn discard(self) -> WorkspaceResult<WorkspaceCloseReport> {
        let overlay_files = self.overlay.len();
        self.remove_session_dir(WorkspaceSessionState::Discarded, overlay_files)
    }
}

#[derive(Debug)]
pub enum WorkspaceError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Store(StoreError),
    Sync(SyncError),
    Loom(LoomError),
    NoBaseRevision,
    MissingBaseRevision(FolderRevisionId),
    MissingSession(WorkspaceSessionId),
    SessionAlreadyExists(WorkspaceSessionId),
    ClosedSession(WorkspaceSessionId),
    InvalidSessionId(String),
    InvalidPath {
        path: PathBuf,
        reason: &'static str,
    },
    MissingPath(PathBuf),
    NotAFile(PathBuf),
    ObjectUnavailable {
        path: PathBuf,
        object_id: ObjectId,
    },
    PathConflict {
        path: PathBuf,
        reason: String,
    },
    PolicyIgnored {
        path: PathBuf,
        reason: String,
    },
    PolicyBlocked {
        path: PathBuf,
        reason: String,
    },
    UnsupportedOperation {
        operation: &'static str,
        adapter: &'static str,
    },
    EmptyMessage(&'static str),
    StaleBaseRevision {
        session_id: WorkspaceSessionId,
        expected: FolderRevisionId,
        actual: FolderRevisionId,
    },
    UncommittedOverlay {
        session_id: WorkspaceSessionId,
        changed_files: usize,
    },
    CorruptSession {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not access {}: {source}", path.display()),
            Self::Store(error) => write!(f, "{error}"),
            Self::Sync(error) => write!(f, "{error}"),
            Self::Loom(error) => write!(f, "{error}"),
            Self::NoBaseRevision => {
                write!(f, "no folder revisions yet; run 'loom status' first")
            }
            Self::MissingBaseRevision(revision_id) => {
                write!(f, "workspace base revision {revision_id} is not in local Loom history")
            }
            Self::MissingSession(session_id) => {
                write!(f, "workspace session {session_id} was not found")
            }
            Self::SessionAlreadyExists(session_id) => {
                write!(f, "workspace session {session_id} already exists")
            }
            Self::ClosedSession(session_id) => {
                write!(f, "workspace session {session_id} is not open")
            }
            Self::InvalidSessionId(session_id) => {
                write!(f, "workspace session id '{session_id}' is not path-safe")
            }
            Self::InvalidPath { path, reason } => {
                write!(f, "invalid workspace path {}: {reason}", path.display())
            }
            Self::MissingPath(path) => {
                write!(f, "workspace path {} is not in the session view", path.display())
            }
            Self::NotAFile(path) => write!(f, "workspace path {} is not a file", path.display()),
            Self::ObjectUnavailable { path, object_id } => write!(
                f,
                "workspace path {} needs object {object_id}, but it is not cached and no remote object source is attached",
                path.display()
            ),
            Self::PathConflict { path, reason } => {
                write!(f, "workspace path {} conflicts with the session view: {reason}", path.display())
            }
            Self::PolicyIgnored { path, reason } => {
                write!(f, "workspace write ignored for {}: {reason}", path.display())
            }
            Self::PolicyBlocked { path, reason } => {
                write!(f, "workspace write blocked for {}: {reason}", path.display())
            }
            Self::UnsupportedOperation { operation, adapter } => {
                write!(f, "{operation} is not supported by {adapter}")
            }
            Self::EmptyMessage(kind) => write!(f, "{kind} cannot be empty"),
            Self::StaleBaseRevision {
                session_id,
                expected,
                actual,
            } => write!(
                f,
                "workspace session {session_id} is based on {expected}, but the folder is now at {actual}; re-open the session before checkpointing"
            ),
            Self::UncommittedOverlay {
                session_id,
                changed_files,
            } => write!(
                f,
                "workspace session {session_id} has {changed_files} uncommitted overlay changes; checkpoint or discard it before close"
            ),
            Self::CorruptSession { path, message } => {
                write!(f, "could not read workspace session {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Store(error) => Some(error),
            Self::Sync(error) => Some(error),
            Self::Loom(error) => Some(error),
            Self::NoBaseRevision
            | Self::MissingBaseRevision(_)
            | Self::MissingSession(_)
            | Self::SessionAlreadyExists(_)
            | Self::ClosedSession(_)
            | Self::InvalidSessionId(_)
            | Self::InvalidPath { .. }
            | Self::MissingPath(_)
            | Self::NotAFile(_)
            | Self::ObjectUnavailable { .. }
            | Self::PathConflict { .. }
            | Self::PolicyIgnored { .. }
            | Self::PolicyBlocked { .. }
            | Self::UnsupportedOperation { .. }
            | Self::EmptyMessage(_)
            | Self::StaleBaseRevision { .. }
            | Self::UncommittedOverlay { .. }
            | Self::CorruptSession { .. } => None,
        }
    }
}

impl From<StoreError> for WorkspaceError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<SyncError> for WorkspaceError {
    fn from(error: SyncError) -> Self {
        Self::Sync(error)
    }
}

impl From<LoomError> for WorkspaceError {
    fn from(error: LoomError) -> Self {
        Self::Loom(error)
    }
}

pub type WorkspaceResult<T> = Result<T, WorkspaceError>;

fn versions_for_revision(
    store: &LocalStore,
    revision: &FolderRevision,
) -> WorkspaceResult<BTreeMap<PathBuf, FileVersion>> {
    let file_versions = store
        .file_versions()?
        .into_iter()
        .map(|version| (version.id().clone(), version))
        .collect::<BTreeMap<_, _>>();
    let mut versions = BTreeMap::new();
    for entry in revision.entries() {
        let version = file_versions.get(entry.file_version_id()).ok_or_else(|| {
            WorkspaceError::PathConflict {
                path: entry.path().to_path_buf(),
                reason: format!(
                    "revision {} references missing file version {}",
                    revision.id(),
                    entry.file_version_id()
                ),
            }
        })?;
        if version.path() != entry.path() {
            return Err(WorkspaceError::PathConflict {
                path: entry.path().to_path_buf(),
                reason: format!(
                    "revision entry points at file version for {}",
                    path_to_store_string(version.path())
                ),
            });
        }
        versions.insert(version.path().to_path_buf(), version.clone());
    }
    Ok(versions)
}

fn enforce_capture_policy(path: &Path, bytes: &[u8]) -> WorkspaceResult<()> {
    match evaluate_file_capture_policy(path, bytes) {
        FileCapturePolicyDecision::Capture => Ok(()),
        FileCapturePolicyDecision::Ignore { path, reason } => {
            Err(WorkspaceError::PolicyIgnored { path, reason })
        }
        FileCapturePolicyDecision::Block { path, reason } => {
            Err(WorkspaceError::PolicyBlocked { path, reason })
        }
    }
}

fn validate_overlay_write_target(
    base_versions: &BTreeMap<PathBuf, FileVersion>,
    overlay: &BTreeMap<PathBuf, OverlayFile>,
    path: &Path,
) -> WorkspaceResult<()> {
    if let Some(base) = base_versions.get(path) {
        if base.kind() != &FileKind::File {
            return Err(WorkspaceError::PathConflict {
                path: path.to_path_buf(),
                reason: "cannot write a file over a directory or unsupported entry".to_string(),
            });
        }
    }
    for ancestor in ancestors(path) {
        if let Some(base) = base_versions.get(&ancestor) {
            if base.kind() == &FileKind::File {
                return Err(WorkspaceError::PathConflict {
                    path: path.to_path_buf(),
                    reason: format!("parent {} is a file", path_to_store_string(&ancestor)),
                });
            }
        }
        if overlay.contains_key(&ancestor) {
            return Err(WorkspaceError::PathConflict {
                path: path.to_path_buf(),
                reason: format!(
                    "parent {} is an overlay file",
                    path_to_store_string(&ancestor)
                ),
            });
        }
    }
    for existing in base_versions.keys().chain(overlay.keys()) {
        if existing.starts_with(path) && existing != path {
            return Err(WorkspaceError::PathConflict {
                path: path.to_path_buf(),
                reason: "cannot write a file over a directory with children".to_string(),
            });
        }
    }
    Ok(())
}

fn ensure_parent_directories(
    versions: &mut BTreeMap<PathBuf, FileVersion>,
    path: &Path,
    captured_at: &str,
) -> WorkspaceResult<()> {
    for ancestor in ancestors(path) {
        if versions.contains_key(&ancestor) {
            continue;
        }
        let version = FileVersion::new(
            stable_file_version_id(&ancestor, FileKind::Directory, None, None)?,
            ancestor.clone(),
            FileKind::Directory,
            None,
            None,
            captured_at.to_string(),
        )?;
        versions.insert(ancestor, version);
    }
    Ok(())
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

fn workspace_id_for(store: &LocalStore) -> WorkspaceResult<WorkspaceId> {
    WorkspaceId::new(format!("workspace-{}", store.shared_folder().id().as_str()))
        .map_err(Into::into)
}

fn generated_session_id(shared_folder_id: &str) -> WorkspaceSessionId {
    let mut hasher = blake3::Hasher::new();
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    hasher.update(b"loom-agent-workspace-session-v1\n");
    hasher.update(shared_folder_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(process::id().to_string().as_bytes());
    hasher.update(b"\n");
    hasher.update(counter.to_string().as_bytes());
    hasher.update(b"\n");
    hasher.update(nanos.to_string().as_bytes());

    WorkspaceSessionId::new(format!("agent-session-b3-{}", hasher.finalize().to_hex()))
        .expect("generated session ids are non-empty")
}

fn validate_session_id(value: &str) -> WorkspaceResult<()> {
    let mut components = Path::new(value).components();
    let valid = matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(component)), None) if component == std::ffi::OsStr::new(value)
    );
    if value.trim().is_empty() || !valid {
        return Err(WorkspaceError::InvalidSessionId(value.to_string()));
    }
    Ok(())
}

fn normalize_relative_path(path: &Path, allow_empty: bool) -> WorkspaceResult<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(WorkspaceError::InvalidPath {
                    path: path.to_path_buf(),
                    reason: "paths must not contain '..'",
                });
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(WorkspaceError::InvalidPath {
                    path: path.to_path_buf(),
                    reason: "paths must be relative to the workspace",
                });
            }
        }
    }
    if normalized.as_os_str().is_empty() && !allow_empty {
        return Err(WorkspaceError::InvalidPath {
            path: path.to_path_buf(),
            reason: "path cannot be empty",
        });
    }
    Ok(normalized)
}

fn path_is_in_scope(path: &Path, scope: &Path) -> bool {
    scope.as_os_str().is_empty() || path == scope || path.starts_with(scope)
}

fn stable_file_version_id(
    relative_path: &Path,
    kind: FileKind,
    object_id: Option<&str>,
    size_bytes: Option<u64>,
) -> WorkspaceResult<FileVersionId> {
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

fn session_kind_to_store(kind: WorkspaceKind) -> &'static str {
    match kind {
        WorkspaceKind::AgentVirtual => "agent-virtual",
        WorkspaceKind::MaterializedSandbox => "materialized-sandbox",
        WorkspaceKind::OsFilesystemMount => "os-filesystem-mount",
    }
}

fn session_kind_from_store(value: &str) -> Option<WorkspaceKind> {
    match value {
        "agent-virtual" => Some(WorkspaceKind::AgentVirtual),
        "materialized-sandbox" => Some(WorkspaceKind::MaterializedSandbox),
        "os-filesystem-mount" => Some(WorkspaceKind::OsFilesystemMount),
        _ => None,
    }
}

fn session_state_to_store(state: WorkspaceSessionState) -> &'static str {
    match state {
        WorkspaceSessionState::Open => "open",
        WorkspaceSessionState::Closed => "closed",
        WorkspaceSessionState::Discarded => "discarded",
    }
}

fn session_state_from_store(value: &str) -> Option<WorkspaceSessionState> {
    match value {
        "open" => Some(WorkspaceSessionState::Open),
        "closed" => Some(WorkspaceSessionState::Closed),
        "discarded" => Some(WorkspaceSessionState::Discarded),
        _ => None,
    }
}

fn write_session_file(session_dir: &Path, session: &WorkspaceSession) -> WorkspaceResult<()> {
    create_dir_all(session_dir)?;
    let path = session_dir.join(SESSION_FILE);
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        encode_field(session.id().as_str()),
        encode_field(session.workspace_id().as_str()),
        encode_field(session.shared_folder_id().as_str()),
        encode_field(session.base_revision_id().as_str()),
        encode_field(session_kind_to_store(session.kind())),
        encode_field(session_state_to_store(session.state())),
        encode_field(session.created_at()),
    );
    fs::write(&path, line).map_err(|source| WorkspaceError::Io { path, source })
}

fn read_session_file(session_dir: &Path) -> WorkspaceResult<WorkspaceSession> {
    let path = session_dir.join(SESSION_FILE);
    let contents = fs::read_to_string(&path).map_err(|source| WorkspaceError::Io {
        path: path.clone(),
        source,
    })?;
    let line = contents
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| WorkspaceError::CorruptSession {
            path: path.clone(),
            message: "session file is empty".to_string(),
        })?;
    let fields = split_fields(&path, 1, line, 7)?;
    let kind = session_kind_from_store(&decode_field(&path, &fields[4])?).ok_or_else(|| {
        WorkspaceError::CorruptSession {
            path: path.clone(),
            message: "unknown workspace session kind".to_string(),
        }
    })?;
    let state = session_state_from_store(&decode_field(&path, &fields[5])?).ok_or_else(|| {
        WorkspaceError::CorruptSession {
            path: path.clone(),
            message: "unknown workspace session state".to_string(),
        }
    })?;
    WorkspaceSession::new(
        WorkspaceSessionId::new(decode_field(&path, &fields[0])?)?,
        WorkspaceId::new(decode_field(&path, &fields[1])?)?,
        loom_core::SharedFolderId::new(decode_field(&path, &fields[2])?)?,
        FolderRevisionId::new(decode_field(&path, &fields[3])?)?,
        kind,
        state,
        decode_field(&path, &fields[6])?,
    )
    .map_err(Into::into)
}

fn write_overlay_file(
    session_dir: &Path,
    overlay: &BTreeMap<PathBuf, OverlayFile>,
) -> WorkspaceResult<()> {
    create_dir_all(session_dir)?;
    let path = session_dir.join(OVERLAY_FILE);
    let mut contents = String::new();
    for file in overlay.values() {
        contents.push_str(&format!(
            "{}\t{}\t{}\n",
            encode_field(&path_to_store_string(&file.path)),
            encode_field(file.object_id.as_str()),
            encode_field(&file.size_bytes.to_string()),
        ));
    }
    fs::write(&path, contents).map_err(|source| WorkspaceError::Io { path, source })
}

fn read_overlay_file(session_dir: &Path) -> WorkspaceResult<BTreeMap<PathBuf, OverlayFile>> {
    let path = session_dir.join(OVERLAY_FILE);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(source) => return Err(WorkspaceError::Io { path, source }),
    };
    let mut overlay = BTreeMap::new();
    for (line_index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields = split_fields(&path, line_index + 1, line, 3)?;
        let overlay_path = store_string_to_path(&decode_field(&path, &fields[0])?);
        let overlay_path = normalize_relative_path(&overlay_path, false)?;
        let object_id = ObjectId::from_blake3_hex(decode_field(&path, &fields[1])?)?;
        let size_bytes = decode_field(&path, &fields[2])?
            .parse::<u64>()
            .map_err(|_| WorkspaceError::CorruptSession {
                path: path.clone(),
                message: format!("line {} has invalid overlay size", line_index + 1),
            })?;
        overlay.insert(
            overlay_path.clone(),
            OverlayFile {
                path: overlay_path,
                object_id,
                size_bytes,
            },
        );
    }
    Ok(overlay)
}

fn split_fields(
    path: &Path,
    line_number: usize,
    line: &str,
    expected: usize,
) -> WorkspaceResult<Vec<String>> {
    let fields = line
        .split('\t')
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if fields.len() != expected {
        return Err(WorkspaceError::CorruptSession {
            path: path.to_path_buf(),
            message: format!(
                "line {line_number} has {} fields, expected {expected}",
                fields.len()
            ),
        });
    }
    Ok(fields)
}

fn store_string_to_path(value: &str) -> PathBuf {
    if value == "." {
        return PathBuf::new();
    }
    value.split('/').collect()
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

fn decode_field(path: &Path, value: &str) -> WorkspaceResult<String> {
    let mut decoded = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(WorkspaceError::CorruptSession {
                    path: path.to_path_buf(),
                    message: "truncated percent escape".to_string(),
                });
            }
            let hex = &value[index + 1..index + 3];
            let byte = u8::from_str_radix(hex, 16).map_err(|_| WorkspaceError::CorruptSession {
                path: path.to_path_buf(),
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

fn create_dir_all(path: impl AsRef<Path>) -> WorkspaceResult<()> {
    let path = path.as_ref();
    fs::create_dir_all(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn current_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("unix:{}", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::{FolderScope, SharedFolder, SharedFolderId};
    use loom_sync::{
        import_pack_metadata_only_from_remote, sync_store_to_remote, LocalFilesystemRemote,
        LoomRemote,
    };
    use std::collections::BTreeSet;
    use std::fs;

    #[test]
    fn agent_session_reads_remote_file_lazily_writes_overlay_and_checkpoints() {
        let fixture = RemoteFixture::new();
        let remote = LocalFilesystemRemote::new(&fixture.remote_root);
        let adapter = AgentWorkspaceAdapter::with_remote(fixture.target_store.clone(), &remote);
        let mut request = WorkspaceSessionRequest::new();
        request.session_id = Some(WorkspaceSessionId::new("agent-lazy-read").expect("session id"));
        let mut session = adapter
            .create_session(request)
            .expect("agent session creates");

        assert!(!fixture.target_root.join("README.md").exists());
        assert!(!fixture
            .target_store
            .object_cache()
            .exists(&fixture.readme_object_id));

        let read = session
            .read_file(Path::new("README.md"))
            .expect("lazy read hydrates object");

        assert_eq!(read, b"hello from source\n");
        assert!(fixture
            .target_store
            .object_cache()
            .exists(&fixture.readme_object_id));
        assert!(!fixture.target_root.join("README.md").exists());

        session
            .write_file(Path::new("README.md"), b"changed in overlay\n")
            .expect("overlay write modifies readme");
        session
            .write_file(Path::new("src/new.rs"), b"fn new() {}\n")
            .expect("overlay write creates nested file");
        let diff = session.diff_overlay().expect("overlay diff");
        assert_eq!(store_paths(diff.modified()), vec!["README.md"]);
        assert_eq!(store_paths(diff.created()), vec!["src/new.rs"]);
        assert_eq!(
            session
                .read_file(Path::new("README.md"))
                .expect("overlay read wins"),
            b"changed in overlay\n"
        );

        let checkpoint = session
            .checkpoint_overlay("agent overlay checkpoint")
            .expect("overlay checkpoints");

        assert!(checkpoint.coalesced().created());
        assert_eq!(checkpoint.overlay_files(), 2);
        assert_eq!(
            checkpoint.checkpoint().message(),
            "agent overlay checkpoint"
        );
        assert_eq!(session.overlay_file_count(), 0);
        assert!(!fixture.target_root.join("README.md").exists());
        assert!(!fixture.target_root.join("src").join("new.rs").exists());

        let latest = fixture
            .target_store
            .latest_revision()
            .expect("latest revision reads")
            .expect("latest revision exists");
        let latest_paths = latest
            .entries()
            .iter()
            .map(|entry| path_to_store_string(entry.path()))
            .collect::<BTreeSet<_>>();
        assert!(latest_paths.contains("README.md"));
        assert!(latest_paths.contains("src"));
        assert!(latest_paths.contains("src/new.rs"));

        let close = session.close().expect("clean session closes");
        assert_eq!(close.state(), WorkspaceSessionState::Closed);
    }

    #[test]
    fn parallel_agent_sessions_keep_overlays_isolated() {
        let fixture = LocalFixture::new();
        let adapter = AgentWorkspaceAdapter::new(fixture.store.clone());
        let mut first = WorkspaceSessionRequest::new();
        first.session_id = Some(WorkspaceSessionId::new("agent-a").expect("session id"));
        let mut second = WorkspaceSessionRequest::new();
        second.session_id = Some(WorkspaceSessionId::new("agent-b").expect("session id"));
        let mut session_a = adapter.create_session(first).expect("session A creates");
        let mut session_b = adapter.create_session(second).expect("session B creates");

        session_a
            .write_file(Path::new("README.md"), b"from A\n")
            .expect("A writes overlay");

        assert_eq!(
            session_b
                .read_file(Path::new("README.md"))
                .expect("B reads base"),
            b"base\n"
        );

        session_b
            .write_file(Path::new("README.md"), b"from B\n")
            .expect("B writes overlay");

        assert_eq!(
            session_a
                .read_file(Path::new("README.md"))
                .expect("A keeps overlay"),
            b"from A\n"
        );
        assert_eq!(
            session_b
                .read_file(Path::new("README.md"))
                .expect("B keeps overlay"),
            b"from B\n"
        );
        assert_eq!(
            store_paths(session_a.diff_overlay().expect("A diff").modified()),
            vec!["README.md"]
        );
        assert_eq!(
            store_paths(session_b.diff_overlay().expect("B diff").modified()),
            vec!["README.md"]
        );

        let discarded_a = session_a.discard().expect("A discards");
        assert_eq!(discarded_a.discarded_overlay_files(), 1);
        assert_eq!(
            session_b
                .read_file(Path::new("README.md"))
                .expect("B still has overlay"),
            b"from B\n"
        );
        session_b.discard().expect("B discards");
    }

    #[test]
    fn virtual_adapter_reports_unsupported_operations_explicitly() {
        let fixture = LocalFixture::new();
        let adapter = AgentWorkspaceAdapter::new(fixture.store);
        let mut request = WorkspaceSessionRequest::new();
        request.session_id =
            Some(WorkspaceSessionId::new("agent-unsupported").expect("session id"));
        let session = adapter.create_session(request).expect("session creates");

        let dehydrate = session
            .dehydrate_path(Path::new("README.md"))
            .expect_err("dehydrate unsupported");
        let pin = session
            .pin_path(Path::new("README.md"))
            .expect_err("pin unsupported");

        assert!(matches!(
            dehydrate,
            WorkspaceError::UnsupportedOperation {
                operation: "dehydrate path",
                adapter: "agent virtual workspace"
            }
        ));
        assert!(matches!(
            pin,
            WorkspaceError::UnsupportedOperation {
                operation: "pin path",
                adapter: "agent virtual workspace"
            }
        ));
    }

    #[test]
    fn workspace_write_blocks_secrets_before_object_cache_write() {
        let fixture = LocalFixture::new();
        let adapter = AgentWorkspaceAdapter::new(fixture.store.clone());
        let mut request = WorkspaceSessionRequest::new();
        request.session_id = Some(WorkspaceSessionId::new("agent-secret").expect("session id"));
        let mut session = adapter.create_session(request).expect("session creates");
        let raw_secret = ["sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456"].concat();
        let bytes = format!("OPENAI_API_KEY={raw_secret}\n");
        let object_count_before = fixture.object_file_count();

        let error = session
            .write_file(Path::new("secrets.env"), bytes.as_bytes())
            .expect_err("secret write is blocked");

        assert!(matches!(error, WorkspaceError::PolicyBlocked { .. }));
        assert_eq!(session.overlay_file_count(), 0);
        assert_eq!(fixture.object_file_count(), object_count_before);
        assert!(!fixture.object_cache_contains(raw_secret.as_bytes()));
    }

    #[test]
    fn workspace_write_ignores_generated_folder_paths_before_object_cache_write() {
        let fixture = LocalFixture::new();
        let adapter = AgentWorkspaceAdapter::new(fixture.store.clone());
        let mut request = WorkspaceSessionRequest::new();
        request.session_id = Some(WorkspaceSessionId::new("agent-generated").expect("session id"));
        let mut session = adapter.create_session(request).expect("session creates");
        let object_count_before = fixture.object_file_count();

        let error = session
            .write_file(
                Path::new("node_modules/pkg/index.js"),
                b"module.exports = true;\n",
            )
            .expect_err("generated folder path is ignored");

        assert!(matches!(
            error,
            WorkspaceError::PolicyIgnored { path, .. } if path == Path::new("node_modules")
        ));
        assert_eq!(session.overlay_file_count(), 0);
        assert_eq!(fixture.object_file_count(), object_count_before);
    }

    #[test]
    fn checkpoint_revalidates_existing_overlay_policy() {
        let fixture = LocalFixture::new();
        let adapter = AgentWorkspaceAdapter::new(fixture.store.clone());
        let mut request = WorkspaceSessionRequest::new();
        request.session_id = Some(WorkspaceSessionId::new("agent-legacy").expect("session id"));
        let mut session = adapter.create_session(request).expect("session creates");
        let raw_secret = ["sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456"].concat();
        let bytes = format!("OPENAI_API_KEY={raw_secret}\n");
        let object = fixture
            .store
            .write_object_bytes(bytes.as_bytes())
            .expect("legacy overlay object writes");
        session.overlay.insert(
            PathBuf::from("secrets.env"),
            OverlayFile {
                path: PathBuf::from("secrets.env"),
                object_id: object.id().clone(),
                size_bytes: object.size_bytes(),
            },
        );

        let error = session
            .checkpoint_overlay("legacy overlay")
            .expect_err("checkpoint revalidates policy");

        assert!(matches!(error, WorkspaceError::PolicyBlocked { .. }));
    }

    #[test]
    fn session_ids_must_be_single_safe_path_components() {
        let fixture = LocalFixture::new();
        let adapter = AgentWorkspaceAdapter::new(fixture.store.clone());
        let mut good_request = WorkspaceSessionRequest::new();
        good_request.session_id = Some(WorkspaceSessionId::new("agent-safe").expect("session id"));
        let good_session = adapter
            .create_session(good_request)
            .expect("safe session creates");

        for value in [".", "..", "nested/session", "nested\\session", "a/."] {
            let mut request = WorkspaceSessionRequest::new();
            request.session_id = Some(WorkspaceSessionId::new(value).expect("session id"));
            let result = adapter.create_session(request);
            assert!(matches!(result, Err(WorkspaceError::InvalidSessionId(_))));
        }

        assert!(adapter
            .open_session(good_session.session().id())
            .expect("safe sibling session still opens")
            .discard()
            .is_ok());
    }

    #[test]
    fn checkpoint_refuses_stale_base_revision() {
        let fixture = LocalFixture::new();
        let adapter = AgentWorkspaceAdapter::new(fixture.store.clone());
        let mut first = WorkspaceSessionRequest::new();
        first.session_id = Some(WorkspaceSessionId::new("agent-first").expect("session id"));
        let mut second = WorkspaceSessionRequest::new();
        second.session_id = Some(WorkspaceSessionId::new("agent-second").expect("session id"));
        let mut session_a = adapter.create_session(first).expect("session A creates");
        let mut session_b = adapter.create_session(second).expect("session B creates");

        session_a
            .write_file(Path::new("a.txt"), b"a\n")
            .expect("A writes");
        session_a
            .checkpoint_overlay("A checkpoint")
            .expect("A checkpoints");
        session_b
            .write_file(Path::new("b.txt"), b"b\n")
            .expect("B writes");

        let error = session_b
            .checkpoint_overlay("B checkpoint")
            .expect_err("stale base refuses checkpoint");

        assert!(matches!(error, WorkspaceError::StaleBaseRevision { .. }));
    }

    struct LocalFixture {
        _dir: tempfile::TempDir,
        store: LocalStore,
    }

    impl LocalFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let folder = dir.path().join("shared");
            fs::create_dir_all(&folder).expect("folder creates");
            let store = LocalStore::open_or_init(&folder)
                .expect("store initializes")
                .into_store();
            let object = store
                .write_object_bytes(b"base\n")
                .expect("base object writes");
            let version = FileVersion::new(
                stable_file_version_id(
                    Path::new("README.md"),
                    FileKind::File,
                    Some(object.id().as_str()),
                    Some(object.size_bytes()),
                )
                .expect("file version id"),
                "README.md",
                FileKind::File,
                Some(object.id().clone()),
                Some(object.size_bytes()),
                "unix:1",
            )
            .expect("file version creates");
            store
                .coalesce_folder_revision(RevisionBoundary::LoomCommand, &[version])
                .expect("revision creates");

            Self { _dir: dir, store }
        }

        fn object_file_count(&self) -> usize {
            count_files(&self.store.store_root().join("objects"))
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

    struct RemoteFixture {
        _dir: tempfile::TempDir,
        remote_root: PathBuf,
        target_root: PathBuf,
        target_store: LocalStore,
        readme_object_id: ObjectId,
    }

    impl RemoteFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let source_root = dir.path().join("source");
            let target_root = dir.path().join("target");
            let remote_root = dir.path().join("remote");
            fs::create_dir_all(&source_root).expect("source creates");
            let source_store = LocalStore::open_or_init(&source_root)
                .expect("source store initializes")
                .into_store();
            let object = source_store
                .write_object_bytes(b"hello from source\n")
                .expect("source object writes");
            let version = FileVersion::new(
                stable_file_version_id(
                    Path::new("README.md"),
                    FileKind::File,
                    Some(object.id().as_str()),
                    Some(object.size_bytes()),
                )
                .expect("file version id"),
                "README.md",
                FileKind::File,
                Some(object.id().clone()),
                Some(object.size_bytes()),
                "unix:1",
            )
            .expect("file version creates");
            let revision = source_store
                .coalesce_folder_revision(RevisionBoundary::Sync, &[version])
                .expect("source revision")
                .revision()
                .clone();
            let remote = LocalFilesystemRemote::new(&remote_root);
            sync_store_to_remote(&source_store, &remote).expect("source syncs to remote");
            let pack = remote.get_pack(revision.id()).expect("pack reads");
            let target_store = LocalStore::init_clone(
                &target_root,
                pack.manifest.shared_folder_id.clone(),
                pack.manifest.display_name.clone(),
            )
            .expect("target store initializes");
            import_pack_metadata_only_from_remote(&target_store, &pack, &remote)
                .expect("metadata-only import");

            Self {
                _dir: dir,
                remote_root,
                target_root,
                target_store,
                readme_object_id: object.id().clone(),
            }
        }
    }

    fn store_paths(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| path_to_store_string(path))
            .collect()
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

    #[test]
    fn workspace_session_request_defaults_to_latest_revision() {
        let request = WorkspaceSessionRequest::new();
        assert!(request.session_id.is_none());
        assert!(request.base_revision_id.is_none());
        assert!(CRATE_ROLE.contains("workspace adapter"));
        let folder = SharedFolder::new(
            SharedFolderId::new("folder").expect("folder id"),
            "/tmp/folder",
            "folder",
            FolderScope::WholeFolder,
        )
        .expect("shared folder");
        assert_eq!(folder.display_name(), "folder");
    }
}
