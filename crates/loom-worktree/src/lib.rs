//! Shared-folder worktree boundary for Loom.
//!
//! Scanning, generated-file policy, materialization, restore safety, and
//! file-version capture belong here as the old snapshot crate is migrated.

use loom_core::{
    FileKind, FileVersion, FileVersionId, FolderRevision, FolderRevisionId, LoomError,
    RevisionBoundary, SharedFolder,
};
use loom_store::{path_to_store_string, LocalStore, ObjectCache, StoreError, STORE_DIR};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

#[cfg(test)]
const OLD_SECRET_SCAN_PREFIX_BYTES: usize = 1024 * 1024;
const MAX_SECRET_FINDINGS: usize = 16;

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

#[derive(Debug, Clone)]
pub struct RestoreEngine<'a> {
    store: &'a LocalStore,
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
            .collect::<Vec<_>>();
        removed.sort_by(|left, right| {
            path_to_store_string(right.path()).cmp(&path_to_store_string(left.path()))
        });

        for version in removed {
            remove_current_entry(current.root(), version)?;
        }

        let mut entries = revision.entries().to_vec();
        entries.sort_by(|left, right| {
            path_to_store_string(left.path()).cmp(&path_to_store_string(right.path()))
        });

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
            materialize_file_version(self.store, current.root(), version)?;
        }

        Ok(RestoreReport {
            revision_id: revision.id().clone(),
            diff,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CaptureEngine<'a> {
    object_cache: &'a ObjectCache,
}

impl<'a> CaptureEngine<'a> {
    pub fn new(object_cache: &'a ObjectCache) -> Self {
        Self { object_cache }
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
        walk_directory(self.object_cache, &root, &root, &captured_at, &mut capture)?;
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

fn materialize_file_version(
    store: &LocalStore,
    root: &Path,
    version: &FileVersion,
) -> CaptureResult<()> {
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
            prepare_file_target(version.path(), &target)?;
            fs::write(&target, bytes).map_err(|source| CaptureError::Io {
                path: target,
                source,
            })
        }
        FileKind::Symlink | FileKind::Unsupported => Err(CaptureError::UnsafeRestore {
            path: version.path().to_path_buf(),
            reason: "only regular files and directories can be restored safely".to_string(),
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
    object_cache: &ObjectCache,
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
                    walk_directory(object_cache, root, &entry_path, captured_at, capture)?;
                }
            }
        } else if metadata.is_file() {
            capture_file(
                object_cache,
                &entry_path,
                &relative_path,
                captured_at,
                capture,
            )?;
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
    object_cache: &ObjectCache,
    path: &Path,
    relative_path: &Path,
    captured_at: &str,
    capture: &mut WorktreeCapture,
) -> CaptureResult<()> {
    let bytes = read_file_bytes_for_secret_check(path)?;
    let findings = SecretDetector.scan_bytes(&bytes);
    if let Some(finding) = findings.first() {
        capture.summary.blocked_secret_files += 1;
        capture.blocked.push(CaptureNotice::new(
            relative_path.to_path_buf(),
            finding.policy_reason(),
        ));
        return Ok(());
    }

    let object = object_cache.write_bytes(&bytes)?;
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
    use loom_core::{FolderScope, SharedFolderId};
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
            CaptureEngine::new(self.store.object_cache())
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
