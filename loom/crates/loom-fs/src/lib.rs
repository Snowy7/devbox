//! Loom native filesystem adapter boundary.
//!
//! This crate describes OS filesystem mount adapters over Loom folder
//! revisions, object cache metadata, and sparse-folder worktree primitives. It
//! intentionally stays outside Loom core. Native Windows, macOS, and Linux
//! adapters are fail-closed alpha stubs until real host integrations exist. The
//! local-dev adapter is a deterministic metadata simulation for tests and CLI
//! wiring; it does not create placeholder files or hydrate bytes on open.

use loom_core::{FolderRevision, RevisionBoundary};
use loom_store::{path_to_store_string, LocalStore, StoreError};
use loom_worktree::{
    diff_revision_to_capture, CaptureEngine, CaptureError, CaptureRequest, WorktreeDiff,
};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CRATE_ROLE: &str =
    "native OS filesystem adapter alpha boundary over Loom folder revisions and cache metadata";

const FS_DIR: &str = "fs";
const MOUNTS_FILE: &str = "mounts.tsv";
const SIMULATED_PROJECTION_STATE: &str = "simulated-metadata-only";

static MOUNT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Windows,
    MacOs,
    Linux,
    Other,
}

impl HostPlatform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::MacOs => "macos",
            Self::Linux => "linux",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsAdapterKind {
    LocalDev,
    WindowsCloudFiles,
    MacOsFileProvider,
    LinuxFuse,
}

impl FsAdapterKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::LocalDev => "local-dev",
            Self::WindowsCloudFiles => "windows-cloud-files",
            Self::MacOsFileProvider => "macos-file-provider",
            Self::LinuxFuse => "linux-fuse",
        }
    }

    pub fn direction(self) -> &'static str {
        match self {
            Self::LocalDev => "local metadata simulation",
            Self::WindowsCloudFiles => "Windows Cloud Files or Projected File System",
            Self::MacOsFileProvider => {
                "macOS File Provider, with FUSE as a possible alpha fallback"
            }
            Self::LinuxFuse => "Linux FUSE",
        }
    }

    fn from_label(value: &str) -> Option<Self> {
        match value {
            "local-dev" => Some(Self::LocalDev),
            "windows-cloud-files" => Some(Self::WindowsCloudFiles),
            "macos-file-provider" => Some(Self::MacOsFileProvider),
            "linux-fuse" => Some(Self::LinuxFuse),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsCapabilities {
    adapter_kind: FsAdapterKind,
    host_platform: HostPlatform,
    host_api: String,
    host_api_detected: bool,
    can_mount: bool,
    real_os_integration: bool,
    supports_hydrate_on_open: bool,
    message: String,
    details: Vec<String>,
}

impl FsCapabilities {
    pub fn new(
        adapter_kind: FsAdapterKind,
        host_platform: HostPlatform,
        host_api: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            adapter_kind,
            host_platform,
            host_api: host_api.into(),
            host_api_detected: false,
            can_mount: false,
            real_os_integration: false,
            supports_hydrate_on_open: false,
            message: message.into(),
            details: Vec::new(),
        }
    }

    pub fn with_host_api_detected(mut self, detected: bool) -> Self {
        self.host_api_detected = detected;
        self
    }

    pub fn with_can_mount(mut self, can_mount: bool) -> Self {
        self.can_mount = can_mount;
        self
    }

    pub fn with_real_os_integration(mut self, real_os_integration: bool) -> Self {
        self.real_os_integration = real_os_integration;
        self
    }

    pub fn with_supports_hydrate_on_open(mut self, supports_hydrate_on_open: bool) -> Self {
        self.supports_hydrate_on_open = supports_hydrate_on_open;
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }

    pub fn adapter_kind(&self) -> FsAdapterKind {
        self.adapter_kind
    }

    pub fn host_platform(&self) -> HostPlatform {
        self.host_platform
    }

    pub fn host_api(&self) -> &str {
        &self.host_api
    }

    pub fn host_api_detected(&self) -> bool {
        self.host_api_detected
    }

    pub fn can_mount(&self) -> bool {
        self.can_mount
    }

    pub fn real_os_integration(&self) -> bool {
        self.real_os_integration
    }

    pub fn supports_hydrate_on_open(&self) -> bool {
        self.supports_hydrate_on_open
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> &[String] {
        &self.details
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsMountState {
    Mounted,
    Unmounted,
}

impl FsMountState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mounted => "mounted",
            Self::Unmounted => "unmounted",
        }
    }

    fn from_label(value: &str) -> Option<Self> {
        match value {
            "mounted" => Some(Self::Mounted),
            "unmounted" => Some(Self::Unmounted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsMountRecord {
    mount_id: String,
    adapter_kind: FsAdapterKind,
    shared_folder_id: String,
    revision_id: String,
    mount_path: PathBuf,
    state: FsMountState,
    projection_state: String,
    hydrate_on_open: bool,
    created_at: String,
    updated_at: String,
}

impl FsMountRecord {
    pub fn mount_id(&self) -> &str {
        &self.mount_id
    }

    pub fn adapter_kind(&self) -> FsAdapterKind {
        self.adapter_kind
    }

    pub fn shared_folder_id(&self) -> &str {
        &self.shared_folder_id
    }

    pub fn revision_id(&self) -> &str {
        &self.revision_id
    }

    pub fn mount_path(&self) -> &Path {
        &self.mount_path
    }

    pub fn state(&self) -> FsMountState {
        self.state
    }

    pub fn projection_state(&self) -> &str {
        &self.projection_state
    }

    pub fn hydrate_on_open(&self) -> bool {
        self.hydrate_on_open
    }

    pub fn created_at(&self) -> &str {
        &self.created_at
    }

    pub fn updated_at(&self) -> &str {
        &self.updated_at
    }

    fn active_for(&self, adapter_kind: FsAdapterKind, mount_path: &Path) -> bool {
        self.adapter_kind == adapter_kind
            && self.mount_path == mount_path
            && self.state == FsMountState::Mounted
    }

    fn same_mount_path(&self, mount_path: &Path) -> bool {
        self.mount_path == mount_path && self.state == FsMountState::Mounted
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsMountRequest {
    mount_path: PathBuf,
}

impl FsMountRequest {
    pub fn new(mount_path: impl Into<PathBuf>) -> Self {
        Self {
            mount_path: mount_path.into(),
        }
    }

    pub fn mount_path(&self) -> &Path {
        &self.mount_path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUnmountRequest {
    mount_path: PathBuf,
}

impl FsUnmountRequest {
    pub fn new(mount_path: impl Into<PathBuf>) -> Self {
        Self {
            mount_path: mount_path.into(),
        }
    }

    pub fn mount_path(&self) -> &Path {
        &self.mount_path
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FsStatusRequest {
    mount_path: Option<PathBuf>,
}

impl FsStatusRequest {
    pub fn all() -> Self {
        Self { mount_path: None }
    }

    pub fn for_mount_path(mount_path: impl Into<PathBuf>) -> Self {
        Self {
            mount_path: Some(mount_path.into()),
        }
    }

    pub fn mount_path(&self) -> Option<&Path> {
        self.mount_path.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsMountSafetyReport {
    revision_id: String,
    tracked_entries: usize,
    ignored_entries: usize,
}

impl FsMountSafetyReport {
    pub fn revision_id(&self) -> &str {
        &self.revision_id
    }

    pub fn tracked_entries(&self) -> usize {
        self.tracked_entries
    }

    pub fn ignored_entries(&self) -> usize {
        self.ignored_entries
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsMountReport {
    record: FsMountRecord,
    already_mounted: bool,
    safety: FsMountSafetyReport,
}

impl FsMountReport {
    pub fn record(&self) -> &FsMountRecord {
        &self.record
    }

    pub fn already_mounted(&self) -> bool {
        self.already_mounted
    }

    pub fn safety(&self) -> &FsMountSafetyReport {
        &self.safety
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsUnmountReport {
    mount_path: PathBuf,
    changed: bool,
    record: Option<FsMountRecord>,
    message: String,
}

impl FsUnmountReport {
    pub fn mount_path(&self) -> &Path {
        &self.mount_path
    }

    pub fn changed(&self) -> bool {
        self.changed
    }

    pub fn record(&self) -> Option<&FsMountRecord> {
        self.record.as_ref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsStatusReport {
    capabilities: FsCapabilities,
    records: Vec<FsMountRecord>,
}

impl FsStatusReport {
    pub fn capabilities(&self) -> &FsCapabilities {
        &self.capabilities
    }

    pub fn records(&self) -> &[FsMountRecord] {
        &self.records
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsDoctorReport {
    capabilities: FsCapabilities,
    records: Vec<FsMountRecord>,
    issues: Vec<String>,
}

impl FsDoctorReport {
    pub fn capabilities(&self) -> &FsCapabilities {
        &self.capabilities
    }

    pub fn records(&self) -> &[FsMountRecord] {
        &self.records
    }

    pub fn issues(&self) -> &[String] {
        &self.issues
    }

    pub fn healthy(&self) -> bool {
        self.issues.is_empty()
    }
}

#[derive(Debug)]
pub enum FsError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Store(StoreError),
    Capture(CaptureError),
    Unsupported {
        adapter_kind: FsAdapterKind,
        message: String,
    },
    MissingRevision,
    DirtyWorktree {
        revision_id: String,
        created: usize,
        modified: usize,
        deleted: usize,
    },
    UnsafeWorktree {
        path: PathBuf,
        reason: String,
    },
    MountPathInsideSharedFolder {
        mount_path: PathBuf,
        folder: PathBuf,
    },
    MountPathIsFile {
        mount_path: PathBuf,
    },
    MountPathInUse {
        mount_path: PathBuf,
        adapter_kind: FsAdapterKind,
    },
    CorruptRegistry {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not access {}: {source}", path.display()),
            Self::Store(error) => write!(f, "{error}"),
            Self::Capture(error) => write!(f, "{error}"),
            Self::Unsupported {
                adapter_kind,
                message,
            } => write!(
                f,
                "{} adapter is unsupported for mount: {message}",
                adapter_kind.label()
            ),
            Self::MissingRevision => {
                write!(f, "fs mount requires a folder revision; run 'loom status' first")
            }
            Self::DirtyWorktree {
                revision_id,
                created,
                modified,
                deleted,
            } => write!(
                f,
                "fs mount refused because the folder differs from revision {revision_id}: {created} created, {modified} modified, {deleted} deleted"
            ),
            Self::UnsafeWorktree { path, reason } => write!(
                f,
                "fs mount refused because {} is unsafe: {reason}",
                path_to_store_string(path)
            ),
            Self::MountPathInsideSharedFolder { mount_path, folder } => write!(
                f,
                "fs mount refused because mount path {} is inside shared folder {}",
                mount_path.display(),
                folder.display()
            ),
            Self::MountPathIsFile { mount_path } => write!(
                f,
                "fs mount refused because mount path is a file: {}",
                mount_path.display()
            ),
            Self::MountPathInUse {
                mount_path,
                adapter_kind,
            } => write!(
                f,
                "fs mount path {} is already mounted by {}",
                mount_path.display(),
                adapter_kind.label()
            ),
            Self::CorruptRegistry { path, message } => {
                write!(f, "could not read fs adapter registry {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for FsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Store(error) => Some(error),
            Self::Capture(error) => Some(error),
            Self::Unsupported { .. }
            | Self::MissingRevision
            | Self::DirtyWorktree { .. }
            | Self::UnsafeWorktree { .. }
            | Self::MountPathInsideSharedFolder { .. }
            | Self::MountPathIsFile { .. }
            | Self::MountPathInUse { .. }
            | Self::CorruptRegistry { .. } => None,
        }
    }
}

impl From<StoreError> for FsError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<CaptureError> for FsError {
    fn from(error: CaptureError) -> Self {
        Self::Capture(error)
    }
}

pub type FsResult<T> = Result<T, FsError>;

pub trait FilesystemAdapter {
    fn kind(&self) -> FsAdapterKind;
    fn capabilities(&self) -> FsCapabilities;
    fn mount(&self, store: &LocalStore, request: FsMountRequest) -> FsResult<FsMountReport>;
    fn unmount(&self, store: &LocalStore, request: FsUnmountRequest) -> FsResult<FsUnmountReport>;
    fn status(&self, store: &LocalStore, request: FsStatusRequest) -> FsResult<FsStatusReport>;
    fn doctor(&self, store: &LocalStore) -> FsResult<FsDoctorReport>;
}

#[derive(Debug, Clone, Default)]
pub struct LocalDevFilesystemAdapter;

impl LocalDevFilesystemAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl FilesystemAdapter for LocalDevFilesystemAdapter {
    fn kind(&self) -> FsAdapterKind {
        FsAdapterKind::LocalDev
    }

    fn capabilities(&self) -> FsCapabilities {
        FsCapabilities::new(
            FsAdapterKind::LocalDev,
            HostPlatform::current(),
            "local metadata registry",
            "Local-dev simulates mount metadata only. It does not create placeholders or hydrate bytes on open.",
        )
        .with_host_api_detected(true)
        .with_can_mount(true)
        .with_detail("Projection state is stored under .loom/fs for deterministic tests.")
        .with_detail("Reads still use explicit Loom hydrate/workspace commands.")
    }

    fn mount(&self, store: &LocalStore, request: FsMountRequest) -> FsResult<FsMountReport> {
        let mount_path = normalize_absolute_path(request.mount_path())?;
        validate_mount_path(store, &mount_path)?;
        let safety = ensure_mount_safe(store)?;
        let mut records = read_mount_records(store)?;

        if let Some(record) = records
            .iter()
            .find(|record| record.active_for(self.kind(), &mount_path))
            .cloned()
        {
            return Ok(FsMountReport {
                record,
                already_mounted: true,
                safety,
            });
        }

        if let Some(record) = records
            .iter()
            .find(|record| record.same_mount_path(&mount_path))
            .cloned()
        {
            return Err(FsError::MountPathInUse {
                mount_path,
                adapter_kind: record.adapter_kind(),
            });
        }

        let now = current_timestamp();
        let replacement = records.iter().position(|record| {
            record.adapter_kind == self.kind() && record.mount_path == mount_path
        });
        let record = match replacement {
            Some(index) => {
                let mut record = records[index].clone();
                record.revision_id = safety.revision_id.clone();
                record.state = FsMountState::Mounted;
                record.projection_state = SIMULATED_PROJECTION_STATE.to_string();
                record.hydrate_on_open = false;
                record.updated_at = now;
                records[index] = record.clone();
                record
            }
            None => {
                let record = FsMountRecord {
                    mount_id: generated_mount_id(),
                    adapter_kind: self.kind(),
                    shared_folder_id: store.shared_folder().id().as_str().to_string(),
                    revision_id: safety.revision_id.clone(),
                    mount_path: mount_path.clone(),
                    state: FsMountState::Mounted,
                    projection_state: SIMULATED_PROJECTION_STATE.to_string(),
                    hydrate_on_open: false,
                    created_at: now.clone(),
                    updated_at: now,
                };
                records.push(record.clone());
                record
            }
        };
        write_mount_records(store, &records)?;

        Ok(FsMountReport {
            record,
            already_mounted: false,
            safety,
        })
    }

    fn unmount(&self, store: &LocalStore, request: FsUnmountRequest) -> FsResult<FsUnmountReport> {
        unmount_record(store, self.kind(), request.mount_path())
    }

    fn status(&self, store: &LocalStore, request: FsStatusRequest) -> FsResult<FsStatusReport> {
        Ok(FsStatusReport {
            capabilities: self.capabilities(),
            records: records_for_status(store, self.kind(), request)?,
        })
    }

    fn doctor(&self, store: &LocalStore) -> FsResult<FsDoctorReport> {
        let capabilities = self.capabilities();
        let records = records_for_status(store, self.kind(), FsStatusRequest::all())?;
        let mut issues = Vec::new();
        if !capabilities.can_mount() {
            issues.push(capabilities.message().to_string());
        }
        for record in &records {
            if record.hydrate_on_open() {
                issues.push(format!(
                    "{} claims hydrate-on-open, which is not implemented",
                    record.mount_id()
                ));
            }
        }

        Ok(FsDoctorReport {
            capabilities,
            records,
            issues,
        })
    }
}

#[derive(Debug, Clone)]
pub struct NativeStubFilesystemAdapter {
    capabilities: FsCapabilities,
}

impl NativeStubFilesystemAdapter {
    pub fn new(capabilities: FsCapabilities) -> Self {
        Self { capabilities }
    }
}

impl FilesystemAdapter for NativeStubFilesystemAdapter {
    fn kind(&self) -> FsAdapterKind {
        self.capabilities.adapter_kind()
    }

    fn capabilities(&self) -> FsCapabilities {
        self.capabilities.clone()
    }

    fn mount(&self, _store: &LocalStore, _request: FsMountRequest) -> FsResult<FsMountReport> {
        Err(FsError::Unsupported {
            adapter_kind: self.kind(),
            message: self.capabilities.message().to_string(),
        })
    }

    fn unmount(&self, store: &LocalStore, request: FsUnmountRequest) -> FsResult<FsUnmountReport> {
        unmount_record(store, self.kind(), request.mount_path())
    }

    fn status(&self, store: &LocalStore, request: FsStatusRequest) -> FsResult<FsStatusReport> {
        Ok(FsStatusReport {
            capabilities: self.capabilities(),
            records: records_for_status(store, self.kind(), request)?,
        })
    }

    fn doctor(&self, store: &LocalStore) -> FsResult<FsDoctorReport> {
        let capabilities = self.capabilities();
        let records = records_for_status(store, self.kind(), FsStatusRequest::all())?;
        let mut issues = Vec::new();
        if !capabilities.can_mount() {
            issues.push(capabilities.message().to_string());
        }
        if capabilities.supports_hydrate_on_open() {
            issues.push("native adapter reports hydrate-on-open before implementation".to_string());
        }

        Ok(FsDoctorReport {
            capabilities,
            records,
            issues,
        })
    }
}

pub fn native_adapter_kind() -> FsAdapterKind {
    match HostPlatform::current() {
        HostPlatform::Windows => FsAdapterKind::WindowsCloudFiles,
        HostPlatform::MacOs => FsAdapterKind::MacOsFileProvider,
        HostPlatform::Linux => FsAdapterKind::LinuxFuse,
        HostPlatform::Other => FsAdapterKind::LinuxFuse,
    }
}

pub fn adapter_for_kind(kind: FsAdapterKind) -> Box<dyn FilesystemAdapter> {
    match kind {
        FsAdapterKind::LocalDev => Box::new(LocalDevFilesystemAdapter::new()),
        FsAdapterKind::WindowsCloudFiles => Box::new(NativeStubFilesystemAdapter::new(
            windows::detect_capabilities(),
        )),
        FsAdapterKind::MacOsFileProvider => Box::new(NativeStubFilesystemAdapter::new(
            macos::detect_capabilities(),
        )),
        FsAdapterKind::LinuxFuse => Box::new(NativeStubFilesystemAdapter::new(
            linux::detect_capabilities(),
        )),
    }
}

pub mod windows {
    use super::{FsAdapterKind, FsCapabilities, HostPlatform};
    use std::path::PathBuf;

    pub fn detect_capabilities() -> FsCapabilities {
        let host = HostPlatform::current();
        let mut details = Vec::new();
        let running_windows = matches!(host, HostPlatform::Windows);
        let cloud_files = running_windows && windows_driver_exists("cldflt.sys");
        let projected_fs = running_windows && windows_driver_exists("prjflt.sys");

        details.push(format!("running on Windows: {running_windows}"));
        details.push(format!("Cloud Files driver detected: {cloud_files}"));
        details.push(format!("Projected FS driver detected: {projected_fs}"));

        let detected = cloud_files || projected_fs;
        let mut capabilities = FsCapabilities::new(
            FsAdapterKind::WindowsCloudFiles,
            host,
            "Cloud Files / Projected File System",
            "Windows native filesystem mounting is an alpha stub; no Cloud Files or Projected FS provider is registered by Loom yet.",
        )
        .with_host_api_detected(detected)
        .with_detail("Mount fails closed until a real provider callback layer exists.")
        .with_detail("Hydrate-on-open is false because no placeholder/open callback is implemented.");

        for detail in details {
            capabilities = capabilities.with_detail(detail);
        }

        capabilities
    }

    fn windows_driver_exists(name: &str) -> bool {
        let Some(system_root) = std::env::var_os("SystemRoot") else {
            return false;
        };
        PathBuf::from(system_root)
            .join("System32")
            .join("drivers")
            .join(name)
            .is_file()
    }
}

pub mod macos {
    use super::{FsAdapterKind, FsCapabilities, HostPlatform};
    use std::path::Path;

    pub fn detect_capabilities() -> FsCapabilities {
        let host = HostPlatform::current();
        let running_macos = matches!(host, HostPlatform::MacOs);
        let file_provider = running_macos
            && Path::new("/System/Library/Frameworks/FileProvider.framework").exists();
        let macfuse = running_macos
            && (Path::new("/Library/Filesystems/macfuse.fs").exists()
                || Path::new("/Library/Filesystems/osxfuse.fs").exists());

        FsCapabilities::new(
            FsAdapterKind::MacOsFileProvider,
            host,
            "File Provider / FUSE",
            "macOS native filesystem mounting is an alpha stub; Loom has no File Provider or FUSE extension yet.",
        )
        .with_host_api_detected(file_provider || macfuse)
        .with_detail("Mount fails closed until a real File Provider domain or FUSE filesystem exists.")
        .with_detail("Hydrate-on-open is false because no provider materialization callback is implemented.")
        .with_detail(format!("running on macOS: {running_macos}"))
        .with_detail(format!("File Provider framework detected: {file_provider}"))
        .with_detail(format!("macFUSE/osxfuse detected: {macfuse}"))
    }
}

pub mod linux {
    use super::{FsAdapterKind, FsCapabilities, HostPlatform};
    use std::path::Path;

    pub fn detect_capabilities() -> FsCapabilities {
        let host = HostPlatform::current();
        let running_linux = matches!(host, HostPlatform::Linux);
        let dev_fuse = running_linux && Path::new("/dev/fuse").exists();

        FsCapabilities::new(
            FsAdapterKind::LinuxFuse,
            host,
            "FUSE",
            "Linux native filesystem mounting is an alpha stub; Loom has no FUSE filesystem process yet.",
        )
        .with_host_api_detected(dev_fuse)
        .with_detail("Mount fails closed until a real FUSE filesystem loop exists.")
        .with_detail("Hydrate-on-open is false because no read/open callback is implemented.")
        .with_detail(format!("running on Linux: {running_linux}"))
        .with_detail(format!("/dev/fuse detected: {dev_fuse}"))
    }
}

fn ensure_mount_safe(store: &LocalStore) -> FsResult<FsMountSafetyReport> {
    let latest = store.latest_revision()?.ok_or(FsError::MissingRevision)?;
    let request = CaptureRequest::new(store.shared_folder().clone(), RevisionBoundary::LoomCommand);
    let capture = CaptureEngine::new(store).capture(&request)?;

    if let Some(notice) = capture.blocked().first() {
        return Err(FsError::UnsafeWorktree {
            path: notice.relative_path().to_path_buf(),
            reason: notice.reason().to_string(),
        });
    }
    if let Some(notice) = capture.deferred().first() {
        return Err(FsError::UnsafeWorktree {
            path: notice.relative_path().to_path_buf(),
            reason: notice.reason().to_string(),
        });
    }

    let diff = diff_revision_to_capture(&latest, &capture)?;
    if diff.has_changes() {
        return Err(dirty_worktree_error(&latest, &diff));
    }

    Ok(FsMountSafetyReport {
        revision_id: latest.id().as_str().to_string(),
        tracked_entries: capture.file_versions().len(),
        ignored_entries: capture.summary().ignored_entries(),
    })
}

fn dirty_worktree_error(latest: &FolderRevision, diff: &WorktreeDiff) -> FsError {
    FsError::DirtyWorktree {
        revision_id: latest.id().as_str().to_string(),
        created: diff.created().len(),
        modified: diff.modified().len(),
        deleted: diff.deleted().len(),
    }
}

fn validate_mount_path(store: &LocalStore, mount_path: &Path) -> FsResult<()> {
    if mount_path.exists() && mount_path.is_file() {
        return Err(FsError::MountPathIsFile {
            mount_path: mount_path.to_path_buf(),
        });
    }
    if same_or_child_path(mount_path, store.folder_root()) {
        return Err(FsError::MountPathInsideSharedFolder {
            mount_path: mount_path.to_path_buf(),
            folder: store.folder_root().to_path_buf(),
        });
    }

    Ok(())
}

fn unmount_record(
    store: &LocalStore,
    adapter_kind: FsAdapterKind,
    mount_path: &Path,
) -> FsResult<FsUnmountReport> {
    let mount_path = normalize_absolute_path(mount_path)?;
    let mut records = read_mount_records(store)?;
    let Some(index) = records
        .iter()
        .position(|record| record.active_for(adapter_kind, &mount_path))
    else {
        return Ok(FsUnmountReport {
            mount_path,
            changed: false,
            record: None,
            message: format!("no active {} mount record", adapter_kind.label()),
        });
    };

    records[index].state = FsMountState::Unmounted;
    records[index].updated_at = current_timestamp();
    let record = records[index].clone();
    write_mount_records(store, &records)?;

    Ok(FsUnmountReport {
        mount_path,
        changed: true,
        record: Some(record),
        message: "mount record marked unmounted".to_string(),
    })
}

fn records_for_status(
    store: &LocalStore,
    adapter_kind: FsAdapterKind,
    request: FsStatusRequest,
) -> FsResult<Vec<FsMountRecord>> {
    let mount_path = request
        .mount_path()
        .map(normalize_absolute_path)
        .transpose()?;
    let mut records = read_mount_records(store)?
        .into_iter()
        .filter(|record| record.adapter_kind == adapter_kind)
        .filter(|record| match mount_path.as_deref() {
            Some(path) => record.mount_path == path,
            None => true,
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| {
        left.mount_path
            .cmp(&right.mount_path)
            .then_with(|| left.created_at.cmp(&right.created_at))
    });
    Ok(records)
}

fn read_mount_records(store: &LocalStore) -> FsResult<Vec<FsMountRecord>> {
    let path = mounts_file(store);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(FsError::Io { path, source }),
    };
    let mut records = Vec::new();
    for (line_index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields = split_registry_fields(&path, line_index + 1, line, 10)?;
        let adapter_kind =
            FsAdapterKind::from_label(&fields[1]).ok_or_else(|| FsError::CorruptRegistry {
                path: path.clone(),
                message: format!(
                    "line {} has unknown adapter '{}'",
                    line_index + 1,
                    fields[1]
                ),
            })?;
        let state =
            FsMountState::from_label(&fields[5]).ok_or_else(|| FsError::CorruptRegistry {
                path: path.clone(),
                message: format!("line {} has unknown state '{}'", line_index + 1, fields[5]),
            })?;
        let hydrate_on_open = match fields[7].as_str() {
            "true" => true,
            "false" => false,
            value => {
                return Err(FsError::CorruptRegistry {
                    path: path.clone(),
                    message: format!(
                        "line {} has invalid hydrate_on_open '{}'",
                        line_index + 1,
                        value
                    ),
                });
            }
        };

        records.push(FsMountRecord {
            mount_id: fields[0].clone(),
            adapter_kind,
            shared_folder_id: fields[2].clone(),
            revision_id: fields[3].clone(),
            mount_path: PathBuf::from(&fields[4]),
            state,
            projection_state: fields[6].clone(),
            hydrate_on_open,
            created_at: fields[8].clone(),
            updated_at: fields[9].clone(),
        });
    }

    Ok(records)
}

fn write_mount_records(store: &LocalStore, records: &[FsMountRecord]) -> FsResult<()> {
    let dir = fs_dir(store);
    fs::create_dir_all(&dir).map_err(|source| FsError::Io {
        path: dir.clone(),
        source,
    })?;
    let path = mounts_file(store);
    let mut contents = String::new();
    for record in records {
        contents.push_str(&mount_record_row(record));
    }
    fs::write(&path, contents).map_err(|source| FsError::Io { path, source })
}

fn mount_record_row(record: &FsMountRecord) -> String {
    [
        record.mount_id.as_str(),
        record.adapter_kind.label(),
        record.shared_folder_id.as_str(),
        record.revision_id.as_str(),
        &record.mount_path.to_string_lossy(),
        record.state.label(),
        record.projection_state.as_str(),
        if record.hydrate_on_open {
            "true"
        } else {
            "false"
        },
        record.created_at.as_str(),
        record.updated_at.as_str(),
    ]
    .into_iter()
    .map(encode_registry_field)
    .collect::<Vec<_>>()
    .join("\t")
        + "\n"
}

fn fs_dir(store: &LocalStore) -> PathBuf {
    store.store_root().join(FS_DIR)
}

fn mounts_file(store: &LocalStore) -> PathBuf {
    fs_dir(store).join(MOUNTS_FILE)
}

fn normalize_absolute_path(path: &Path) -> FsResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| FsError::Io {
                path: PathBuf::from("."),
                source,
            })?
            .join(path)
    };

    Ok(normalize_components(&absolute))
}

fn normalize_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn same_or_child_path(candidate: &Path, parent: &Path) -> bool {
    if cfg!(windows) {
        let candidate = windows_path_key(candidate);
        let parent = windows_path_key(parent);
        candidate == parent || candidate.starts_with(&format!("{parent}\\"))
    } else {
        candidate == parent || candidate.starts_with(parent)
    }
}

#[cfg(windows)]
fn windows_path_key(path: &Path) -> String {
    let mut value = path.to_string_lossy().replace('/', "\\");
    if let Some(stripped) = value.strip_prefix(r"\\?\UNC\") {
        value = format!(r"\\{}", stripped);
    } else if let Some(stripped) = value.strip_prefix(r"\\?\") {
        value = stripped.to_string();
    }
    while value.ends_with('\\') && value.len() > 3 {
        value.pop();
    }
    value.to_ascii_lowercase()
}

#[cfg(not(windows))]
fn windows_path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn split_registry_fields(
    path: &Path,
    line_number: usize,
    line: &str,
    expected: usize,
) -> FsResult<Vec<String>> {
    let fields = line
        .split('\t')
        .map(decode_registry_field)
        .collect::<FsResult<Vec<_>>>()?;
    if fields.len() != expected {
        return Err(FsError::CorruptRegistry {
            path: path.to_path_buf(),
            message: format!(
                "line {line_number} has {} fields, expected {expected}",
                fields.len()
            ),
        });
    }

    Ok(fields)
}

fn encode_registry_field(value: &str) -> String {
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

fn decode_registry_field(value: &str) -> FsResult<String> {
    let mut decoded = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(FsError::CorruptRegistry {
                    path: PathBuf::from("<field>"),
                    message: "truncated percent escape".to_string(),
                });
            }
            let hex = &value[index + 1..index + 3];
            let byte = u8::from_str_radix(hex, 16).map_err(|_| FsError::CorruptRegistry {
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

fn generated_mount_id() -> String {
    let counter = MOUNT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    format!("fs-mount-{}-{nanos}-{counter}", process::id())
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
    use loom_store::LocalStore;
    use std::fs;

    #[test]
    fn local_dev_capabilities_are_explicit_simulation() {
        let capabilities = LocalDevFilesystemAdapter::new().capabilities();

        assert_eq!(capabilities.adapter_kind(), FsAdapterKind::LocalDev);
        assert!(capabilities.can_mount());
        assert!(!capabilities.real_os_integration());
        assert!(!capabilities.supports_hydrate_on_open());
        assert!(capabilities.message().contains("metadata only"));
        assert!(CRATE_ROLE.contains("adapter"));
    }

    #[test]
    fn native_platform_capabilities_fail_closed_without_hydrate_on_open() {
        for capabilities in [
            windows::detect_capabilities(),
            macos::detect_capabilities(),
            linux::detect_capabilities(),
        ] {
            assert!(!capabilities.can_mount());
            assert!(!capabilities.real_os_integration());
            assert!(!capabilities.supports_hydrate_on_open());
            assert!(capabilities.message().contains("alpha stub"));
        }
    }

    #[test]
    fn local_dev_mount_persists_simulated_projection_state() {
        let fixture = TestFolder::new();
        fixture.write("README.md", "hello\n");
        fixture.capture_latest();
        let mount_path = fixture.dir.path().join("mount-view");
        let adapter = LocalDevFilesystemAdapter::new();

        let report = adapter
            .mount(&fixture.store, FsMountRequest::new(&mount_path))
            .expect("local dev mount records");

        assert!(!report.already_mounted());
        assert_eq!(report.record().state(), FsMountState::Mounted);
        assert_eq!(
            report.record().projection_state(),
            SIMULATED_PROJECTION_STATE
        );
        assert!(!report.record().hydrate_on_open());
        assert!(
            !mount_path.exists(),
            "simulation must not create placeholder paths"
        );

        let reopened = LocalStore::open(&fixture.root).expect("store reopens");
        let status = adapter
            .status(&reopened, FsStatusRequest::for_mount_path(&mount_path))
            .expect("status reads registry");
        assert_eq!(status.records().len(), 1);
        assert_eq!(status.records()[0].mount_path(), mount_path.as_path());
        assert_eq!(status.records()[0].state(), FsMountState::Mounted);
    }

    #[test]
    fn local_dev_mount_is_idempotent_for_active_record() {
        let fixture = TestFolder::new();
        fixture.write("README.md", "hello\n");
        fixture.capture_latest();
        let mount_path = fixture.dir.path().join("mount-view");
        let adapter = LocalDevFilesystemAdapter::new();

        let first = adapter
            .mount(&fixture.store, FsMountRequest::new(&mount_path))
            .expect("first mount");
        let second = adapter
            .mount(&fixture.store, FsMountRequest::new(&mount_path))
            .expect("second mount is idempotent");

        assert!(!first.already_mounted());
        assert!(second.already_mounted());
        assert_eq!(first.record().mount_id(), second.record().mount_id());
    }

    #[test]
    fn local_dev_unmount_is_idempotent_and_truthful() {
        let fixture = TestFolder::new();
        fixture.write("README.md", "hello\n");
        fixture.capture_latest();
        let mount_path = fixture.dir.path().join("mount-view");
        let adapter = LocalDevFilesystemAdapter::new();
        adapter
            .mount(&fixture.store, FsMountRequest::new(&mount_path))
            .expect("mount records");

        let first = adapter
            .unmount(&fixture.store, FsUnmountRequest::new(&mount_path))
            .expect("unmount records");
        let second = adapter
            .unmount(&fixture.store, FsUnmountRequest::new(&mount_path))
            .expect("second unmount no-ops");

        assert!(first.changed());
        assert_eq!(
            first.record().expect("record").state(),
            FsMountState::Unmounted
        );
        assert!(!second.changed());
        assert!(second.message().contains("no active"));
    }

    #[test]
    fn native_mount_returns_unsupported_and_writes_no_record() {
        let fixture = TestFolder::new();
        fixture.write("README.md", "hello\n");
        fixture.capture_latest();
        let adapter = adapter_for_kind(native_adapter_kind());
        let mount_path = fixture.dir.path().join("native-view");

        let error = adapter
            .mount(&fixture.store, FsMountRequest::new(&mount_path))
            .expect_err("native mount fails closed");

        assert!(matches!(error, FsError::Unsupported { .. }));
        let status = adapter
            .status(&fixture.store, FsStatusRequest::all())
            .expect("status works");
        assert!(status.records().is_empty());
    }

    #[test]
    fn mount_refuses_dirty_folder_state() {
        let fixture = TestFolder::new();
        fixture.write("README.md", "hello\n");
        fixture.capture_latest();
        fixture.write("new.txt", "dirty\n");
        let adapter = LocalDevFilesystemAdapter::new();
        let mount_path = fixture.dir.path().join("mount-view");

        let error = adapter
            .mount(&fixture.store, FsMountRequest::new(&mount_path))
            .expect_err("dirty mount refused");

        assert!(matches!(error, FsError::DirtyWorktree { created: 1, .. }));
        assert!(adapter
            .status(&fixture.store, FsStatusRequest::all())
            .expect("status")
            .records()
            .is_empty());
    }

    #[test]
    fn mount_refuses_path_inside_shared_folder() {
        let fixture = TestFolder::new();
        fixture.write("README.md", "hello\n");
        fixture.capture_latest();
        let adapter = LocalDevFilesystemAdapter::new();

        let error = adapter
            .mount(
                &fixture.store,
                FsMountRequest::new(fixture.root.join("nested-view")),
            )
            .expect_err("mount path inside folder refused");

        assert!(matches!(error, FsError::MountPathInsideSharedFolder { .. }));
    }

    struct TestFolder {
        dir: tempfile::TempDir,
        root: PathBuf,
        store: LocalStore,
    }

    impl TestFolder {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let root = dir.path().join("shared");
            fs::create_dir_all(&root).expect("folder creates");
            let store = LocalStore::open_or_init(&root)
                .expect("store initializes")
                .into_store();

            Self { dir, root, store }
        }

        fn write(&self, path: &str, contents: &str) {
            let path = self.root.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent creates");
            }
            fs::write(path, contents).expect("file writes");
        }

        fn capture_latest(&self) {
            let request = CaptureRequest::new(
                self.store.shared_folder().clone(),
                RevisionBoundary::LoomCommand,
            );
            let capture = CaptureEngine::new(&self.store)
                .capture(&request)
                .expect("capture succeeds");
            self.store
                .coalesce_folder_revision(RevisionBoundary::LoomCommand, capture.file_versions())
                .expect("revision coalesces");
        }
    }
}
