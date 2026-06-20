use devbox_core::{BlobId, ManifestEntryKind, PolicyDecision};
use devbox_store::{BlobCache, BlobCacheError, ManifestEntryRecord, PersistedSnapshot};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
    snapshot_id: String,
    target: PathBuf,
    target_status: RestoreTargetStatus,
    dirs_to_create: Vec<PathBuf>,
    files_to_write: Vec<RestoreWrite>,
    skipped_entries: Vec<RestoreSkippedEntry>,
    missing_blobs: Vec<RestoreMissingBlob>,
    total_bytes: u64,
}

impl RestorePlan {
    pub fn from_persisted_snapshot(
        snapshot: &PersistedSnapshot,
        cache: &BlobCache,
        target: impl AsRef<Path>,
    ) -> Result<Self, RestorePlanError> {
        let target = target.as_ref().to_path_buf();
        let target_status = inspect_target(&target)?;

        let mut dirs = BTreeSet::new();
        let mut files_to_write = Vec::new();
        let mut skipped_entries = Vec::new();
        let mut missing_blobs = Vec::new();
        let mut total_bytes = 0;

        for entry in &snapshot.entries {
            let Some(relative_path) = safe_restore_path(&entry.relative_path)? else {
                skipped_entries.push(skipped_entry(
                    entry,
                    "empty root path is not materialized by restore",
                ));
                continue;
            };

            match (&entry.policy_decision, &entry.kind) {
                (PolicyDecision::Include, ManifestEntryKind::Directory) => {
                    insert_dir_with_ancestors(&mut dirs, &relative_path);
                }
                (PolicyDecision::Include, ManifestEntryKind::File) => {
                    let Some(blob_id) = entry.blob_id.clone() else {
                        missing_blobs.push(RestoreMissingBlob {
                            path: relative_path,
                            blob_id: None,
                            object_ref: entry.object_ref.clone(),
                            reason: "included file entry is missing a blob id".to_string(),
                        });
                        continue;
                    };

                    let Some(object_ref) = entry.object_ref.clone() else {
                        missing_blobs.push(RestoreMissingBlob {
                            path: relative_path,
                            blob_id: Some(blob_id),
                            object_ref: None,
                            reason: "included file entry is missing an object ref".to_string(),
                        });
                        continue;
                    };

                    if !cache.exists(&blob_id) {
                        missing_blobs.push(RestoreMissingBlob {
                            path: relative_path,
                            blob_id: Some(blob_id),
                            object_ref: Some(object_ref),
                            reason: "blob cache object is missing".to_string(),
                        });
                        continue;
                    }

                    if let Some(parent) = relative_path.parent() {
                        if !parent.as_os_str().is_empty() {
                            insert_dir_with_ancestors(&mut dirs, parent);
                        }
                    }

                    total_bytes += entry.size_bytes;
                    files_to_write.push(RestoreWrite {
                        path: relative_path,
                        blob_id,
                        object_ref,
                        size_bytes: entry.size_bytes,
                    });
                }
                (PolicyDecision::Include, ManifestEntryKind::Symlink) => {
                    skipped_entries.push(skipped_entry(
                        entry,
                        "symlink restore is deferred until safety rules exist",
                    ));
                }
                (PolicyDecision::Include, ManifestEntryKind::Unsupported) => {
                    skipped_entries.push(skipped_entry(
                        entry,
                        "unsupported filesystem node restore is deferred",
                    ));
                }
                (PolicyDecision::Exclude { reason }, _) => {
                    skipped_entries.push(skipped_entry(entry, reason));
                }
                (PolicyDecision::RequiresUserDecision { reason }, _) => {
                    skipped_entries.push(skipped_entry(entry, reason));
                }
            }
        }

        Ok(Self {
            snapshot_id: snapshot.snapshot.id.clone(),
            target,
            target_status,
            dirs_to_create: dirs.into_iter().collect(),
            files_to_write,
            skipped_entries,
            missing_blobs,
            total_bytes,
        })
    }

    pub fn snapshot_id(&self) -> &str {
        &self.snapshot_id
    }

    pub fn target(&self) -> &Path {
        &self.target
    }

    pub fn target_status(&self) -> &RestoreTargetStatus {
        &self.target_status
    }

    pub fn dirs_to_create(&self) -> &[PathBuf] {
        &self.dirs_to_create
    }

    pub fn files_to_write(&self) -> &[RestoreWrite] {
        &self.files_to_write
    }

    pub fn skipped_entries(&self) -> &[RestoreSkippedEntry] {
        &self.skipped_entries
    }

    pub fn missing_blobs(&self) -> &[RestoreMissingBlob] {
        &self.missing_blobs
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn apply_allowed(&self) -> bool {
        self.target_status.allows_apply() && self.missing_blobs.is_empty()
    }

    pub fn summary(&self) -> RestorePlanSummary {
        RestorePlanSummary {
            dirs_to_create: self.dirs_to_create.len(),
            files_to_write: self.files_to_write.len(),
            skipped_entries: self.skipped_entries.len(),
            missing_blobs: self.missing_blobs.len(),
            bytes_to_write: self.total_bytes,
            apply_allowed: self.apply_allowed(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreWrite {
    pub path: PathBuf,
    pub blob_id: BlobId,
    pub object_ref: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreSkippedEntry {
    pub path: PathBuf,
    pub kind: ManifestEntryKind,
    pub decision: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreMissingBlob {
    pub path: PathBuf,
    pub blob_id: Option<BlobId>,
    pub object_ref: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlanSummary {
    pub dirs_to_create: usize,
    pub files_to_write: usize,
    pub skipped_entries: usize,
    pub missing_blobs: usize,
    pub bytes_to_write: u64,
    pub apply_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreTargetStatus {
    Missing,
    EmptyDirectory,
    NonEmptyDirectory,
    NotDirectory,
    Symlink,
}

impl RestoreTargetStatus {
    fn allows_apply(&self) -> bool {
        matches!(self, Self::Missing | Self::EmptyDirectory)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::EmptyDirectory => "empty_directory",
            Self::NonEmptyDirectory => "non_empty_directory",
            Self::NotDirectory => "not_directory",
            Self::Symlink => "symlink",
        }
    }
}

#[derive(Debug)]
pub enum RestorePlanError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    UnsafeManifestPath {
        path: PathBuf,
        reason: String,
    },
    ApplyNotAllowed {
        reason: String,
    },
    BlobCache {
        path: PathBuf,
        source: BlobCacheError,
    },
    FileAlreadyExists {
        path: PathBuf,
    },
    TargetEscaped {
        path: PathBuf,
        target: PathBuf,
    },
}

impl fmt::Display for RestorePlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "could not restore {}: {source}", path.display())
            }
            Self::UnsafeManifestPath { path, reason } => {
                write!(f, "unsafe manifest path {}: {reason}", path.display())
            }
            Self::ApplyNotAllowed { reason } => f.write_str(reason),
            Self::BlobCache { path, source } => {
                write!(
                    f,
                    "could not read cached blob for {}: {source}",
                    path.display()
                )
            }
            Self::FileAlreadyExists { path } => {
                write!(
                    f,
                    "restore would overwrite existing file: {}",
                    path.display()
                )
            }
            Self::TargetEscaped { path, target } => write!(
                f,
                "restore path {} escaped target {}",
                path.display(),
                target.display()
            ),
        }
    }
}

impl std::error::Error for RestorePlanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::BlobCache { source, .. } => Some(source),
            Self::UnsafeManifestPath { .. }
            | Self::ApplyNotAllowed { .. }
            | Self::FileAlreadyExists { .. }
            | Self::TargetEscaped { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RestoreMaterializer {
    cache: BlobCache,
}

impl RestoreMaterializer {
    pub fn new(cache: BlobCache) -> Self {
        Self { cache }
    }

    pub fn apply(&self, plan: &RestorePlan) -> Result<RestorePlanSummary, RestorePlanError> {
        if !plan.apply_allowed() {
            return Err(RestorePlanError::ApplyNotAllowed {
                reason: apply_block_reason(plan),
            });
        }

        match inspect_target(&plan.target)? {
            RestoreTargetStatus::Missing => {
                fs::create_dir_all(&plan.target).map_err(|source| RestorePlanError::Io {
                    path: plan.target.clone(),
                    source,
                })?
            }
            RestoreTargetStatus::EmptyDirectory => {}
            status => {
                return Err(RestorePlanError::ApplyNotAllowed {
                    reason: format!(
                        "restore target must be missing or empty; target status is {}",
                        status.as_str()
                    ),
                });
            }
        }

        let target_root =
            fs::canonicalize(&plan.target).map_err(|source| RestorePlanError::Io {
                path: plan.target.clone(),
                source,
            })?;

        for dir in &plan.dirs_to_create {
            let path = checked_target_path(&target_root, dir)?;
            fs::create_dir_all(&path).map_err(|source| RestorePlanError::Io {
                path: path.clone(),
                source,
            })?;
            ensure_real_directory(&path)?;
        }

        for file in &plan.files_to_write {
            let final_path = checked_target_path(&target_root, &file.path)?;
            if final_path.exists() {
                return Err(RestorePlanError::FileAlreadyExists { path: final_path });
            }

            let parent = final_path
                .parent()
                .expect("restore file paths always have a parent directory");
            ensure_real_directory(parent)?;

            let bytes =
                self.cache
                    .read(&file.blob_id)
                    .map_err(|source| RestorePlanError::BlobCache {
                        path: file.path.clone(),
                        source,
                    })?;
            let temp_path = write_temp_file(parent, &bytes)?;

            if final_path.exists() {
                cleanup_temp_file(&temp_path);
                return Err(RestorePlanError::FileAlreadyExists { path: final_path });
            }

            if let Err(source) = fs::rename(&temp_path, &final_path) {
                cleanup_temp_file(&temp_path);
                return Err(RestorePlanError::Io {
                    path: final_path,
                    source,
                });
            }
        }

        Ok(plan.summary())
    }
}

fn inspect_target(target: &Path) -> Result<RestoreTargetStatus, RestorePlanError> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(RestoreTargetStatus::Symlink),
        Ok(metadata) if !metadata.is_dir() => Ok(RestoreTargetStatus::NotDirectory),
        Ok(_) => {
            let mut entries = fs::read_dir(target).map_err(|source| RestorePlanError::Io {
                path: target.to_path_buf(),
                source,
            })?;
            if entries.next().is_some() {
                Ok(RestoreTargetStatus::NonEmptyDirectory)
            } else {
                Ok(RestoreTargetStatus::EmptyDirectory)
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(RestoreTargetStatus::Missing),
        Err(source) => Err(RestorePlanError::Io {
            path: target.to_path_buf(),
            source,
        }),
    }
}

fn safe_restore_path(path: &Path) -> Result<Option<PathBuf>, RestorePlanError> {
    let raw = path.to_string_lossy();
    if raw.is_empty() || raw == "." {
        return Ok(None);
    }

    if raw.contains('\\') {
        return Err(RestorePlanError::UnsafeManifestPath {
            path: path.to_path_buf(),
            reason: "alternate path separators are not accepted".to_string(),
        });
    }

    if raw.contains(':') {
        return Err(RestorePlanError::UnsafeManifestPath {
            path: path.to_path_buf(),
            reason: "path prefixes are not accepted".to_string(),
        });
    }

    if raw
        .split('/')
        .any(|segment| segment.is_empty() || segment == ".")
    {
        return Err(RestorePlanError::UnsafeManifestPath {
            path: path.to_path_buf(),
            reason: "empty or current directory path segments are not accepted".to_string(),
        });
    }

    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.is_empty() || part == "." || part == ".." {
                    return Err(RestorePlanError::UnsafeManifestPath {
                        path: path.to_path_buf(),
                        reason: "empty, current, or parent path segments are not accepted"
                            .to_string(),
                    });
                }
                safe.push(part.as_ref());
            }
            Component::CurDir => {
                return Err(RestorePlanError::UnsafeManifestPath {
                    path: path.to_path_buf(),
                    reason: "current directory segments are not accepted".to_string(),
                });
            }
            Component::ParentDir => {
                return Err(RestorePlanError::UnsafeManifestPath {
                    path: path.to_path_buf(),
                    reason: "parent directory traversal is not accepted".to_string(),
                });
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(RestorePlanError::UnsafeManifestPath {
                    path: path.to_path_buf(),
                    reason: "absolute paths and platform prefixes are not accepted".to_string(),
                });
            }
        }
    }

    if safe.as_os_str().is_empty() {
        Ok(None)
    } else {
        Ok(Some(safe))
    }
}

fn skipped_entry(entry: &ManifestEntryRecord, reason: &str) -> RestoreSkippedEntry {
    RestoreSkippedEntry {
        path: entry.relative_path.clone(),
        kind: entry.kind.clone(),
        decision: policy_name(&entry.policy_decision).to_string(),
        reason: reason.to_string(),
    }
}

fn insert_dir_with_ancestors(dirs: &mut BTreeSet<PathBuf>, path: &Path) {
    let mut current = PathBuf::new();
    for component in path.components() {
        if let Component::Normal(part) = component {
            current.push(part);
            dirs.insert(current.clone());
        }
    }
}

fn policy_name(policy: &PolicyDecision) -> &'static str {
    match policy {
        PolicyDecision::Include => "include",
        PolicyDecision::Exclude { .. } => "exclude",
        PolicyDecision::RequiresUserDecision { .. } => "requires_user_decision",
    }
}

fn apply_block_reason(plan: &RestorePlan) -> String {
    if !plan.target_status.allows_apply() {
        return format!(
            "restore target must be missing or empty; target status is {}",
            plan.target_status.as_str()
        );
    }

    if !plan.missing_blobs.is_empty() {
        return format!(
            "restore cannot apply because {} blob reference(s) are missing",
            plan.missing_blobs.len()
        );
    }

    "restore cannot apply".to_string()
}

fn checked_target_path(
    target_root: &Path,
    relative_path: &Path,
) -> Result<PathBuf, RestorePlanError> {
    let path = target_root.join(relative_path);
    if !path.starts_with(target_root) {
        return Err(RestorePlanError::TargetEscaped {
            path,
            target: target_root.to_path_buf(),
        });
    }

    Ok(path)
}

fn ensure_real_directory(path: &Path) -> Result<(), RestorePlanError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| RestorePlanError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RestorePlanError::ApplyNotAllowed {
            reason: format!("restore path is not a real directory: {}", path.display()),
        });
    }

    Ok(())
}

fn write_temp_file(parent: &Path, bytes: &[u8]) -> Result<PathBuf, RestorePlanError> {
    for _ in 0..100 {
        let temp_path = parent.join(temp_file_name());
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(mut file) => {
                if let Err(source) = write_all_and_sync(&mut file, bytes) {
                    cleanup_temp_file(&temp_path);
                    return Err(RestorePlanError::Io {
                        path: temp_path,
                        source,
                    });
                }
                return Ok(temp_path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(RestorePlanError::Io {
                    path: temp_path,
                    source,
                });
            }
        }
    }

    Err(RestorePlanError::Io {
        path: parent.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique restore temp file",
        ),
    })
}

fn write_all_and_sync(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
}

fn temp_file_name() -> String {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    format!(".devbox-restore-{}-{nanos}-{counter}.tmp", process::id())
}

fn cleanup_temp_file(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use devbox_core::BlobId;
    use devbox_store::{BlobCache, ProjectRecord, SnapshotRecord};

    #[test]
    fn plans_restore_from_persisted_snapshot_entries() {
        let fixture = RestoreFixture::new();
        let blob = fixture.cache.write_bytes(b"hello\n").expect("blob writes");
        let snapshot = fixture.snapshot(vec![
            included_dir("src"),
            included_file("src/main.rs", blob.id().clone(), blob.object_ref(), 6),
        ]);

        let plan = RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
            .expect("plan builds");

        assert!(plan.apply_allowed());
        assert_eq!(plan.snapshot_id(), "snapshot-1");
        assert_eq!(plan.target_status(), &RestoreTargetStatus::Missing);
        assert_eq!(plan.dirs_to_create(), &[PathBuf::from("src")]);
        assert_eq!(plan.files_to_write().len(), 1);
        assert_eq!(plan.total_bytes(), 6);
    }

    #[test]
    fn dry_run_plan_does_not_write_files() {
        let fixture = RestoreFixture::new();
        let blob = fixture.cache.write_bytes(b"hello").expect("blob writes");
        let snapshot = fixture.snapshot(vec![included_file(
            "README.md",
            blob.id().clone(),
            blob.object_ref(),
            5,
        )]);

        let plan = RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
            .expect("plan builds");

        assert!(plan.apply_allowed());
        assert!(!fixture.target.exists());
    }

    #[test]
    fn apply_writes_regular_files_and_nested_directories() {
        let fixture = RestoreFixture::new();
        let blob = fixture
            .cache
            .write_bytes(b"fn main() {}\n")
            .expect("blob writes");
        let snapshot = fixture.snapshot(vec![included_file(
            "src/bin/main.rs",
            blob.id().clone(),
            blob.object_ref(),
            13,
        )]);
        let plan = RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
            .expect("plan builds");

        let summary = RestoreMaterializer::new(fixture.cache.clone())
            .apply(&plan)
            .expect("restore applies");

        assert_eq!(summary.files_to_write, 1);
        assert_eq!(
            fs::read(fixture.target.join("src/bin/main.rs")).expect("restored file reads"),
            b"fn main() {}\n"
        );
    }

    #[test]
    fn refuses_non_empty_target_without_overwriting() {
        let fixture = RestoreFixture::new();
        fs::create_dir_all(&fixture.target).expect("target creates");
        fs::write(fixture.target.join("keep.txt"), "keep").expect("existing file writes");
        let blob = fixture.cache.write_bytes(b"new").expect("blob writes");
        let snapshot = fixture.snapshot(vec![included_file(
            "keep.txt",
            blob.id().clone(),
            blob.object_ref(),
            3,
        )]);
        let plan = RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
            .expect("plan builds");

        assert!(!plan.apply_allowed());
        let error = RestoreMaterializer::new(fixture.cache.clone())
            .apply(&plan)
            .expect_err("non-empty target is refused");

        assert!(matches!(error, RestorePlanError::ApplyNotAllowed { .. }));
        assert_eq!(
            fs::read_to_string(fixture.target.join("keep.txt")).expect("existing file reads"),
            "keep"
        );
    }

    #[test]
    fn rejects_path_traversal_manifest_paths() {
        let fixture = RestoreFixture::new();
        let blob = fixture.cache.write_bytes(b"evil").expect("blob writes");
        let snapshot = fixture.snapshot(vec![included_file(
            "../evil.txt",
            blob.id().clone(),
            blob.object_ref(),
            4,
        )]);

        let error =
            RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
                .expect_err("unsafe path is rejected");

        assert!(matches!(error, RestorePlanError::UnsafeManifestPath { .. }));
        assert!(!fixture.target.exists());
    }

    #[test]
    fn rejects_interior_current_directory_segments_before_normalization() {
        let fixture = RestoreFixture::new();
        let blob = fixture.cache.write_bytes(b"tampered").expect("blob writes");
        let snapshot = fixture.snapshot(vec![included_file(
            "src/./main.rs",
            blob.id().clone(),
            blob.object_ref(),
            8,
        )]);

        let error =
            RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
                .expect_err("current directory segment is rejected");

        assert!(matches!(error, RestorePlanError::UnsafeManifestPath { .. }));
        assert!(!fixture.target.exists());
    }

    #[test]
    fn rejects_empty_path_segments_before_normalization() {
        let fixture = RestoreFixture::new();
        let blob = fixture.cache.write_bytes(b"tampered").expect("blob writes");
        let snapshot = fixture.snapshot(vec![included_file(
            "src//main.rs",
            blob.id().clone(),
            blob.object_ref(),
            8,
        )]);

        let error =
            RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
                .expect_err("empty path segment is rejected");

        assert!(matches!(error, RestorePlanError::UnsafeManifestPath { .. }));
        assert!(!fixture.target.exists());
    }

    #[test]
    fn accepts_normal_nested_paths_after_raw_segment_validation() {
        let fixture = RestoreFixture::new();
        let blob = fixture.cache.write_bytes(b"normal").expect("blob writes");
        let snapshot = fixture.snapshot(vec![included_file(
            "src/main.rs",
            blob.id().clone(),
            blob.object_ref(),
            6,
        )]);

        let plan = RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
            .expect("normal nested path is accepted");

        assert_eq!(plan.files_to_write()[0].path, PathBuf::from("src/main.rs"));
        assert_eq!(plan.dirs_to_create(), &[PathBuf::from("src")]);
        assert!(plan.apply_allowed());
    }

    #[test]
    fn missing_blob_blocks_apply_without_partial_writes() {
        let fixture = RestoreFixture::new();
        let missing_id = blob_id_for(b"missing");
        let present = fixture.cache.write_bytes(b"present").expect("blob writes");
        let snapshot = fixture.snapshot(vec![
            included_file("present.txt", present.id().clone(), present.object_ref(), 7),
            included_file("missing.txt", missing_id, "blobs/b3/missing".to_string(), 7),
        ]);

        let plan = RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
            .expect("plan builds");

        assert!(!plan.apply_allowed());
        assert_eq!(plan.missing_blobs().len(), 1);
        let error = RestoreMaterializer::new(fixture.cache.clone())
            .apply(&plan)
            .expect_err("missing blob blocks apply");

        assert!(matches!(error, RestorePlanError::ApplyNotAllowed { .. }));
        assert!(!fixture.target.exists());
    }

    #[test]
    fn skipped_policy_and_unsupported_entries_are_not_materialized() {
        let fixture = RestoreFixture::new();
        let blob = fixture.cache.write_bytes(b"hello").expect("blob writes");
        let snapshot = fixture.snapshot(vec![
            included_file("README.md", blob.id().clone(), blob.object_ref(), 5),
            excluded_dir("node_modules", "generated Node dependency directory"),
            deferred_symlink("linked.txt", "symlink capture is deferred"),
            included_unsupported("socket"),
        ]);

        let plan = RestorePlan::from_persisted_snapshot(&snapshot, &fixture.cache, &fixture.target)
            .expect("plan builds");
        RestoreMaterializer::new(fixture.cache.clone())
            .apply(&plan)
            .expect("restore applies");

        assert_eq!(plan.skipped_entries().len(), 3);
        assert!(fixture.target.join("README.md").is_file());
        assert!(!fixture.target.join("node_modules").exists());
        assert!(!fixture.target.join("linked.txt").exists());
        assert!(!fixture.target.join("socket").exists());
    }

    struct RestoreFixture {
        _dir: tempfile::TempDir,
        cache: BlobCache,
        target: PathBuf,
    }

    impl RestoreFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let cache = BlobCache::open(dir.path().join("cache")).expect("cache opens");
            let target = dir.path().join("target");

            Self {
                _dir: dir,
                cache,
                target,
            }
        }

        fn snapshot(&self, entries: Vec<ManifestEntryRecord>) -> PersistedSnapshot {
            PersistedSnapshot {
                project: ProjectRecord {
                    id: "project-1".to_string(),
                    root_path: "/source".to_string(),
                    kind: "local".to_string(),
                    display_name: "source".to_string(),
                    discovered_at: "2026-06-18T10:00:00Z".to_string(),
                },
                snapshot: SnapshotRecord {
                    id: "snapshot-1".to_string(),
                    project_id: "project-1".to_string(),
                    parent_snapshot_id: None,
                    created_at: "2026-06-18T10:01:00Z".to_string(),
                    reason: "manual".to_string(),
                    manifest_entry_count: entries.len() as u64,
                    total_size_bytes: entries.iter().map(|entry| entry.size_bytes).sum(),
                },
                entries,
            }
        }
    }

    fn included_dir(path: &str) -> ManifestEntryRecord {
        entry(
            path,
            ManifestEntryKind::Directory,
            0,
            None,
            None,
            PolicyDecision::Include,
        )
    }

    fn included_file(
        path: &str,
        blob_id: BlobId,
        object_ref: String,
        size_bytes: u64,
    ) -> ManifestEntryRecord {
        entry(
            path,
            ManifestEntryKind::File,
            size_bytes,
            Some(blob_id),
            Some(object_ref),
            PolicyDecision::Include,
        )
    }

    fn excluded_dir(path: &str, reason: &str) -> ManifestEntryRecord {
        entry(
            path,
            ManifestEntryKind::Directory,
            0,
            None,
            None,
            PolicyDecision::Exclude {
                reason: reason.to_string(),
            },
        )
    }

    fn deferred_symlink(path: &str, reason: &str) -> ManifestEntryRecord {
        entry(
            path,
            ManifestEntryKind::Symlink,
            0,
            None,
            None,
            PolicyDecision::RequiresUserDecision {
                reason: reason.to_string(),
            },
        )
    }

    fn included_unsupported(path: &str) -> ManifestEntryRecord {
        entry(
            path,
            ManifestEntryKind::Unsupported,
            0,
            None,
            None,
            PolicyDecision::Include,
        )
    }

    fn entry(
        path: &str,
        kind: ManifestEntryKind,
        size_bytes: u64,
        blob_id: Option<BlobId>,
        object_ref: Option<String>,
        policy_decision: PolicyDecision,
    ) -> ManifestEntryRecord {
        ManifestEntryRecord {
            relative_path: PathBuf::from(path),
            kind,
            size_bytes,
            blob_id,
            object_ref,
            policy_decision,
        }
    }

    fn blob_id_for(content: &[u8]) -> BlobId {
        BlobId::from_blake3_hex(blake3::hash(content).to_hex().to_string())
            .expect("BLAKE3 returns valid blob ids")
    }
}
