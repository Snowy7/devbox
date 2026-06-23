//! Local divergent-snapshot conflict comparison domain.

use bindhub_core::{BlobId, ManifestEntryKind, PolicyDecision};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparableSnapshot {
    project_id: String,
    snapshot_id: String,
    entries: Vec<ComparableEntry>,
}

impl ComparableSnapshot {
    pub fn new(
        project_id: impl Into<String>,
        snapshot_id: impl Into<String>,
        entries: Vec<ComparableEntry>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            snapshot_id: snapshot_id.into(),
            entries,
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn snapshot_id(&self) -> &str {
        &self.snapshot_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparableEntry {
    path: PathBuf,
    kind: ManifestEntryKind,
    size_bytes: u64,
    blob_id: Option<BlobId>,
    object_ref: Option<String>,
    policy_decision: PolicyDecision,
}

impl ComparableEntry {
    pub fn new(
        path: impl Into<PathBuf>,
        kind: ManifestEntryKind,
        size_bytes: u64,
        blob_id: Option<BlobId>,
        object_ref: Option<String>,
        policy_decision: PolicyDecision,
    ) -> Self {
        Self {
            path: path.into(),
            kind,
            size_bytes,
            blob_id,
            object_ref,
            policy_decision,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PathComparisonState {
    Same,
    LocalOnly,
    IncomingOnly,
    LocalDeleted,
    IncomingDeleted,
    BothModifiedSame,
    BothModifiedDifferent,
    PolicyExcluded,
    PolicyDeferred,
    PolicyBlocked,
    Unsupported,
}

impl PathComparisonState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::LocalOnly => "local-only",
            Self::IncomingOnly => "incoming-only",
            Self::LocalDeleted => "local-deleted",
            Self::IncomingDeleted => "incoming-deleted",
            Self::BothModifiedSame => "both-modified-same",
            Self::BothModifiedDifferent => "both-modified-different",
            Self::PolicyExcluded => "policy-excluded",
            Self::PolicyDeferred => "policy-deferred",
            Self::PolicyBlocked => "policy-blocked",
            Self::Unsupported => "unsupported",
        }
    }
}

impl fmt::Display for PathComparisonState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathComparisonRow {
    path: PathBuf,
    state: PathComparisonState,
    entry_kind: ManifestEntryKind,
    base_blob_id: Option<BlobId>,
    local_blob_id: Option<BlobId>,
    incoming_blob_id: Option<BlobId>,
    base_size_bytes: Option<u64>,
    local_size_bytes: Option<u64>,
    incoming_size_bytes: Option<u64>,
    local_policy_decision: Option<PolicyDecision>,
    incoming_policy_decision: Option<PolicyDecision>,
}

impl PathComparisonRow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        path: PathBuf,
        state: PathComparisonState,
        entry_kind: ManifestEntryKind,
        base: Option<&ComparableEntry>,
        local: Option<&ComparableEntry>,
        incoming: Option<&ComparableEntry>,
    ) -> Self {
        Self {
            path,
            state,
            entry_kind,
            base_blob_id: base.and_then(|entry| entry.blob_id.clone()),
            local_blob_id: local.and_then(|entry| entry.blob_id.clone()),
            incoming_blob_id: incoming.and_then(|entry| entry.blob_id.clone()),
            base_size_bytes: base.map(|entry| entry.size_bytes),
            local_size_bytes: local.map(|entry| entry.size_bytes),
            incoming_size_bytes: incoming.map(|entry| entry.size_bytes),
            local_policy_decision: local.map(|entry| entry.policy_decision.clone()),
            incoming_policy_decision: incoming.map(|entry| entry.policy_decision.clone()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn state(&self) -> PathComparisonState {
        self.state
    }

    pub fn entry_kind(&self) -> &ManifestEntryKind {
        &self.entry_kind
    }

    pub fn base_blob_id(&self) -> Option<&BlobId> {
        self.base_blob_id.as_ref()
    }

    pub fn local_blob_id(&self) -> Option<&BlobId> {
        self.local_blob_id.as_ref()
    }

    pub fn incoming_blob_id(&self) -> Option<&BlobId> {
        self.incoming_blob_id.as_ref()
    }

    pub fn base_size_bytes(&self) -> Option<u64> {
        self.base_size_bytes
    }

    pub fn local_size_bytes(&self) -> Option<u64> {
        self.local_size_bytes
    }

    pub fn incoming_size_bytes(&self) -> Option<u64> {
        self.incoming_size_bytes
    }

    pub fn local_policy_decision(&self) -> Option<&PolicyDecision> {
        self.local_policy_decision.as_ref()
    }

    pub fn incoming_policy_decision(&self) -> Option<&PolicyDecision> {
        self.incoming_policy_decision.as_ref()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictSummary {
    same: usize,
    local_only: usize,
    incoming_only: usize,
    local_deleted: usize,
    incoming_deleted: usize,
    both_modified_same: usize,
    both_modified_different: usize,
    policy_excluded: usize,
    policy_deferred: usize,
    policy_blocked: usize,
    unsupported: usize,
}

impl ConflictSummary {
    pub fn from_rows(rows: &[PathComparisonRow]) -> Self {
        let mut summary = Self::default();
        for row in rows {
            match row.state {
                PathComparisonState::Same => summary.same += 1,
                PathComparisonState::LocalOnly => summary.local_only += 1,
                PathComparisonState::IncomingOnly => summary.incoming_only += 1,
                PathComparisonState::LocalDeleted => summary.local_deleted += 1,
                PathComparisonState::IncomingDeleted => summary.incoming_deleted += 1,
                PathComparisonState::BothModifiedSame => summary.both_modified_same += 1,
                PathComparisonState::BothModifiedDifferent => summary.both_modified_different += 1,
                PathComparisonState::PolicyExcluded => summary.policy_excluded += 1,
                PathComparisonState::PolicyDeferred => summary.policy_deferred += 1,
                PathComparisonState::PolicyBlocked => summary.policy_blocked += 1,
                PathComparisonState::Unsupported => summary.unsupported += 1,
            }
        }
        summary
    }

    pub fn total(&self) -> usize {
        self.same
            + self.local_only
            + self.incoming_only
            + self.local_deleted
            + self.incoming_deleted
            + self.both_modified_same
            + self.both_modified_different
            + self.policy_excluded
            + self.policy_deferred
            + self.policy_blocked
            + self.unsupported
    }

    pub fn same(&self) -> usize {
        self.same
    }

    pub fn local_only(&self) -> usize {
        self.local_only
    }

    pub fn incoming_only(&self) -> usize {
        self.incoming_only
    }

    pub fn local_deleted(&self) -> usize {
        self.local_deleted
    }

    pub fn incoming_deleted(&self) -> usize {
        self.incoming_deleted
    }

    pub fn both_modified_same(&self) -> usize {
        self.both_modified_same
    }

    pub fn both_modified_different(&self) -> usize {
        self.both_modified_different
    }

    pub fn policy_excluded(&self) -> usize {
        self.policy_excluded
    }

    pub fn policy_deferred(&self) -> usize {
        self.policy_deferred
    }

    pub fn policy_blocked(&self) -> usize {
        self.policy_blocked
    }

    pub fn unsupported(&self) -> usize {
        self.unsupported
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotComparison {
    conflict_id: String,
    project_id: String,
    base_snapshot_id: Option<String>,
    local_snapshot_id: String,
    incoming_snapshot_id: String,
    rows: Vec<PathComparisonRow>,
    summary: ConflictSummary,
}

impl SnapshotComparison {
    pub fn conflict_id(&self) -> &str {
        &self.conflict_id
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn base_snapshot_id(&self) -> Option<&str> {
        self.base_snapshot_id.as_deref()
    }

    pub fn local_snapshot_id(&self) -> &str {
        &self.local_snapshot_id
    }

    pub fn incoming_snapshot_id(&self) -> &str {
        &self.incoming_snapshot_id
    }

    pub fn rows(&self) -> &[PathComparisonRow] {
        &self.rows
    }

    pub fn summary(&self) -> &ConflictSummary {
        &self.summary
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictCompareError {
    ProjectMismatch {
        local_project_id: String,
        incoming_project_id: String,
    },
    BaseProjectMismatch {
        base_project_id: String,
        project_id: String,
    },
}

impl fmt::Display for ConflictCompareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProjectMismatch {
                local_project_id,
                incoming_project_id,
            } => write!(
                f,
                "cannot compare snapshots from different projects: local={local_project_id}, incoming={incoming_project_id}"
            ),
            Self::BaseProjectMismatch {
                base_project_id,
                project_id,
            } => write!(
                f,
                "base snapshot project {base_project_id} does not match compared project {project_id}"
            ),
        }
    }
}

impl std::error::Error for ConflictCompareError {}

pub fn compare_snapshots(
    base: Option<&ComparableSnapshot>,
    local: &ComparableSnapshot,
    incoming: &ComparableSnapshot,
) -> Result<SnapshotComparison, ConflictCompareError> {
    if local.project_id != incoming.project_id {
        return Err(ConflictCompareError::ProjectMismatch {
            local_project_id: local.project_id.clone(),
            incoming_project_id: incoming.project_id.clone(),
        });
    }

    if let Some(base) = base {
        if base.project_id != local.project_id {
            return Err(ConflictCompareError::BaseProjectMismatch {
                base_project_id: base.project_id.clone(),
                project_id: local.project_id.clone(),
            });
        }
    }

    let base_entries = base.map(index_entries).unwrap_or_default();
    let local_entries = index_entries(local);
    let incoming_entries = index_entries(incoming);
    let paths = base_entries
        .keys()
        .chain(local_entries.keys())
        .chain(incoming_entries.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    let rows = paths
        .into_iter()
        .map(|path| {
            let base = base_entries.get(&path);
            let local = local_entries.get(&path);
            let incoming = incoming_entries.get(&path);
            let base = base.copied();
            let local = local.copied();
            let incoming = incoming.copied();
            let state = classify_path(base, local, incoming);
            let entry_kind = local
                .or(incoming)
                .or(base)
                .map(|entry| entry.kind.clone())
                .expect("path came from at least one entry");

            PathComparisonRow::new(path, state, entry_kind, base, local, incoming)
        })
        .collect::<Vec<_>>();
    let summary = ConflictSummary::from_rows(&rows);
    let conflict_id = stable_conflict_id(
        &local.project_id,
        base.map(ComparableSnapshot::snapshot_id),
        local.snapshot_id(),
        incoming.snapshot_id(),
    );

    Ok(SnapshotComparison {
        conflict_id,
        project_id: local.project_id.clone(),
        base_snapshot_id: base.map(|snapshot| snapshot.snapshot_id.clone()),
        local_snapshot_id: local.snapshot_id.clone(),
        incoming_snapshot_id: incoming.snapshot_id.clone(),
        rows,
        summary,
    })
}

pub fn stable_conflict_id(
    project_id: &str,
    base_snapshot_id: Option<&str>,
    local_snapshot_id: &str,
    incoming_snapshot_id: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"bindhub-conflict-v1\n");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(base_snapshot_id.unwrap_or("-").as_bytes());
    hasher.update(b"\n");
    hasher.update(local_snapshot_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(incoming_snapshot_id.as_bytes());
    format!("conflict-b3-{}", hasher.finalize().to_hex())
}

fn index_entries(snapshot: &ComparableSnapshot) -> BTreeMap<PathBuf, &ComparableEntry> {
    snapshot
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect()
}

fn classify_path(
    base: Option<&ComparableEntry>,
    local: Option<&ComparableEntry>,
    incoming: Option<&ComparableEntry>,
) -> PathComparisonState {
    if [base, local, incoming]
        .into_iter()
        .flatten()
        .any(|entry| entry.kind == ManifestEntryKind::Unsupported)
    {
        return PathComparisonState::Unsupported;
    }

    if [base, local, incoming]
        .into_iter()
        .flatten()
        .any(|entry| is_secret_blocked(&entry.policy_decision))
    {
        return PathComparisonState::PolicyBlocked;
    }

    if [base, local, incoming].into_iter().flatten().any(|entry| {
        matches!(
            entry.policy_decision,
            PolicyDecision::RequiresUserDecision { .. }
        )
    }) {
        return PathComparisonState::PolicyDeferred;
    }

    if [base, local, incoming]
        .into_iter()
        .flatten()
        .any(|entry| matches!(entry.policy_decision, PolicyDecision::Exclude { .. }))
    {
        return PathComparisonState::PolicyExcluded;
    }

    match (base, local, incoming) {
        (_, Some(local), Some(incoming)) if same_identity(local, incoming) => {
            if base.is_some_and(|base| !same_identity(base, local)) {
                PathComparisonState::BothModifiedSame
            } else {
                PathComparisonState::Same
            }
        }
        (_, Some(_), Some(_)) => PathComparisonState::BothModifiedDifferent,
        (Some(_), None, Some(_)) => PathComparisonState::LocalDeleted,
        (Some(_), Some(_), None) => PathComparisonState::IncomingDeleted,
        (Some(_), None, None) => PathComparisonState::Same,
        (None, Some(_), None) => PathComparisonState::LocalOnly,
        (None, None, Some(_)) => PathComparisonState::IncomingOnly,
        (None, None, None) => PathComparisonState::Same,
    }
}

fn same_identity(left: &ComparableEntry, right: &ComparableEntry) -> bool {
    left.kind == right.kind
        && left.size_bytes == right.size_bytes
        && left.blob_id == right.blob_id
        && left.object_ref == right.object_ref
        && left.policy_decision == right.policy_decision
}

fn is_secret_blocked(policy: &PolicyDecision) -> bool {
    matches!(
        policy,
        PolicyDecision::RequiresUserDecision { reason }
            if reason.starts_with("secret blocked by policy rule ")
    )
}

pub fn path_to_conflict_string(path: &Path) -> String {
    let parts = path
        .components()
        .map(|component| match component {
            Component::Prefix(prefix) => prefix.as_os_str().to_string_lossy().into_owned(),
            Component::RootDir => String::new(),
            Component::Normal(part) => part.to_string_lossy().into_owned(),
            Component::CurDir => ".".to_string(),
            Component::ParentDir => "..".to_string(),
        })
        .collect::<Vec<_>>();

    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_same_local_only_incoming_only_and_modified_rows() {
        let base = snapshot(
            "project-1",
            "base",
            [
                file("both-change-same.txt", "old", 3),
                file("different.txt", "old", 3),
                file("incoming-deleted.txt", "old", 3),
                file("local-deleted.txt", "old", 3),
                file("same.txt", "same", 4),
            ],
        );
        let local = snapshot(
            "project-1",
            "local",
            [
                file("both-change-same.txt", "new", 3),
                file("different.txt", "local", 5),
                file("incoming-deleted.txt", "old", 3),
                file("local-only.txt", "local", 5),
                file("same.txt", "same", 4),
            ],
        );
        let incoming = snapshot(
            "project-1",
            "incoming",
            [
                file("both-change-same.txt", "new", 3),
                file("different.txt", "incoming", 8),
                file("incoming-only.txt", "incoming", 8),
                file("local-deleted.txt", "old", 3),
                file("same.txt", "same", 4),
            ],
        );

        let comparison = compare_snapshots(Some(&base), &local, &incoming).expect("compares");

        assert_eq!(
            comparison
                .rows()
                .iter()
                .map(|row| (path_to_conflict_string(row.path()), row.state()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "both-change-same.txt".to_string(),
                    PathComparisonState::BothModifiedSame
                ),
                (
                    "different.txt".to_string(),
                    PathComparisonState::BothModifiedDifferent
                ),
                (
                    "incoming-deleted.txt".to_string(),
                    PathComparisonState::IncomingDeleted
                ),
                (
                    "incoming-only.txt".to_string(),
                    PathComparisonState::IncomingOnly
                ),
                (
                    "local-deleted.txt".to_string(),
                    PathComparisonState::LocalDeleted
                ),
                ("local-only.txt".to_string(), PathComparisonState::LocalOnly),
                ("same.txt".to_string(), PathComparisonState::Same),
            ]
        );
        assert_eq!(comparison.summary().both_modified_different(), 1);
    }

    #[test]
    fn policy_blocked_deferred_excluded_and_unsupported_are_not_normal_file_conflicts() {
        let local = snapshot(
            "project-1",
            "local",
            [
                entry(
                    "node_modules",
                    ManifestEntryKind::Directory,
                    0,
                    None,
                    PolicyDecision::Exclude {
                        reason: "generated Node dependency directory".to_string(),
                    },
                ),
                entry(
                    "secret.env",
                    ManifestEntryKind::File,
                    40,
                    None,
                    PolicyDecision::RequiresUserDecision {
                        reason:
                            "secret blocked by policy rule openai_api_key at line 1: sk-<redacted>"
                                .to_string(),
                    },
                ),
                entry(
                    "linked.txt",
                    ManifestEntryKind::Symlink,
                    0,
                    None,
                    PolicyDecision::RequiresUserDecision {
                        reason: "symlink capture is deferred until restore safety rules exist"
                            .to_string(),
                    },
                ),
                entry(
                    "socket",
                    ManifestEntryKind::Unsupported,
                    0,
                    None,
                    PolicyDecision::RequiresUserDecision {
                        reason: "unsupported filesystem node".to_string(),
                    },
                ),
            ],
        );
        let incoming = snapshot("project-1", "incoming", []);

        let comparison = compare_snapshots(None, &local, &incoming).expect("compares");

        assert_eq!(
            comparison
                .rows()
                .iter()
                .map(|row| (path_to_conflict_string(row.path()), row.state()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "linked.txt".to_string(),
                    PathComparisonState::PolicyDeferred
                ),
                (
                    "node_modules".to_string(),
                    PathComparisonState::PolicyExcluded
                ),
                ("secret.env".to_string(), PathComparisonState::PolicyBlocked),
                ("socket".to_string(), PathComparisonState::Unsupported),
            ]
        );
    }

    #[test]
    fn rejects_cross_project_compare() {
        let local = snapshot("project-a", "local", []);
        let incoming = snapshot("project-b", "incoming", []);

        assert!(matches!(
            compare_snapshots(None, &local, &incoming),
            Err(ConflictCompareError::ProjectMismatch { .. })
        ));
    }

    fn snapshot<const N: usize>(
        project_id: &str,
        snapshot_id: &str,
        entries: [ComparableEntry; N],
    ) -> ComparableSnapshot {
        ComparableSnapshot::new(project_id, snapshot_id, entries.into_iter().collect())
    }

    fn file(path: &str, identity: &str, size: u64) -> ComparableEntry {
        let digest = blake3::hash(identity.as_bytes()).to_hex().to_string();
        entry(
            path,
            ManifestEntryKind::File,
            size,
            Some(BlobId::from_blake3_hex(digest).expect("BLAKE3 produces valid blob identifiers")),
            PolicyDecision::Include,
        )
    }

    fn entry(
        path: &str,
        kind: ManifestEntryKind,
        size_bytes: u64,
        blob_id: Option<BlobId>,
        policy_decision: PolicyDecision,
    ) -> ComparableEntry {
        let object_ref = blob_id
            .as_ref()
            .map(|blob_id| format!("blobs/b3/{}", blob_id.as_str()));
        ComparableEntry::new(path, kind, size_bytes, blob_id, object_ref, policy_decision)
    }
}
