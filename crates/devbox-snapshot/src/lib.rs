//! Snapshot manifest construction over local project files.

mod restore;

use devbox_core::scanner::evaluate_directory_policy;
use devbox_core::{BlobId, ManifestEntryKind, PolicyDecision, SnapshotId};
use devbox_store::{BlobCache, BlobCacheError};
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
    use devbox_store::BlobCache;
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
}
