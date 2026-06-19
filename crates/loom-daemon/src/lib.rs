//! Loom background sync daemon.
//!
//! The daemon treats filesystem notifications as hints. Each cycle captures the
//! whole shared folder through `loom-worktree`, coalesces at debounce
//! boundaries, then reconciles with the configured Loom remote cursor.

use loom_core::{FolderRevision, FolderRevisionId, RevisionBoundary};
use loom_store::{path_to_store_string, LocalStore, RemoteConfig, StoreError};
use loom_sync::{
    import_pack, sync_store_to_remote, LocalFilesystemRemote, LoomRemote, SyncError,
    DEFAULT_CURSOR_ID, DEFAULT_REMOTE_NAME, LOCAL_FILESYSTEM_REMOTE_KIND,
};
use loom_worktree::{
    diff_revision_to_capture, CaptureEngine, CaptureError, CaptureRequest, RestoreEngine,
};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_DEBOUNCE_MS: u64 = 500;
pub const DEFAULT_POLL_MS: u64 = 500;

const DAEMON_DIR: &str = "daemon";
const STATUS_FILE: &str = "status.tsv";
const STOP_FILE: &str = "stop.request";
const LOG_FILE: &str = "daemon.log";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonLoopOptions {
    pub folder: PathBuf,
    pub debounce_ms: u64,
    pub poll_ms: u64,
    pub max_cycles: Option<usize>,
}

impl DaemonLoopOptions {
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            poll_ms: DEFAULT_POLL_MS,
            max_cycles: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStartOptions {
    pub folder: PathBuf,
    pub debounce_ms: u64,
    pub poll_ms: u64,
}

impl DaemonStartOptions {
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            poll_ms: DEFAULT_POLL_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStatus {
    pub folder: PathBuf,
    pub state: String,
    pub pid: Option<u32>,
    pub remote_name: Option<String>,
    pub remote_location: Option<String>,
    pub last_local_revision: Option<String>,
    pub last_remote_revision: Option<String>,
    pub cycles: usize,
    pub last_error: Option<String>,
    pub updated_at: String,
    pub stop_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStartReport {
    pub folder: PathBuf,
    pub pid: u32,
    pub status_path: PathBuf,
    pub log_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStopReport {
    pub folder: PathBuf,
    pub stop_path: PathBuf,
    pub status: DaemonStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    pub action: ReconcileAction,
    pub local_revision_id: FolderRevisionId,
    pub remote_revision_id: Option<FolderRevisionId>,
    pub uploaded_objects: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileAction {
    Unchanged,
    Pushed,
    Pulled,
}

#[derive(Debug)]
pub enum DaemonError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Store(StoreError),
    Sync(SyncError),
    Capture(CaptureError),
    Notify(notify::Error),
    NoRemote,
    UnsupportedRemote {
        name: String,
        kind: String,
    },
    NoLocalRevision,
    DivergentState {
        remote_revision_id: FolderRevisionId,
        local_revision_id: FolderRevisionId,
    },
    BlockedSource {
        path: PathBuf,
        reason: String,
    },
    DeferredSource {
        path: PathBuf,
        reason: String,
    },
    Spawn {
        source: io::Error,
    },
    InvalidStatus {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not access {}: {source}", path.display()),
            Self::Store(error) => write!(f, "{error}"),
            Self::Sync(error) => write!(f, "{error}"),
            Self::Capture(error) => write!(f, "{error}"),
            Self::Notify(error) => write!(f, "filesystem watcher failed: {error}"),
            Self::NoRemote => {
                write!(f, "no Loom remote configured; run 'loom remote add local <PATH>' first")
            }
            Self::UnsupportedRemote { name, kind } => {
                write!(f, "remote {name} uses unsupported kind {kind}")
            }
            Self::NoLocalRevision => {
                write!(f, "no local folder revisions yet; run 'loom status' first")
            }
            Self::DivergentState {
                remote_revision_id,
                local_revision_id,
            } => write!(
                f,
                "background sync refused because remote revision {remote_revision_id} and local revision {local_revision_id} diverged"
            ),
            Self::BlockedSource { path, reason } => write!(
                f,
                "background sync refused because {} is secret-blocked: {reason}",
                path_to_store_string(path)
            ),
            Self::DeferredSource { path, reason } => write!(
                f,
                "background sync refused because {} is deferred: {reason}",
                path_to_store_string(path)
            ),
            Self::Spawn { source } => write!(f, "could not start Loom background sync: {source}"),
            Self::InvalidStatus { path, message } => {
                write!(f, "could not read daemon status {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for DaemonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Store(error) => Some(error),
            Self::Sync(error) => Some(error),
            Self::Capture(error) => Some(error),
            Self::Notify(error) => Some(error),
            Self::Spawn { source } => Some(source),
            Self::NoRemote
            | Self::UnsupportedRemote { .. }
            | Self::NoLocalRevision
            | Self::DivergentState { .. }
            | Self::BlockedSource { .. }
            | Self::DeferredSource { .. }
            | Self::InvalidStatus { .. } => None,
        }
    }
}

impl From<StoreError> for DaemonError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<SyncError> for DaemonError {
    fn from(error: SyncError) -> Self {
        Self::Sync(error)
    }
}

impl From<CaptureError> for DaemonError {
    fn from(error: CaptureError) -> Self {
        Self::Capture(error)
    }
}

impl From<notify::Error> for DaemonError {
    fn from(error: notify::Error) -> Self {
        Self::Notify(error)
    }
}

pub type DaemonResult<T> = Result<T, DaemonError>;

pub fn start_background(options: &DaemonStartOptions) -> DaemonResult<DaemonStartReport> {
    let store = LocalStore::open(&options.folder)?;
    let paths = DaemonPaths::new(&store);
    paths.ensure()?;
    remove_file_if_exists(&paths.stop_path)?;

    fs::write(
        &paths.log_path,
        "loom background sync writes durable status to status.tsv\n",
    )
    .map_err(|source| DaemonError::Io {
        path: paths.log_path.clone(),
        source,
    })?;

    let pid = spawn_run_loop_process(&store, options)?;

    let status = status_for_state(&store, "starting", Some(pid), None, 0, None)?;
    write_status(&paths.status_path, &status)?;

    Ok(DaemonStartReport {
        folder: store.folder_root().to_path_buf(),
        pid,
        status_path: paths.status_path,
        log_path: paths.log_path,
    })
}

pub fn request_stop(folder: impl AsRef<Path>) -> DaemonResult<DaemonStopReport> {
    let store = LocalStore::open(folder)?;
    let paths = DaemonPaths::new(&store);
    paths.ensure()?;
    fs::write(&paths.stop_path, format!("requested_at\t{}\n", timestamp())).map_err(|source| {
        DaemonError::Io {
            path: paths.stop_path.clone(),
            source,
        }
    })?;

    let status = wait_for_stopped(&store, Duration::from_secs(5))?;
    Ok(DaemonStopReport {
        folder: store.folder_root().to_path_buf(),
        stop_path: paths.stop_path,
        status,
    })
}

pub fn read_status(folder: impl AsRef<Path>) -> DaemonResult<DaemonStatus> {
    let store = LocalStore::open(folder)?;
    let paths = DaemonPaths::new(&store);
    match parse_status(&paths.status_path)? {
        Some(mut status) => {
            status.stop_requested = paths.stop_path.is_file();
            Ok(status)
        }
        None => status_for_state(&store, "stopped", None, None, 0, None),
    }
}

pub fn run_loop(options: &DaemonLoopOptions) -> DaemonResult<()> {
    let store = LocalStore::open(&options.folder)?;
    let paths = DaemonPaths::new(&store);
    paths.ensure()?;
    remove_file_if_exists(&paths.stop_path)?;
    let remote_config = configured_remote(&store)?;
    let remote = remote_from_config(&remote_config)?;

    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            let _ = tx.send(result);
        },
        Config::default(),
    )?;
    watcher.watch(store.folder_root(), RecursiveMode::Recursive)?;

    let start = Instant::now();
    let mut planner = DebouncePlanner::new(options.debounce_ms);
    let mut cycles = 0usize;
    let mut last_report = None;
    cycles += 1;
    match reconcile_once(&store, &remote) {
        Ok(report) => {
            last_report = Some(report);
            write_status(
                &paths.status_path,
                &status_for_state(
                    &store,
                    "running",
                    Some(std::process::id()),
                    last_report.as_ref(),
                    cycles,
                    None,
                )?,
            )?;
        }
        Err(error) => {
            write_status(
                &paths.status_path,
                &status_for_state(
                    &store,
                    "blocked",
                    Some(std::process::id()),
                    last_report.as_ref(),
                    cycles,
                    Some(error.to_string()),
                )?,
            )?;
            if options.max_cycles.is_some() {
                return Err(error);
            }
        }
    }

    loop {
        if options.max_cycles.is_some_and(|max| cycles >= max) {
            break;
        }
        if paths.stop_path.is_file() {
            break;
        }

        let now_ms = elapsed_ms(start);
        if planner.take_due_batch(now_ms).is_some() {
            cycles += 1;
            match reconcile_once(&store, &remote) {
                Ok(report) => {
                    last_report = Some(report);
                    write_status(
                        &paths.status_path,
                        &status_for_state(
                            &store,
                            "running",
                            Some(std::process::id()),
                            last_report.as_ref(),
                            cycles,
                            None,
                        )?,
                    )?;
                }
                Err(error) => {
                    write_status(
                        &paths.status_path,
                        &status_for_state(
                            &store,
                            "blocked",
                            Some(std::process::id()),
                            last_report.as_ref(),
                            cycles,
                            Some(error.to_string()),
                        )?,
                    )?;
                    if options.max_cycles.is_some() {
                        return Err(error);
                    }
                }
            }
        }

        match rx.recv_timeout(receive_timeout(&planner, options.poll_ms, start)) {
            Ok(Ok(_event)) => {
                planner.record_event(elapsed_ms(start));
            }
            Ok(Err(error)) => return Err(error.into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !planner.has_pending() {
                    planner.record_event(elapsed_ms(start));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(DaemonError::Notify(notify::Error::generic(
                    "filesystem watcher channel disconnected",
                )));
            }
        }
    }

    write_status(
        &paths.status_path,
        &status_for_state(
            &store,
            "stopped",
            Some(std::process::id()),
            last_report.as_ref(),
            cycles,
            None,
        )?,
    )?;
    remove_file_if_exists(&paths.stop_path)?;
    Ok(())
}

pub fn reconcile_once(
    store: &LocalStore,
    remote: &LocalFilesystemRemote,
) -> DaemonResult<ReconcileReport> {
    let capture = CaptureEngine::new(store.object_cache()).capture(&CaptureRequest::new(
        store.shared_folder().clone(),
        RevisionBoundary::DebounceWindow,
    ))?;
    if let Some(notice) = capture.blocked().first() {
        return Err(DaemonError::BlockedSource {
            path: notice.relative_path().to_path_buf(),
            reason: notice.reason().to_string(),
        });
    }
    if let Some(notice) = capture.deferred().first() {
        return Err(DaemonError::DeferredSource {
            path: notice.relative_path().to_path_buf(),
            reason: notice.reason().to_string(),
        });
    }

    let local = store
        .coalesce_folder_revision(RevisionBoundary::DebounceWindow, capture.file_versions())?
        .revision()
        .clone();
    let remote_revision_id = remote.get_cursor(DEFAULT_CURSOR_ID)?;

    let Some(remote_revision_id) = remote_revision_id else {
        let report = sync_store_to_remote(store, remote)?;
        return Ok(ReconcileReport {
            action: ReconcileAction::Pushed,
            local_revision_id: report.latest_revision_id,
            remote_revision_id: report.previous_remote_revision_id,
            uploaded_objects: report.uploaded_objects,
        });
    };

    if &remote_revision_id == local.id() {
        return Ok(ReconcileReport {
            action: ReconcileAction::Unchanged,
            local_revision_id: local.id().clone(),
            remote_revision_id: Some(remote_revision_id),
            uploaded_objects: 0,
        });
    }

    if revision_is_ancestor(store, &remote_revision_id, local.id())? {
        let report = sync_store_to_remote(store, remote)?;
        return Ok(ReconcileReport {
            action: ReconcileAction::Pushed,
            local_revision_id: report.latest_revision_id,
            remote_revision_id: report.previous_remote_revision_id,
            uploaded_objects: report.uploaded_objects,
        });
    }

    let pack = remote.get_pack(&remote_revision_id)?;
    if pack_revision_is_ancestor(&pack.revisions, local.id(), &remote_revision_id) {
        let current = CaptureEngine::new(store.object_cache()).capture(&CaptureRequest::new(
            store.shared_folder().clone(),
            RevisionBoundary::Sync,
        ))?;
        let diff = diff_revision_to_capture(&local, &current)?;
        if diff.has_changes() {
            return Err(DaemonError::DivergentState {
                remote_revision_id,
                local_revision_id: local.id().clone(),
            });
        }

        import_pack(store, &pack)?;
        let revision = store.revision_by_id(&remote_revision_id)?.ok_or_else(|| {
            DaemonError::Sync(SyncError::MissingRevision(remote_revision_id.clone()))
        })?;
        RestoreEngine::new(store).restore(&revision, &current)?;
        let restored = CaptureEngine::new(store.object_cache()).capture(&CaptureRequest::new(
            store.shared_folder().clone(),
            RevisionBoundary::Sync,
        ))?;
        let coalesced =
            store.coalesce_folder_revision(RevisionBoundary::Sync, restored.file_versions())?;
        return Ok(ReconcileReport {
            action: ReconcileAction::Pulled,
            local_revision_id: coalesced.revision().id().clone(),
            remote_revision_id: Some(remote_revision_id),
            uploaded_objects: 0,
        });
    }

    Err(DaemonError::DivergentState {
        remote_revision_id,
        local_revision_id: local.id().clone(),
    })
}

pub fn configured_remote(store: &LocalStore) -> DaemonResult<RemoteConfig> {
    store
        .remote(DEFAULT_REMOTE_NAME)?
        .or_else(|| store.remotes().ok().and_then(|mut remotes| remotes.pop()))
        .ok_or(DaemonError::NoRemote)
}

pub fn remote_from_config(config: &RemoteConfig) -> DaemonResult<LocalFilesystemRemote> {
    if config.kind() != LOCAL_FILESYSTEM_REMOTE_KIND {
        return Err(DaemonError::UnsupportedRemote {
            name: config.name().to_string(),
            kind: config.kind().to_string(),
        });
    }

    Ok(LocalFilesystemRemote::new(config.location()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebouncePlanner {
    debounce_ms: u64,
    pending_events: usize,
    next_scan_at_ms: Option<u64>,
}

impl DebouncePlanner {
    pub fn new(debounce_ms: u64) -> Self {
        Self {
            debounce_ms,
            pending_events: 0,
            next_scan_at_ms: None,
        }
    }

    pub fn record_event(&mut self, now_ms: u64) -> usize {
        self.pending_events += 1;
        self.next_scan_at_ms = Some(now_ms.saturating_add(self.debounce_ms));
        self.pending_events
    }

    pub fn take_due_batch(&mut self, now_ms: u64) -> Option<usize> {
        if !self
            .next_scan_at_ms
            .is_some_and(|next_scan_at_ms| next_scan_at_ms <= now_ms)
        {
            return None;
        }

        let batch = self.pending_events;
        self.pending_events = 0;
        self.next_scan_at_ms = None;
        Some(batch)
    }

    fn next_scan_at_ms(&self) -> Option<u64> {
        self.next_scan_at_ms
    }

    fn has_pending(&self) -> bool {
        self.pending_events > 0
    }
}

fn revision_is_ancestor(
    store: &LocalStore,
    possible_ancestor: &FolderRevisionId,
    revision_id: &FolderRevisionId,
) -> DaemonResult<bool> {
    let revisions = store
        .revisions()?
        .into_iter()
        .map(|revision| (revision.id().clone(), revision))
        .collect::<BTreeMap<_, _>>();
    Ok(revision_is_ancestor_in_map(
        &revisions,
        possible_ancestor,
        revision_id,
    ))
}

fn pack_revision_is_ancestor(
    revisions: &[FolderRevision],
    possible_ancestor: &FolderRevisionId,
    revision_id: &FolderRevisionId,
) -> bool {
    let revisions = revisions
        .iter()
        .cloned()
        .map(|revision| (revision.id().clone(), revision))
        .collect::<BTreeMap<_, _>>();
    revision_is_ancestor_in_map(&revisions, possible_ancestor, revision_id)
}

fn revision_is_ancestor_in_map(
    revisions: &BTreeMap<FolderRevisionId, FolderRevision>,
    possible_ancestor: &FolderRevisionId,
    revision_id: &FolderRevisionId,
) -> bool {
    let mut current = Some(revision_id.clone());
    while let Some(current_id) = current {
        if &current_id == possible_ancestor {
            return true;
        }
        current = revisions
            .get(&current_id)
            .and_then(FolderRevision::parent_id)
            .cloned();
    }
    false
}

fn configured_remote_status(store: &LocalStore) -> (Option<String>, Option<String>) {
    configured_remote(store)
        .map(|remote| {
            (
                Some(remote.name().to_string()),
                Some(remote.location().to_string()),
            )
        })
        .unwrap_or((None, None))
}

fn status_for_state(
    store: &LocalStore,
    state: &str,
    pid: Option<u32>,
    report: Option<&ReconcileReport>,
    cycles: usize,
    last_error: Option<String>,
) -> DaemonResult<DaemonStatus> {
    let (remote_name, remote_location) = configured_remote_status(store);
    let latest = store
        .latest_revision()?
        .map(|revision| revision.id().to_string());

    Ok(DaemonStatus {
        folder: store.folder_root().to_path_buf(),
        state: state.to_string(),
        pid,
        remote_name,
        remote_location,
        last_local_revision: report
            .map(|report| report.local_revision_id.to_string())
            .or(latest),
        last_remote_revision: report
            .and_then(|report| report.remote_revision_id.as_ref())
            .map(ToString::to_string),
        cycles,
        last_error,
        updated_at: timestamp(),
        stop_requested: DaemonPaths::new(store).stop_path.is_file(),
    })
}

fn wait_for_stopped(store: &LocalStore, timeout: Duration) -> DaemonResult<DaemonStatus> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let status = read_status(store.folder_root())?;
        if status.state == "stopped" {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    read_status(store.folder_root())
}

fn write_status(path: &Path, status: &DaemonStatus) -> DaemonResult<()> {
    let parent = path.parent().expect("daemon status has a parent");
    fs::create_dir_all(parent).map_err(|source| DaemonError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let temp_path = path.with_extension("tmp");
    let mut file = File::create(&temp_path).map_err(|source| DaemonError::Io {
        path: temp_path.clone(),
        source,
    })?;
    for (key, value) in [
        ("version", "1".to_string()),
        ("folder", status.folder.display().to_string()),
        ("state", status.state.clone()),
        (
            "pid",
            status
                .pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "remote_name",
            status
                .remote_name
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "remote_location",
            status
                .remote_location
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "last_local_revision",
            status
                .last_local_revision
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "last_remote_revision",
            status
                .last_remote_revision
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("cycles", status.cycles.to_string()),
        (
            "last_error",
            status.last_error.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("updated_at", status.updated_at.clone()),
        ("stop_requested", status.stop_requested.to_string()),
    ] {
        file.write_all(format!("{}\t{}\n", key, encode_field(&value)).as_bytes())
            .map_err(|source| DaemonError::Io {
                path: temp_path.clone(),
                source,
            })?;
    }
    file.flush().map_err(|source| DaemonError::Io {
        path: temp_path.clone(),
        source,
    })?;
    drop(file);
    fs::rename(&temp_path, path).map_err(|source| DaemonError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn parse_status(path: &Path) -> DaemonResult<Option<DaemonStatus>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DaemonError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let mut values = BTreeMap::new();
    for (line_index, line) in contents.lines().enumerate() {
        let Some((key, value)) = line.split_once('\t') else {
            return Err(DaemonError::InvalidStatus {
                path: path.to_path_buf(),
                message: format!("line {} is not a key/value row", line_index + 1),
            });
        };
        values.insert(key.to_string(), decode_field(value)?);
    }

    let folder = required_status_value(path, &values, "folder")?;
    Ok(Some(DaemonStatus {
        folder: PathBuf::from(folder),
        state: required_status_value(path, &values, "state")?,
        pid: optional_status_value(&values, "pid").and_then(|value| value.parse().ok()),
        remote_name: optional_status_value(&values, "remote_name"),
        remote_location: optional_status_value(&values, "remote_location"),
        last_local_revision: optional_status_value(&values, "last_local_revision"),
        last_remote_revision: optional_status_value(&values, "last_remote_revision"),
        cycles: optional_status_value(&values, "cycles")
            .and_then(|value| value.parse().ok())
            .unwrap_or_default(),
        last_error: optional_status_value(&values, "last_error"),
        updated_at: required_status_value(path, &values, "updated_at")?,
        stop_requested: optional_status_value(&values, "stop_requested")
            .map(|value| value == "true")
            .unwrap_or(false),
    }))
}

fn required_status_value(
    path: &Path,
    values: &BTreeMap<String, String>,
    key: &'static str,
) -> DaemonResult<String> {
    values
        .get(key)
        .cloned()
        .ok_or_else(|| DaemonError::InvalidStatus {
            path: path.to_path_buf(),
            message: format!("missing {key}"),
        })
}

fn optional_status_value(values: &BTreeMap<String, String>, key: &'static str) -> Option<String> {
    values
        .get(key)
        .filter(|value| value.as_str() != "-")
        .cloned()
}

fn receive_timeout(planner: &DebouncePlanner, poll_ms: u64, start: Instant) -> Duration {
    let poll = Duration::from_millis(poll_ms.max(1));
    let Some(deadline) = planner.next_scan_at_ms() else {
        return poll;
    };
    let now_ms = elapsed_ms(start);
    poll.min(Duration::from_millis(deadline.saturating_sub(now_ms)))
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
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

fn decode_field(value: &str) -> DaemonResult<String> {
    let mut decoded = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(DaemonError::InvalidStatus {
                    path: PathBuf::from("<field>"),
                    message: "truncated percent escape".to_string(),
                });
            }
            let hex = &value[index + 1..index + 3];
            let byte = u8::from_str_radix(hex, 16).map_err(|_| DaemonError::InvalidStatus {
                path: PathBuf::from("<field>"),
                message: "invalid percent escape".to_string(),
            })?;
            decoded.push(byte as char);
            index += 3;
        } else {
            let character = value[index..]
                .chars()
                .next()
                .expect("index is inside string");
            decoded.push(character);
            index += character.len_utf8();
        }
    }
    Ok(decoded)
}

fn remove_file_if_exists(path: &Path) -> DaemonResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DaemonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(windows)]
fn spawn_run_loop_process(store: &LocalStore, options: &DaemonStartOptions) -> DaemonResult<u32> {
    let exe = std::env::current_exe().map_err(|source| DaemonError::Spawn { source })?;
    let args = [
        "sync".to_string(),
        "run-loop".to_string(),
        store.folder_root().display().to_string(),
        "--debounce-ms".to_string(),
        options.debounce_ms.to_string(),
        "--poll-ms".to_string(),
        options.poll_ms.to_string(),
    ];
    let argument_list = args
        .iter()
        .map(|arg| powershell_quote(arg))
        .collect::<Vec<_>>()
        .join(", ");
    let script = format!(
        "$p = Start-Process -FilePath {} -ArgumentList @({argument_list}) -PassThru -WindowStyle Hidden; [Console]::Out.Write($p.Id)",
        powershell_quote(&exe.display().to_string())
    );
    let output = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-Command")
        .arg(script)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| DaemonError::Spawn { source })?;
    if !output.status.success() {
        return Err(DaemonError::Spawn {
            source: io::Error::new(
                io::ErrorKind::Other,
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ),
        });
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .map_err(|error| DaemonError::Spawn {
            source: io::Error::new(io::ErrorKind::Other, error),
        })
}

#[cfg(not(windows))]
fn spawn_run_loop_process(store: &LocalStore, options: &DaemonStartOptions) -> DaemonResult<u32> {
    let child =
        Command::new(std::env::current_exe().map_err(|source| DaemonError::Spawn { source })?)
            .arg("sync")
            .arg("run-loop")
            .arg(store.folder_root())
            .arg("--debounce-ms")
            .arg(options.debounce_ms.to_string())
            .arg("--poll-ms")
            .arg(options.poll_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| DaemonError::Spawn { source })?;
    Ok(child.id())
}

#[cfg(windows)]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[derive(Debug, Clone)]
struct DaemonPaths {
    root: PathBuf,
    status_path: PathBuf,
    stop_path: PathBuf,
    log_path: PathBuf,
}

impl DaemonPaths {
    fn new(store: &LocalStore) -> Self {
        let root = store.store_root().join(DAEMON_DIR);
        Self {
            status_path: root.join(STATUS_FILE),
            stop_path: root.join(STOP_FILE),
            log_path: root.join(LOG_FILE),
            root,
        }
    }

    fn ensure(&self) -> DaemonResult<()> {
        fs::create_dir_all(&self.root).map_err(|source| DaemonError::Io {
            path: self.root.clone(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_store::RemoteConfig;
    use std::fs;

    #[test]
    fn debounce_batches_until_quiet_window_elapses() {
        let mut planner = DebouncePlanner::new(100);

        assert_eq!(planner.record_event(0), 1);
        assert_eq!(planner.record_event(50), 2);
        assert_eq!(planner.take_due_batch(149), None);
        assert_eq!(planner.take_due_batch(150), Some(2));
        assert_eq!(planner.take_due_batch(151), None);
    }

    #[test]
    fn status_file_round_trips() {
        let dir = tempfile::tempdir().expect("temp dir");
        let folder = dir.path().join("source");
        fs::create_dir_all(&folder).expect("folder creates");
        let store = LocalStore::open_or_init(&folder)
            .expect("store initializes")
            .into_store();
        let path = store.store_root().join("daemon").join("status.tsv");
        let status = status_for_state(&store, "running", Some(7), None, 3, Some("ok".to_string()))
            .expect("status creates");

        write_status(&path, &status).expect("status writes");
        let parsed = parse_status(&path)
            .expect("status parses")
            .expect("status exists");

        assert_eq!(parsed.state, "running");
        assert_eq!(parsed.pid, Some(7));
        assert_eq!(parsed.cycles, 3);
        assert_eq!(parsed.last_error.as_deref(), Some("ok"));
    }

    #[test]
    fn reconcile_pulls_remote_descendant_into_materialized_folder() {
        let fixture = RemoteFixture::new();
        let source = &fixture.source;
        let target = &fixture.target;
        let source_store = &fixture.source_store;
        let target_store = &fixture.target_store;
        let remote = &fixture.remote;

        fs::write(source.join("README.md"), "two\n").expect("source edits");
        capture_and_coalesce(source_store);
        sync_store_to_remote(source_store, remote).expect("second sync");

        let report = reconcile_once(&target_store, &remote).expect("target pulls");

        assert_eq!(report.action, ReconcileAction::Pulled);
        assert_eq!(
            fs::read_to_string(target.join("README.md")).expect("target reads"),
            "two\n"
        );
    }

    #[test]
    fn reconcile_refuses_divergent_target_edit() {
        let fixture = RemoteFixture::new();
        let source = &fixture.source;
        let target = &fixture.target;
        let source_store = &fixture.source_store;
        let target_store = &fixture.target_store;
        let remote = &fixture.remote;

        fs::write(source.join("README.md"), "remote\n").expect("source edits");
        capture_and_coalesce(source_store);
        sync_store_to_remote(source_store, remote).expect("source syncs");
        fs::write(target.join("README.md"), "local\n").expect("target edits");

        let error = reconcile_once(target_store, remote).expect_err("target refuses divergence");

        assert!(matches!(error, DaemonError::DivergentState { .. }));
        assert_eq!(
            fs::read_to_string(target.join("README.md")).expect("target reads"),
            "local\n"
        );
    }

    struct RemoteFixture {
        _dir: tempfile::TempDir,
        source: PathBuf,
        target: PathBuf,
        source_store: LocalStore,
        target_store: LocalStore,
        remote: LocalFilesystemRemote,
    }

    impl RemoteFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let source = dir.path().join("source");
            let target = dir.path().join("target");
            let remote_path = dir.path().join("remote");
            fs::create_dir_all(&source).expect("source creates");
            fs::create_dir_all(&target).expect("target creates");
            fs::write(source.join("README.md"), "one\n").expect("source writes");

            let source_store = LocalStore::open_or_init(&source)
                .expect("source store")
                .into_store();
            source_store
                .upsert_remote(
                    RemoteConfig::new(
                        "local",
                        LOCAL_FILESYSTEM_REMOTE_KIND,
                        path_string(&remote_path),
                    )
                    .expect("remote config"),
                )
                .expect("source remote writes");
            capture_and_coalesce(&source_store);
            let remote = LocalFilesystemRemote::new(&remote_path);
            sync_store_to_remote(&source_store, &remote).expect("initial sync");

            let remote_revision_id = remote
                .get_cursor(DEFAULT_CURSOR_ID)
                .expect("cursor reads")
                .expect("cursor exists");
            let pack = remote.get_pack(&remote_revision_id).expect("pack reads");
            let target_store = LocalStore::init_clone(
                &target,
                pack.manifest.shared_folder_id.clone(),
                pack.manifest.display_name.clone(),
            )
            .expect("target clone store");
            import_pack(&target_store, &pack).expect("pack imports");
            let current = CaptureEngine::new(target_store.object_cache())
                .capture(&CaptureRequest::new(
                    target_store.shared_folder().clone(),
                    RevisionBoundary::Restore,
                ))
                .expect("target captures");
            let revision = target_store
                .revision_by_id(&remote_revision_id)
                .expect("revision reads")
                .expect("revision exists");
            RestoreEngine::new(&target_store)
                .restore(&revision, &current)
                .expect("restore applies");
            let restored = CaptureEngine::new(target_store.object_cache())
                .capture(&CaptureRequest::new(
                    target_store.shared_folder().clone(),
                    RevisionBoundary::Sync,
                ))
                .expect("target captures");
            target_store
                .coalesce_folder_revision(RevisionBoundary::Sync, restored.file_versions())
                .expect("coalesces");

            Self {
                _dir: dir,
                source,
                target,
                source_store,
                target_store,
                remote,
            }
        }
    }

    fn capture_and_coalesce(store: &LocalStore) {
        let capture = CaptureEngine::new(store.object_cache())
            .capture(&CaptureRequest::new(
                store.shared_folder().clone(),
                RevisionBoundary::Sync,
            ))
            .expect("capture");
        store
            .coalesce_folder_revision(RevisionBoundary::Sync, capture.file_versions())
            .expect("coalesce");
    }

    fn path_string(path: &Path) -> String {
        path.to_str().expect("test path is UTF-8").to_string()
    }
}
