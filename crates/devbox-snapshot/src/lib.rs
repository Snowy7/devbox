//! Snapshot manifest construction over local project files.

mod restore;

use devbox_core::scanner::evaluate_directory_policy;
use devbox_core::{BlobId, ManifestEntryKind, PolicyDecision, SnapshotId};
use devbox_store::{
    path_to_store_string, BlobCache, BlobCacheError, LocalChangeKind, ManifestEntryRecord,
    PersistedSnapshot,
};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, DirEntry, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};

pub use restore::{
    RestoreMaterializer, RestoreMissingBlob, RestorePlan, RestorePlanError, RestorePlanSummary,
    RestoreSkippedEntry, RestoreTargetStatus, RestoreWrite,
};

const MANIFEST_ID_PREFIX: &str = "snapshot-draft-b3-";

#[derive(Debug, Clone)]
pub struct SnapshotManifestBuilder {
    blob_cache: BlobCache,
}

impl SnapshotManifestBuilder {
    pub fn new(blob_cache: BlobCache) -> Self {
        Self { blob_cache }
    }

    pub fn build_draft(&self, root: impl AsRef<Path>) -> Result<DraftSnapshot, SnapshotError> {
        let root = root.as_ref();
        if !root.exists() {
            return Err(SnapshotError::RootNotFound {
                path: root.to_path_buf(),
            });
        }

        if !root.is_dir() {
            return Err(SnapshotError::RootNotDirectory {
                path: root.to_path_buf(),
            });
        }

        let root = canonical_root(root)?;
        let cache_root =
            fs::canonicalize(self.blob_cache.root()).map_err(|source| SnapshotError::Io {
                path: self.blob_cache.root().to_path_buf(),
                source,
            })?;
        if cache_root == root || cache_root.starts_with(&root) {
            return Err(SnapshotError::BlobCacheInsideSnapshotRoot {
                cache_root,
                snapshot_root: root,
            });
        }

        let mut entries = Vec::new();
        walk_directory(&self.blob_cache, &root, &root, &mut entries)?;
        let summary = SnapshotSummary::from_entries(&entries);
        let id = stable_snapshot_id(&entries);

        Ok(DraftSnapshot {
            id,
            root,
            entries,
            summary,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftSnapshot {
    id: SnapshotId,
    root: PathBuf,
    entries: Vec<SnapshotManifestEntry>,
    summary: SnapshotSummary,
}

impl DraftSnapshot {
    pub fn id(&self) -> &SnapshotId {
        &self.id
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entries(&self) -> &[SnapshotManifestEntry] {
        &self.entries
    }

    pub fn summary(&self) -> &SnapshotSummary {
        &self.summary
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotManifestEntry {
    relative_path: PathBuf,
    kind: ManifestEntryKind,
    size_bytes: Option<u64>,
    blob_id: Option<BlobId>,
    object_ref: Option<String>,
    policy_decision: PolicyDecision,
}

impl SnapshotManifestEntry {
    fn new(
        relative_path: PathBuf,
        kind: ManifestEntryKind,
        size_bytes: Option<u64>,
        blob_id: Option<BlobId>,
        object_ref: Option<String>,
        policy_decision: PolicyDecision,
    ) -> Self {
        Self {
            relative_path,
            kind,
            size_bytes,
            blob_id,
            object_ref,
            policy_decision,
        }
    }

    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    pub fn kind(&self) -> &ManifestEntryKind {
        &self.kind
    }

    pub fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }

    pub fn blob_id(&self) -> Option<&BlobId> {
        self.blob_id.as_ref()
    }

    pub fn object_ref(&self) -> Option<&str> {
        self.object_ref.as_deref()
    }

    pub fn policy_decision(&self) -> &PolicyDecision {
        &self.policy_decision
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSummary {
    total_entries: usize,
    included_files: usize,
    included_directories: usize,
    included_symlinks: usize,
    excluded_entries: usize,
    total_file_bytes: u64,
}

impl SnapshotSummary {
    fn from_entries(entries: &[SnapshotManifestEntry]) -> Self {
        let mut summary = Self {
            total_entries: entries.len(),
            included_files: 0,
            included_directories: 0,
            included_symlinks: 0,
            excluded_entries: 0,
            total_file_bytes: 0,
        };

        for entry in entries {
            match entry.policy_decision() {
                PolicyDecision::Include => match entry.kind() {
                    ManifestEntryKind::File => {
                        summary.included_files += 1;
                        summary.total_file_bytes += entry.size_bytes().unwrap_or_default();
                    }
                    ManifestEntryKind::Directory => summary.included_directories += 1,
                    ManifestEntryKind::Symlink => summary.included_symlinks += 1,
                    ManifestEntryKind::Unsupported => {}
                },
                PolicyDecision::Exclude { .. } => summary.excluded_entries += 1,
                PolicyDecision::RequiresUserDecision { .. } => {}
            }
        }

        summary
    }

    pub fn total_entries(&self) -> usize {
        self.total_entries
    }

    pub fn included_files(&self) -> usize {
        self.included_files
    }

    pub fn included_directories(&self) -> usize {
        self.included_directories
    }

    pub fn included_symlinks(&self) -> usize {
        self.included_symlinks
    }

    pub fn excluded_entries(&self) -> usize {
        self.excluded_entries
    }

    pub fn total_file_bytes(&self) -> u64 {
        self.total_file_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalChangeFeedDiff {
    base_snapshot_id: Option<String>,
    changes: Vec<LocalChange>,
    summary: LocalChangeSummary,
}

impl LocalChangeFeedDiff {
    pub fn compare(base: Option<&PersistedSnapshot>, current: &DraftSnapshot) -> Self {
        let base_snapshot_id = base.map(|snapshot| snapshot.snapshot.id.clone());
        let base_files = base
            .map(|snapshot| {
                snapshot
                    .entries
                    .iter()
                    .filter(|entry| stored_included_file(entry))
                    .map(|entry| (entry.relative_path.clone(), entry))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let current_files = current
            .entries()
            .iter()
            .filter(|entry| draft_included_file(entry))
            .map(|entry| (entry.relative_path().to_path_buf(), entry))
            .collect::<BTreeMap<_, _>>();

        let mut changes = Vec::new();
        let mut summary = LocalChangeSummary {
            skipped_deferred: current
                .entries()
                .iter()
                .filter(|entry| skipped_or_deferred(entry))
                .count(),
            ..LocalChangeSummary::default()
        };

        for (path, current_entry) in &current_files {
            match base_files.get(path) {
                Some(base_entry) if same_file_identity(base_entry, current_entry) => {
                    summary.unchanged += 1;
                }
                Some(base_entry) => {
                    summary.modified += 1;
                    summary.bytes_to_upload += current_entry.size_bytes().unwrap_or_default();
                    changes.push(LocalChange::modified(base_entry, current_entry));
                }
                None => {
                    summary.created += 1;
                    summary.bytes_to_upload += current_entry.size_bytes().unwrap_or_default();
                    changes.push(LocalChange::created(current_entry));
                }
            }
        }

        for (path, base_entry) in &base_files {
            if !current_files.contains_key(path) {
                summary.deleted += 1;
                summary.bytes_deleted += base_entry.size_bytes;
                changes.push(LocalChange::deleted(base_entry));
            }
        }

        changes.sort_by(|left, right| {
            left.relative_path
                .cmp(&right.relative_path)
                .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
        });

        Self {
            base_snapshot_id,
            changes,
            summary,
        }
    }

    pub fn base_snapshot_id(&self) -> Option<&str> {
        self.base_snapshot_id.as_deref()
    }

    pub fn changes(&self) -> &[LocalChange] {
        &self.changes
    }

    pub fn summary(&self) -> &LocalChangeSummary {
        &self.summary
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalChange {
    relative_path: PathBuf,
    kind: LocalChangeKind,
    previous_blob_id: Option<BlobId>,
    blob_id: Option<BlobId>,
    object_ref: Option<String>,
    size_bytes: u64,
}

impl LocalChange {
    fn created(entry: &SnapshotManifestEntry) -> Self {
        Self {
            relative_path: entry.relative_path().to_path_buf(),
            kind: LocalChangeKind::Created,
            previous_blob_id: None,
            blob_id: entry.blob_id().cloned(),
            object_ref: entry.object_ref().map(ToString::to_string),
            size_bytes: entry.size_bytes().unwrap_or_default(),
        }
    }

    fn modified(base: &ManifestEntryRecord, entry: &SnapshotManifestEntry) -> Self {
        Self {
            relative_path: entry.relative_path().to_path_buf(),
            kind: LocalChangeKind::Modified,
            previous_blob_id: base.blob_id.clone(),
            blob_id: entry.blob_id().cloned(),
            object_ref: entry.object_ref().map(ToString::to_string),
            size_bytes: entry.size_bytes().unwrap_or_default(),
        }
    }

    fn deleted(base: &ManifestEntryRecord) -> Self {
        Self {
            relative_path: base.relative_path.clone(),
            kind: LocalChangeKind::Deleted,
            previous_blob_id: base.blob_id.clone(),
            blob_id: None,
            object_ref: None,
            size_bytes: base.size_bytes,
        }
    }

    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    pub fn kind(&self) -> &LocalChangeKind {
        &self.kind
    }

    pub fn previous_blob_id(&self) -> Option<&BlobId> {
        self.previous_blob_id.as_ref()
    }

    pub fn blob_id(&self) -> Option<&BlobId> {
        self.blob_id.as_ref()
    }

    pub fn object_ref(&self) -> Option<&str> {
        self.object_ref.as_deref()
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn stable_id(&self, project_id: &str, base_snapshot_id: Option<&str>) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"devbox-pending-local-change-v1\n");
        hasher.update(project_id.as_bytes());
        hasher.update(b"\n");
        hasher.update(base_snapshot_id.unwrap_or("-").as_bytes());
        hasher.update(b"\n");
        hasher.update(path_to_store_string(self.relative_path()).as_bytes());
        hasher.update(b"\n");
        hasher.update(self.kind.as_str().as_bytes());
        hasher.update(b"\n");
        hasher.update(
            self.previous_blob_id()
                .map(BlobId::as_str)
                .unwrap_or("-")
                .as_bytes(),
        );
        hasher.update(b"\n");
        hasher.update(self.blob_id().map(BlobId::as_str).unwrap_or("-").as_bytes());
        let digest = hasher.finalize().to_hex().to_string();

        format!("pending-change-b3-{digest}")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalChangeSummary {
    created: usize,
    modified: usize,
    deleted: usize,
    unchanged: usize,
    skipped_deferred: usize,
    bytes_to_upload: u64,
    bytes_deleted: u64,
}

impl LocalChangeSummary {
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

    pub fn skipped_deferred(&self) -> usize {
        self.skipped_deferred
    }

    pub fn bytes_to_upload(&self) -> u64 {
        self.bytes_to_upload
    }

    pub fn bytes_deleted(&self) -> u64 {
        self.bytes_deleted
    }
}

#[derive(Debug)]
pub enum SnapshotError {
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
    BlobCache {
        path: PathBuf,
        source: BlobCacheError,
    },
    BlobCacheInsideSnapshotRoot {
        cache_root: PathBuf,
        snapshot_root: PathBuf,
    },
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootNotFound { path } => {
                write!(f, "snapshot root does not exist: {}", path.display())
            }
            Self::RootNotDirectory { path } => {
                write!(f, "snapshot root is not a directory: {}", path.display())
            }
            Self::Io { path, source } => {
                write!(f, "could not inspect {}: {source}", path.display())
            }
            Self::BlobCache { path, source } => {
                write!(f, "could not cache {}: {source}", path.display())
            }
            Self::BlobCacheInsideSnapshotRoot {
                cache_root,
                snapshot_root,
            } => write!(
                f,
                "blob cache root {} is inside snapshot root {}; choose a cache outside the project",
                cache_root.display(),
                snapshot_root.display()
            ),
        }
    }
}

impl std::error::Error for SnapshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::BlobCache { source, .. } => Some(source),
            Self::RootNotFound { .. }
            | Self::RootNotDirectory { .. }
            | Self::BlobCacheInsideSnapshotRoot { .. } => None,
        }
    }
}

fn walk_directory(
    blob_cache: &BlobCache,
    root: &Path,
    path: &Path,
    entries: &mut Vec<SnapshotManifestEntry>,
) -> Result<(), SnapshotError> {
    let mut children = fs::read_dir(path)
        .map_err(|source| SnapshotError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| SnapshotError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    children.sort_by_key(DirEntry::file_name);

    for child in children {
        let child_path = child.path();
        let metadata = fs::symlink_metadata(&child_path).map_err(|source| SnapshotError::Io {
            path: child_path.clone(),
            source,
        })?;
        let relative_path = relative_to(root, &child_path);
        let kind = entry_kind(&metadata);
        let policy_decision = match kind {
            ManifestEntryKind::Directory => evaluate_directory_policy(&relative_path),
            ManifestEntryKind::File => PolicyDecision::Include,
            ManifestEntryKind::Symlink => PolicyDecision::RequiresUserDecision {
                reason: "symlink capture is deferred until restore safety rules exist".to_string(),
            },
            ManifestEntryKind::Unsupported => PolicyDecision::RequiresUserDecision {
                reason:
                    "unsupported filesystem node type is deferred until restore safety rules exist"
                        .to_string(),
            },
        };

        if matches!(policy_decision, PolicyDecision::Exclude { .. }) {
            entries.push(SnapshotManifestEntry::new(
                relative_path,
                kind,
                size_for_metadata(&metadata),
                None,
                None,
                policy_decision,
            ));
            continue;
        }

        match kind {
            ManifestEntryKind::File => {
                let blob = blob_cache.write_file(&child_path).map_err(|source| {
                    SnapshotError::BlobCache {
                        path: child_path.clone(),
                        source,
                    }
                })?;
                entries.push(SnapshotManifestEntry::new(
                    relative_path,
                    ManifestEntryKind::File,
                    Some(blob.size_bytes()),
                    Some(blob.id().clone()),
                    Some(blob.object_ref()),
                    PolicyDecision::Include,
                ));
            }
            ManifestEntryKind::Directory => {
                entries.push(SnapshotManifestEntry::new(
                    relative_path,
                    ManifestEntryKind::Directory,
                    None,
                    None,
                    None,
                    PolicyDecision::Include,
                ));
                walk_directory(blob_cache, root, &child_path, entries)?;
            }
            ManifestEntryKind::Symlink => {
                entries.push(SnapshotManifestEntry::new(
                    relative_path,
                    ManifestEntryKind::Symlink,
                    None,
                    None,
                    None,
                    policy_decision,
                ));
            }
            ManifestEntryKind::Unsupported => {
                entries.push(SnapshotManifestEntry::new(
                    relative_path,
                    ManifestEntryKind::Unsupported,
                    None,
                    None,
                    None,
                    policy_decision,
                ));
            }
        }
    }

    Ok(())
}

fn entry_kind(metadata: &Metadata) -> ManifestEntryKind {
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        ManifestEntryKind::Symlink
    } else if file_type.is_dir() {
        ManifestEntryKind::Directory
    } else if file_type.is_file() {
        ManifestEntryKind::File
    } else {
        ManifestEntryKind::Unsupported
    }
}

fn size_for_metadata(metadata: &Metadata) -> Option<u64> {
    if metadata.is_file() {
        Some(metadata.len())
    } else {
        None
    }
}

fn stable_snapshot_id(entries: &[SnapshotManifestEntry]) -> SnapshotId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"devbox-snapshot-manifest-v1\n");
    for entry in entries {
        hasher.update(canonical_entry(entry).as_bytes());
    }

    let digest = hasher.finalize().to_hex().to_string();
    SnapshotId::new(format!("{MANIFEST_ID_PREFIX}{digest}"))
        .expect("stable draft snapshot ids are non-empty")
}

fn canonical_entry(entry: &SnapshotManifestEntry) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        kind_name(entry.kind()),
        path_to_manifest_string(entry.relative_path()),
        entry
            .size_bytes()
            .map(|size| size.to_string())
            .unwrap_or_else(|| "-".to_string()),
        entry
            .blob_id()
            .map(ToString::to_string)
            .unwrap_or_else(|| "-".to_string()),
        entry.object_ref().unwrap_or("-"),
        policy_to_manifest_string(entry.policy_decision())
    )
}

fn stored_included_file(entry: &ManifestEntryRecord) -> bool {
    entry.kind == ManifestEntryKind::File && entry.policy_decision == PolicyDecision::Include
}

fn draft_included_file(entry: &SnapshotManifestEntry) -> bool {
    entry.kind() == &ManifestEntryKind::File && entry.policy_decision() == &PolicyDecision::Include
}

fn skipped_or_deferred(entry: &SnapshotManifestEntry) -> bool {
    !matches!(entry.policy_decision(), PolicyDecision::Include)
        || matches!(
            entry.kind(),
            ManifestEntryKind::Symlink | ManifestEntryKind::Unsupported
        )
}

fn same_file_identity(base: &ManifestEntryRecord, current: &SnapshotManifestEntry) -> bool {
    base.blob_id.as_ref() == current.blob_id()
        && base.size_bytes == current.size_bytes().unwrap_or_default()
}

fn kind_name(kind: &ManifestEntryKind) -> &'static str {
    match kind {
        ManifestEntryKind::File => "file",
        ManifestEntryKind::Directory => "directory",
        ManifestEntryKind::Symlink => "symlink",
        ManifestEntryKind::Unsupported => "unsupported",
    }
}

fn policy_to_manifest_string(policy: &PolicyDecision) -> String {
    match policy {
        PolicyDecision::Include => "include".to_string(),
        PolicyDecision::Exclude { reason } => format!("exclude:{reason}"),
        PolicyDecision::RequiresUserDecision { reason } => {
            format!("requires_user_decision:{reason}")
        }
    }
}

fn path_to_manifest_string(path: &Path) -> String {
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

fn relative_to(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn canonical_root(root: &Path) -> Result<PathBuf, SnapshotError> {
    fs::canonicalize(root).map_err(|source| SnapshotError::Io {
        path: root.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use devbox_store::{BlobCache, ProjectRecord, SnapshotRecord};
    use std::fs;

    #[test]
    fn builds_manifest_in_deterministic_order() {
        let fixture = TestProject::new();
        fixture.write("z-last.txt", "z");
        fixture.write("a-first.txt", "a");
        fixture.mkdir("src");
        fixture.write("src/lib.rs", "pub fn lib() {}\n");

        let first = fixture.build();
        let second = fixture.build();

        assert_eq!(
            paths(&first),
            vec!["a-first.txt", "src", "src/lib.rs", "z-last.txt"]
        );
        assert_eq!(paths(&first), paths(&second));
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn excludes_generated_directories_without_caching_descendants() {
        let fixture = TestProject::new();
        fixture.write("src/main.rs", "fn main() {}\n");
        fixture.mkdir("node_modules/left-pad");
        fixture.write("node_modules/left-pad/index.js", "module.exports = true;\n");
        fixture.mkdir(".git/objects");
        fixture.write(".git/objects/ignored", "git object\n");

        let snapshot = fixture.build();

        assert_eq!(
            paths(&snapshot),
            vec![".git", "node_modules", "src", "src/main.rs"]
        );
        assert!(excluded(&snapshot).contains(&(
            ".git".to_string(),
            "Git metadata is handled by the Git adapter".to_string()
        )));
        assert!(excluded(&snapshot).contains(&(
            "node_modules".to_string(),
            "generated Node dependency directory".to_string()
        )));
        assert_eq!(snapshot.summary().included_files(), 1);
        assert_eq!(snapshot.summary().excluded_entries(), 2);
        assert_eq!(fixture.object_file_count(), 1);
    }

    #[test]
    fn includes_regular_files_named_like_generated_directories() {
        let fixture = TestProject::new();
        fixture.write("build", "regular file named build\n");

        let snapshot = fixture.build();
        let entry = snapshot
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new("build"))
            .expect("build file entry exists");

        assert_eq!(entry.kind(), &ManifestEntryKind::File);
        assert_eq!(entry.policy_decision(), &PolicyDecision::Include);
        assert!(entry.blob_id().is_some());
        assert_eq!(fixture.object_file_count(), 1);
    }

    #[test]
    fn rejects_blob_cache_inside_snapshot_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).expect("project dir creates");
        fs::write(root.join("main.rs"), "fn main() {}\n").expect("file writes");
        let cache = BlobCache::open(root.join(".devbox-cache")).expect("cache opens");

        let error = SnapshotManifestBuilder::new(cache)
            .build_draft(&root)
            .expect_err("cache inside snapshot root is rejected");

        assert!(matches!(
            error,
            SnapshotError::BlobCacheInsideSnapshotRoot { .. }
        ));
    }

    #[test]
    fn writes_included_file_bytes_to_blob_cache() {
        let fixture = TestProject::new();
        fixture.write("README.md", "hello snapshot\n");

        let snapshot = fixture.build();
        let entry = snapshot
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new("README.md"))
            .expect("README entry exists");

        let blob_id = entry.blob_id().expect("file has blob id");
        assert_eq!(entry.size_bytes(), Some("hello snapshot\n".len() as u64));
        assert!(entry
            .object_ref()
            .expect("file has object ref")
            .starts_with("blobs/b3/"));
        assert_eq!(
            fixture.cache.read(blob_id).expect("blob reads"),
            b"hello snapshot\n"
        );
    }

    #[test]
    fn records_empty_directories_without_blob_refs() {
        let fixture = TestProject::new();
        fixture.mkdir("empty");

        let snapshot = fixture.build();
        let entry = snapshot
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new("empty"))
            .expect("empty directory entry exists");

        assert_eq!(entry.kind(), &ManifestEntryKind::Directory);
        assert_eq!(entry.blob_id(), None);
        assert_eq!(entry.object_ref(), None);
        assert_eq!(entry.policy_decision(), &PolicyDecision::Include);
    }

    #[test]
    fn symlinks_are_deferred_without_blob_cache_writes() {
        let fixture = TestProject::new();
        fixture.write("real.txt", "real content\n");
        if !fixture.symlink_file("real.txt", "linked.txt") {
            return;
        }

        let snapshot = fixture.build();
        let entry = snapshot
            .entries()
            .iter()
            .find(|entry| entry.relative_path() == Path::new("linked.txt"))
            .expect("symlink entry exists");

        assert_eq!(entry.kind(), &ManifestEntryKind::Symlink);
        assert!(matches!(
            entry.policy_decision(),
            PolicyDecision::RequiresUserDecision { reason }
                if reason == "symlink capture is deferred until restore safety rules exist"
        ));
        assert_eq!(entry.blob_id(), None);
        assert_eq!(fixture.object_file_count(), 1);
    }

    #[test]
    fn stable_identity_changes_when_file_content_changes() {
        let fixture = TestProject::new();
        fixture.write("app.txt", "first");
        let first = fixture.build();

        fixture.write("app.txt", "second");
        let second = fixture.build();

        assert_ne!(first.id(), second.id());
        assert_eq!(first.summary().included_files(), 1);
        assert_eq!(second.summary().included_files(), 1);
    }

    #[test]
    fn diff_reports_created_modified_deleted_and_unchanged_files() {
        let fixture = TestProject::new();
        fixture.write("created.txt", "new\n");
        fixture.write("deleted.txt", "gone\n");
        fixture.write("same.txt", "same\n");
        fixture.write("src/main.rs", "before\n");
        let base = persisted_from_draft(&fixture.build());

        fs::remove_file(fixture.root.join("deleted.txt")).expect("delete fixture file");
        fixture.write("src/main.rs", "after\n");
        let current = fixture.build();

        let diff = LocalChangeFeedDiff::compare(Some(&base), &current);

        assert_eq!(diff.base_snapshot_id(), Some(base.snapshot.id.as_str()));
        assert_eq!(diff.summary().created(), 0);
        assert_eq!(diff.summary().modified(), 1);
        assert_eq!(diff.summary().deleted(), 1);
        assert_eq!(diff.summary().unchanged(), 2);
        assert_eq!(
            diff.changes()
                .iter()
                .map(|change| {
                    (
                        path_to_manifest_string(change.relative_path()),
                        change.kind().clone(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                ("deleted.txt".to_string(), LocalChangeKind::Deleted),
                ("src/main.rs".to_string(), LocalChangeKind::Modified),
            ]
        );
    }

    #[test]
    fn diff_treats_no_base_snapshot_as_all_current_files_created() {
        let fixture = TestProject::new();
        fixture.write("a.txt", "a");
        fixture.mkdir("empty");
        let current = fixture.build();

        let diff = LocalChangeFeedDiff::compare(None, &current);

        assert_eq!(diff.base_snapshot_id(), None);
        assert_eq!(diff.summary().created(), 1);
        assert_eq!(diff.summary().unchanged(), 0);
        assert_eq!(diff.changes().len(), 1);
        assert_eq!(diff.changes()[0].kind(), &LocalChangeKind::Created);
        assert_eq!(diff.changes()[0].relative_path(), Path::new("a.txt"));
    }

    #[test]
    fn diff_summarizes_policy_and_deferred_entries_without_uploadable_changes() {
        let fixture = TestProject::new();
        fixture.mkdir("node_modules/left-pad");
        fixture.write("node_modules/left-pad/index.js", "module.exports = true;\n");
        fixture.write("real.txt", "real content\n");
        let symlink_created = fixture.symlink_file("real.txt", "linked.txt");
        let current = fixture.build();

        let diff = LocalChangeFeedDiff::compare(None, &current);

        assert_eq!(diff.summary().created(), 1);
        assert_eq!(
            diff.summary().skipped_deferred(),
            if symlink_created { 2 } else { 1 }
        );
        assert_eq!(diff.changes().len(), 1);
        assert_eq!(diff.changes()[0].relative_path(), Path::new("real.txt"));
    }

    struct TestProject {
        _dir: tempfile::TempDir,
        root: PathBuf,
        cache: BlobCache,
    }

    impl TestProject {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let root = dir.path().join("project");
            let cache_root = dir.path().join("cache");
            fs::create_dir_all(&root).expect("project dir creates");
            let cache = BlobCache::open(cache_root).expect("cache opens");

            Self {
                _dir: dir,
                root,
                cache,
            }
        }

        fn build(&self) -> DraftSnapshot {
            SnapshotManifestBuilder::new(self.cache.clone())
                .build_draft(&self.root)
                .expect("snapshot builds")
        }

        fn mkdir(&self, path: &str) {
            fs::create_dir_all(self.root.join(path)).expect("directory creates");
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.root.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent creates");
            }
            fs::write(path, content).expect("file writes");
        }

        #[cfg(unix)]
        fn symlink_file(&self, original: &str, link: &str) -> bool {
            std::os::unix::fs::symlink(self.root.join(original), self.root.join(link)).is_ok()
        }

        #[cfg(windows)]
        fn symlink_file(&self, original: &str, link: &str) -> bool {
            std::os::windows::fs::symlink_file(self.root.join(original), self.root.join(link))
                .is_ok()
        }

        fn object_file_count(&self) -> usize {
            let mut count = 0;
            let mut stack = vec![self.cache.root().join("blobs")];

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

    fn paths(snapshot: &DraftSnapshot) -> Vec<String> {
        snapshot
            .entries()
            .iter()
            .map(|entry| path_to_manifest_string(entry.relative_path()))
            .collect()
    }

    fn excluded(snapshot: &DraftSnapshot) -> Vec<(String, String)> {
        snapshot
            .entries()
            .iter()
            .filter_map(|entry| match entry.policy_decision() {
                PolicyDecision::Exclude { reason } => Some((
                    path_to_manifest_string(entry.relative_path()),
                    reason.to_string(),
                )),
                _ => None,
            })
            .collect()
    }

    fn persisted_from_draft(draft: &DraftSnapshot) -> PersistedSnapshot {
        PersistedSnapshot {
            project: ProjectRecord {
                id: "project-1".to_string(),
                root_path: draft.root().display().to_string(),
                kind: "local".to_string(),
                display_name: "project".to_string(),
                discovered_at: "2026-06-18T10:00:00Z".to_string(),
            },
            snapshot: SnapshotRecord {
                id: draft.id().to_string(),
                project_id: "project-1".to_string(),
                parent_snapshot_id: None,
                created_at: "2026-06-18T10:00:00Z".to_string(),
                reason: "manual".to_string(),
                manifest_entry_count: draft.entries().len() as u64,
                total_size_bytes: draft.summary().total_file_bytes(),
            },
            entries: draft
                .entries()
                .iter()
                .map(|entry| ManifestEntryRecord {
                    relative_path: entry.relative_path().to_path_buf(),
                    kind: entry.kind().clone(),
                    size_bytes: entry.size_bytes().unwrap_or_default(),
                    blob_id: entry.blob_id().cloned(),
                    object_ref: entry.object_ref().map(ToString::to_string),
                    policy_decision: entry.policy_decision().clone(),
                })
                .collect(),
        }
    }
}
