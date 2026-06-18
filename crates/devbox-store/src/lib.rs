//! SQLite-backed local metadata boundary for Devbox.

mod blob_cache;

pub use blob_cache::{BlobCache, BlobCacheError, BlobCacheResult, BlobRef};

use devbox_core::{BlobId, DomainIdError, ManifestEntryKind, PolicyDecision, ProjectId};
use rusqlite::{params, Connection, OptionalExtension};
use std::fmt;
use std::path::{Component, Path, PathBuf};

pub const CURRENT_SCHEMA_VERSION: u32 = 2;

const SUMMARY_TABLES: &[&str] = &[
    "projects",
    "snapshots",
    "manifest_entries",
    "blobs",
    "chunks",
    "operations",
    "policies",
    "policy_evaluations",
    "restore_attempts",
];

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    InvalidMigrationState(String),
    InvalidSnapshotDraft(String),
    InvalidStoredValue(String),
    DuplicateSnapshotId(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "{error}"),
            Self::UnsupportedSchemaVersion { found, supported } => write!(
                f,
                "SQLite schema version {found} is newer than supported version {supported}"
            ),
            Self::InvalidMigrationState(message) => f.write_str(message),
            Self::InvalidSnapshotDraft(message) => f.write_str(message),
            Self::InvalidStoredValue(message) => f.write_str(message),
            Self::DuplicateSnapshotId(id) => write!(f, "snapshot already exists: {id}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::UnsupportedSchemaVersion { .. }
            | Self::InvalidMigrationState(_)
            | Self::InvalidSnapshotDraft(_)
            | Self::InvalidStoredValue(_)
            | Self::DuplicateSnapshotId(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open_in_memory() -> StoreResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub fn open_file(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    fn from_connection(conn: Connection) -> StoreResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let enabled: u8 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        if enabled != 1 {
            return Err(StoreError::InvalidMigrationState(
                "SQLite foreign key enforcement is disabled".to_string(),
            ));
        }

        Ok(Self { conn })
    }

    pub fn apply_migrations(&self) -> StoreResult<()> {
        let version = self.schema_version()?;
        if version > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchemaVersion {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }

        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );
            "#,
        )?;

        if version < 1 {
            self.conn.execute_batch(INITIAL_SCHEMA)?;
        }

        if version < 2 {
            self.conn.execute_batch(MIGRATION_2_MANIFEST_UNSUPPORTED)?;
        }

        Ok(())
    }

    pub fn schema_version(&self) -> StoreResult<u32> {
        let version: u32 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(version)
    }

    pub fn schema_summary(&self) -> StoreResult<SchemaSummary> {
        let version = self.schema_version()?;
        let tables = SUMMARY_TABLES
            .iter()
            .map(|table| self.table_count(table))
            .collect::<StoreResult<Vec<_>>>()?;

        Ok(SchemaSummary { version, tables })
    }

    fn table_count(&self, table: &str) -> StoreResult<TableCount> {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let rows = self.conn.query_row(&sql, [], |row| row.get(0))?;

        Ok(TableCount {
            table: table.to_string(),
            rows,
        })
    }

    pub fn insert_project(&self, project: &NewProject<'_>) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO projects (id, root_path, kind, display_name, discovered_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                project.id,
                project.root_path,
                project.kind,
                project.display_name,
                project.discovered_at,
            ],
        )?;

        Ok(())
    }

    pub fn upsert_project(&self, project: &NewProject<'_>) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO projects (id, root_path, kind, display_name, discovered_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                root_path = excluded.root_path,
                kind = excluded.kind,
                display_name = excluded.display_name
            "#,
            params![
                project.id,
                project.root_path,
                project.kind,
                project.display_name,
                project.discovered_at,
            ],
        )?;

        Ok(())
    }

    pub fn project(&self, id: &str) -> StoreResult<Option<ProjectRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT id, root_path, kind, display_name, discovered_at
                FROM projects
                WHERE id = ?1
                "#,
                params![id],
                |row| {
                    Ok(ProjectRecord {
                        id: row.get(0)?,
                        root_path: row.get(1)?,
                        kind: row.get(2)?,
                        display_name: row.get(3)?,
                        discovered_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn insert_snapshot(&self, snapshot: &NewSnapshot<'_>) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO snapshots (
                id,
                project_id,
                parent_snapshot_id,
                created_at,
                reason,
                manifest_entry_count,
                total_size_bytes
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                snapshot.id,
                snapshot.project_id,
                snapshot.parent_snapshot_id,
                snapshot.created_at,
                snapshot.reason,
                snapshot.manifest_entry_count,
                snapshot.total_size_bytes,
            ],
        )?;

        Ok(())
    }

    pub fn snapshot(&self, id: &str) -> StoreResult<Option<SnapshotRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    id,
                    project_id,
                    parent_snapshot_id,
                    created_at,
                    reason,
                    manifest_entry_count,
                    total_size_bytes
                FROM snapshots
                WHERE id = ?1
                "#,
                params![id],
                |row| {
                    Ok(SnapshotRecord {
                        id: row.get(0)?,
                        project_id: row.get(1)?,
                        parent_snapshot_id: row.get(2)?,
                        created_at: row.get(3)?,
                        reason: row.get(4)?,
                        manifest_entry_count: row.get(5)?,
                        total_size_bytes: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn current_timestamp(&self) -> StoreResult<String> {
        self.conn
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
                row.get(0)
            })
            .map_err(StoreError::from)
    }

    pub fn persist_draft_snapshot(
        &mut self,
        draft: &NewSnapshotDraft<'_>,
    ) -> StoreResult<PersistedSnapshot> {
        if self.snapshot(draft.snapshot.id)?.is_some() {
            return Err(StoreError::DuplicateSnapshotId(
                draft.snapshot.id.to_string(),
            ));
        }

        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO projects (id, root_path, kind, display_name, discovered_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                root_path = excluded.root_path,
                kind = excluded.kind,
                display_name = excluded.display_name
            "#,
            params![
                draft.project.id,
                draft.project.root_path,
                draft.project.kind,
                draft.project.display_name,
                draft.project.discovered_at,
            ],
        )?;

        tx.execute(
            r#"
            INSERT INTO snapshots (
                id,
                project_id,
                parent_snapshot_id,
                created_at,
                reason,
                manifest_entry_count,
                total_size_bytes
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                draft.snapshot.id,
                draft.snapshot.project_id,
                draft.snapshot.parent_snapshot_id,
                draft.snapshot.created_at,
                draft.snapshot.reason,
                draft.snapshot.manifest_entry_count,
                draft.snapshot.total_size_bytes,
            ],
        )?;

        for (index, entry) in draft.entries.iter().enumerate() {
            let path = path_to_store_string(entry.relative_path);
            let (decision, reason) = policy_to_store(entry.policy_decision);

            if let Some(blob_id) = entry.blob_id {
                let object_ref = entry.object_ref.ok_or_else(|| {
                    StoreError::InvalidSnapshotDraft(format!(
                        "manifest entry {path} has blob id {blob_id} without an object ref"
                    ))
                })?;

                tx.execute(
                    r#"
                    INSERT INTO blobs (id, hash_algorithm, size_bytes, object_ref, created_at)
                    VALUES (?1, 'blake3', ?2, ?3, ?4)
                    ON CONFLICT(id) DO UPDATE SET
                        size_bytes = excluded.size_bytes,
                        object_ref = excluded.object_ref
                    "#,
                    params![
                        blob_id.as_str(),
                        entry.size_bytes,
                        object_ref,
                        draft.snapshot.created_at,
                    ],
                )?;
            }

            tx.execute(
                r#"
                INSERT INTO manifest_entries (
                    snapshot_id,
                    path,
                    entry_kind,
                    blob_id,
                    target_path,
                    file_mode,
                    size_bytes,
                    policy_decision,
                    policy_reason
                )
                VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, ?6, ?7)
                "#,
                params![
                    draft.snapshot.id,
                    path,
                    kind_to_store(&entry.kind),
                    entry.blob_id.map(BlobId::as_str),
                    entry.size_bytes,
                    decision,
                    reason,
                ],
            )?;

            if !matches!(entry.policy_decision, PolicyDecision::Include) {
                tx.execute(
                    r#"
                    INSERT INTO policy_evaluations (
                        id,
                        policy_id,
                        project_id,
                        snapshot_id,
                        path,
                        decision,
                        reason,
                        evaluated_at
                    )
                    VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7)
                    "#,
                    params![
                        format!("policy-eval-{}-{index}", draft.snapshot.id),
                        draft.snapshot.project_id,
                        draft.snapshot.id,
                        path,
                        decision,
                        reason,
                        draft.snapshot.created_at,
                    ],
                )?;
            }
        }

        tx.commit()?;
        self.snapshot_with_entries(draft.snapshot.id)?
            .ok_or_else(|| {
                StoreError::InvalidMigrationState("persisted snapshot is missing".into())
            })
    }

    pub fn list_snapshots(&self) -> StoreResult<Vec<SnapshotListRecord>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT
                s.id,
                s.project_id,
                p.root_path,
                p.display_name,
                s.created_at,
                s.manifest_entry_count,
                s.total_size_bytes
            FROM snapshots s
            JOIN projects p ON p.id = s.project_id
            ORDER BY s.created_at DESC, s.id ASC
            "#,
        )?;

        let rows = statement.query_map([], |row| {
            Ok(SnapshotListRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                project_root_path: row.get(2)?,
                project_display_name: row.get(3)?,
                created_at: row.get(4)?,
                manifest_entry_count: row.get(5)?,
                total_size_bytes: row.get(6)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn snapshot_with_entries(&self, id: &str) -> StoreResult<Option<PersistedSnapshot>> {
        let Some(snapshot) = self.snapshot(id)? else {
            return Ok(None);
        };
        let project = self.project(&snapshot.project_id)?.ok_or_else(|| {
            StoreError::InvalidStoredValue(format!(
                "snapshot {} references missing project {}",
                snapshot.id, snapshot.project_id
            ))
        })?;
        let entries = self.manifest_entries(id)?;

        Ok(Some(PersistedSnapshot {
            project,
            snapshot,
            entries,
        }))
    }

    fn manifest_entries(&self, snapshot_id: &str) -> StoreResult<Vec<ManifestEntryRecord>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT
                me.path,
                me.entry_kind,
                me.blob_id,
                b.object_ref,
                me.size_bytes,
                me.policy_decision,
                me.policy_reason
            FROM manifest_entries me
            LEFT JOIN blobs b ON b.id = me.blob_id
            WHERE me.snapshot_id = ?1
            ORDER BY me.id ASC
            "#,
        )?;

        let rows = statement.query_map(params![snapshot_id], |row| {
            Ok(RawManifestEntryRecord {
                relative_path: PathBuf::from(row.get::<_, String>(0)?),
                kind: row.get(1)?,
                blob_id: row.get(2)?,
                object_ref: row.get(3)?,
                size_bytes: row.get(4)?,
                policy_decision: row.get(5)?,
                policy_reason: row.get(6)?,
            })
        })?;

        rows.map(|row| {
            let record = row?;
            let blob_id = record
                .blob_id
                .map(BlobId::from_blake3_hex)
                .transpose()
                .map_err(|error| invalid_domain_value("blob id", error))?;

            Ok(ManifestEntryRecord {
                relative_path: record.relative_path,
                kind: kind_from_store(&record.kind)?,
                size_bytes: record.size_bytes,
                blob_id,
                object_ref: record.object_ref,
                policy_decision: policy_from_store(record.policy_decision, record.policy_reason)?,
            })
        })
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSummary {
    pub version: u32,
    pub tables: Vec<TableCount>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCount {
    pub table: String,
    pub rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProject<'a> {
    pub id: &'a str,
    pub root_path: &'a str,
    pub kind: &'a str,
    pub display_name: &'a str,
    pub discovered_at: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: String,
    pub root_path: String,
    pub kind: String,
    pub display_name: String,
    pub discovered_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSnapshot<'a> {
    pub id: &'a str,
    pub project_id: &'a str,
    pub parent_snapshot_id: Option<&'a str>,
    pub created_at: &'a str,
    pub reason: &'a str,
    pub manifest_entry_count: u64,
    pub total_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRecord {
    pub id: String,
    pub project_id: String,
    pub parent_snapshot_id: Option<String>,
    pub created_at: String,
    pub reason: String,
    pub manifest_entry_count: u64,
    pub total_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSnapshotDraft<'a> {
    pub project: NewProject<'a>,
    pub snapshot: NewSnapshot<'a>,
    pub entries: Vec<NewSnapshotManifestEntry<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSnapshotManifestEntry<'a> {
    pub relative_path: &'a Path,
    pub kind: ManifestEntryKind,
    pub size_bytes: u64,
    pub blob_id: Option<&'a BlobId>,
    pub object_ref: Option<&'a str>,
    pub policy_decision: &'a PolicyDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotListRecord {
    pub id: String,
    pub project_id: String,
    pub project_root_path: String,
    pub project_display_name: String,
    pub created_at: String,
    pub manifest_entry_count: u64,
    pub total_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSnapshot {
    pub project: ProjectRecord,
    pub snapshot: SnapshotRecord,
    pub entries: Vec<ManifestEntryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntryRecord {
    pub relative_path: PathBuf,
    pub kind: ManifestEntryKind,
    pub size_bytes: u64,
    pub blob_id: Option<BlobId>,
    pub object_ref: Option<String>,
    pub policy_decision: PolicyDecision,
}

#[derive(Debug)]
struct RawManifestEntryRecord {
    relative_path: PathBuf,
    kind: String,
    size_bytes: u64,
    blob_id: Option<String>,
    object_ref: Option<String>,
    policy_decision: String,
    policy_reason: Option<String>,
}

pub fn local_project_id(root_path: impl AsRef<Path>) -> ProjectId {
    let path = root_path.as_ref();
    let identity = path_to_store_string(path);
    let digest = blake3::hash(identity.as_bytes()).to_hex().to_string();
    ProjectId::new(format!("project-local-b3-{digest}"))
        .expect("local project ids are generated from a non-empty prefix")
}

pub fn path_to_store_string(path: &Path) -> String {
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

fn kind_to_store(kind: &ManifestEntryKind) -> &'static str {
    match kind {
        ManifestEntryKind::File => "file",
        ManifestEntryKind::Directory => "directory",
        ManifestEntryKind::Symlink => "symlink",
        ManifestEntryKind::Unsupported => "unsupported",
    }
}

fn kind_from_store(value: &str) -> StoreResult<ManifestEntryKind> {
    match value {
        "file" => Ok(ManifestEntryKind::File),
        "directory" => Ok(ManifestEntryKind::Directory),
        "symlink" => Ok(ManifestEntryKind::Symlink),
        "unsupported" => Ok(ManifestEntryKind::Unsupported),
        _ => Err(StoreError::InvalidStoredValue(format!(
            "unknown manifest entry kind: {value}"
        ))),
    }
}

fn policy_to_store(policy: &PolicyDecision) -> (&'static str, Option<&str>) {
    match policy {
        PolicyDecision::Include => ("include", None),
        PolicyDecision::Exclude { reason } => ("exclude", Some(reason.as_str())),
        PolicyDecision::RequiresUserDecision { reason } => {
            ("requires_user_decision", Some(reason.as_str()))
        }
    }
}

fn policy_from_store(decision: String, reason: Option<String>) -> StoreResult<PolicyDecision> {
    match decision.as_str() {
        "include" => Ok(PolicyDecision::Include),
        "exclude" => Ok(PolicyDecision::Exclude {
            reason: reason.unwrap_or_default(),
        }),
        "requires_user_decision" => Ok(PolicyDecision::RequiresUserDecision {
            reason: reason.unwrap_or_default(),
        }),
        _ => Err(StoreError::InvalidStoredValue(format!(
            "unknown policy decision: {decision}"
        ))),
    }
}

fn invalid_domain_value(field: &str, error: DomainIdError) -> StoreError {
    StoreError::InvalidStoredValue(format!("invalid stored {field}: {error}"))
}

const INITIAL_SCHEMA: &str = r#"
BEGIN;

CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    root_path TEXT NOT NULL,
    kind TEXT NOT NULL,
    display_name TEXT NOT NULL,
    discovered_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshots (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    parent_snapshot_id TEXT REFERENCES snapshots(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    reason TEXT NOT NULL,
    manifest_entry_count INTEGER NOT NULL DEFAULT 0 CHECK (manifest_entry_count >= 0),
    total_size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (total_size_bytes >= 0)
);

CREATE TABLE IF NOT EXISTS blobs (
    id TEXT PRIMARY KEY,
    hash_algorithm TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    object_ref TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    blob_id TEXT NOT NULL REFERENCES blobs(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    chunk_hash TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    object_ref TEXT NOT NULL,
    UNIQUE (blob_id, chunk_index)
);

CREATE TABLE IF NOT EXISTS manifest_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('file', 'directory', 'symlink')),
    blob_id TEXT REFERENCES blobs(id) ON DELETE RESTRICT,
    target_path TEXT,
    file_mode INTEGER,
    size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    policy_decision TEXT NOT NULL CHECK (
        policy_decision IN ('include', 'exclude', 'requires_user_decision')
    ),
    policy_reason TEXT,
    UNIQUE (snapshot_id, path)
);

CREATE TABLE IF NOT EXISTS operations (
    id TEXT PRIMARY KEY,
    project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
    snapshot_id TEXT REFERENCES snapshots(id) ON DELETE SET NULL,
    operation_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    details_json TEXT
);

CREATE TABLE IF NOT EXISTS policies (
    id TEXT PRIMARY KEY,
    project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    policy_kind TEXT NOT NULL,
    body_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS policy_evaluations (
    id TEXT PRIMARY KEY,
    policy_id TEXT REFERENCES policies(id) ON DELETE SET NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    snapshot_id TEXT REFERENCES snapshots(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('include', 'exclude', 'requires_user_decision')),
    reason TEXT,
    evaluated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS restore_attempts (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE RESTRICT,
    status TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    completed_at TEXT,
    target_path TEXT NOT NULL,
    safety_report_json TEXT,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_snapshots_project_created
    ON snapshots(project_id, created_at);
CREATE INDEX IF NOT EXISTS idx_manifest_entries_snapshot_path
    ON manifest_entries(snapshot_id, path);
CREATE INDEX IF NOT EXISTS idx_chunks_blob_index
    ON chunks(blob_id, chunk_index);
CREATE INDEX IF NOT EXISTS idx_operations_project_started
    ON operations(project_id, started_at);
CREATE INDEX IF NOT EXISTS idx_policy_evaluations_project_path
    ON policy_evaluations(project_id, path);
CREATE INDEX IF NOT EXISTS idx_restore_attempts_project_requested
    ON restore_attempts(project_id, requested_at);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (1, 'phase_0_metadata_foundation');

PRAGMA user_version = 1;

COMMIT;
"#;

const MIGRATION_2_MANIFEST_UNSUPPORTED: &str = r#"
BEGIN;

CREATE TABLE manifest_entries_v2 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('file', 'directory', 'symlink', 'unsupported')),
    blob_id TEXT REFERENCES blobs(id) ON DELETE RESTRICT,
    target_path TEXT,
    file_mode INTEGER,
    size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    policy_decision TEXT NOT NULL CHECK (
        policy_decision IN ('include', 'exclude', 'requires_user_decision')
    ),
    policy_reason TEXT,
    UNIQUE (snapshot_id, path)
);

INSERT INTO manifest_entries_v2 (
    id,
    snapshot_id,
    path,
    entry_kind,
    blob_id,
    target_path,
    file_mode,
    size_bytes,
    policy_decision,
    policy_reason
)
SELECT
    id,
    snapshot_id,
    path,
    entry_kind,
    blob_id,
    target_path,
    file_mode,
    size_bytes,
    policy_decision,
    policy_reason
FROM manifest_entries;

DROP TABLE manifest_entries;
ALTER TABLE manifest_entries_v2 RENAME TO manifest_entries;

CREATE INDEX IF NOT EXISTS idx_manifest_entries_snapshot_path
    ON manifest_entries(snapshot_id, path);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (2, 'manifest_entries_support_unsupported_kind');

PRAGMA user_version = 2;

COMMIT;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_migration_creates_initial_schema() {
        let store = Store::open_in_memory().expect("store opens");

        assert_eq!(store.schema_version().expect("version reads"), 0);

        store.apply_migrations().expect("migrations apply");
        let summary = store.schema_summary().expect("summary reads");

        assert_eq!(summary.version, CURRENT_SCHEMA_VERSION);
        assert_eq!(summary.tables.len(), SUMMARY_TABLES.len());
        assert!(summary.tables.iter().all(|table| table.rows == 0));
    }

    #[test]
    fn migration_is_idempotent() {
        let store = Store::open_in_memory().expect("store opens");

        store.apply_migrations().expect("first migration applies");
        store.apply_migrations().expect("second migration applies");

        let version = store.schema_version().expect("version reads");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn file_backed_store_persists_schema_version() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("devbox.sqlite3");

        {
            let store = Store::open_file(&path).expect("store opens");
            store.apply_migrations().expect("migrations apply");
        }

        let reopened = Store::open_file(&path).expect("store reopens");
        assert_eq!(
            reopened.schema_version().expect("version reads"),
            CURRENT_SCHEMA_VERSION
        );
    }

    #[test]
    fn project_and_snapshot_metadata_round_trip() {
        let store = migrated_store();

        store
            .insert_project(&NewProject {
                id: "project-1",
                root_path: "/workspace/devbox",
                kind: "rust",
                display_name: "devbox",
                discovered_at: "2026-06-18T10:00:00Z",
            })
            .expect("project inserts");

        store
            .insert_snapshot(&NewSnapshot {
                id: "snapshot-1",
                project_id: "project-1",
                parent_snapshot_id: None,
                created_at: "2026-06-18T10:01:00Z",
                reason: "manual",
                manifest_entry_count: 3,
                total_size_bytes: 42,
            })
            .expect("snapshot inserts");

        let project = store
            .project("project-1")
            .expect("project query works")
            .expect("project exists");
        assert_eq!(project.root_path, "/workspace/devbox");
        assert_eq!(project.kind, "rust");

        let snapshot = store
            .snapshot("snapshot-1")
            .expect("snapshot query works")
            .expect("snapshot exists");
        assert_eq!(snapshot.project_id, "project-1");
        assert_eq!(snapshot.manifest_entry_count, 3);
        assert_eq!(snapshot.total_size_bytes, 42);

        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "projects"), 1);
        assert_eq!(count(&summary, "snapshots"), 1);
    }

    #[test]
    fn draft_snapshot_persistence_round_trips_manifest_metadata() {
        let mut store = migrated_store();
        let blob_id = blob_id_for(b"hello");
        let include = PolicyDecision::Include;
        let src_path = Path::new("src");
        let file_path = Path::new("src/main.rs");
        let entries = vec![
            NewSnapshotManifestEntry {
                relative_path: src_path,
                kind: ManifestEntryKind::Directory,
                size_bytes: 0,
                blob_id: None,
                object_ref: None,
                policy_decision: &include,
            },
            NewSnapshotManifestEntry {
                relative_path: file_path,
                kind: ManifestEntryKind::File,
                size_bytes: 5,
                blob_id: Some(&blob_id),
                object_ref: Some("blobs/b3/ea/8f/object"),
                policy_decision: &include,
            },
        ];

        store
            .persist_draft_snapshot(&draft("snapshot-1", &entries))
            .expect("draft persists");

        let persisted = store
            .snapshot_with_entries("snapshot-1")
            .expect("snapshot loads")
            .expect("snapshot exists");

        assert_eq!(persisted.project.root_path, "/workspace/devbox");
        assert_eq!(persisted.snapshot.manifest_entry_count, 2);
        assert_eq!(persisted.snapshot.total_size_bytes, 5);
        assert_eq!(
            persisted
                .entries
                .iter()
                .map(|entry| entry.relative_path.clone())
                .collect::<Vec<_>>(),
            vec![PathBuf::from("src"), PathBuf::from("src/main.rs")]
        );
        assert_eq!(persisted.entries[1].blob_id, Some(blob_id));
        assert_eq!(
            persisted.entries[1].object_ref.as_deref(),
            Some("blobs/b3/ea/8f/object")
        );
    }

    #[test]
    fn policy_exclusions_and_deferred_entries_round_trip() {
        let mut store = migrated_store();
        let excluded = PolicyDecision::Exclude {
            reason: "generated dependency directory".to_string(),
        };
        let deferred = PolicyDecision::RequiresUserDecision {
            reason: "symlink capture is deferred until restore safety rules exist".to_string(),
        };
        let node_modules_path = Path::new("node_modules");
        let symlink_path = Path::new("linked.txt");
        let entries = vec![
            NewSnapshotManifestEntry {
                relative_path: node_modules_path,
                kind: ManifestEntryKind::Directory,
                size_bytes: 0,
                blob_id: None,
                object_ref: None,
                policy_decision: &excluded,
            },
            NewSnapshotManifestEntry {
                relative_path: symlink_path,
                kind: ManifestEntryKind::Symlink,
                size_bytes: 0,
                blob_id: None,
                object_ref: None,
                policy_decision: &deferred,
            },
        ];

        store
            .persist_draft_snapshot(&draft("snapshot-policy", &entries))
            .expect("draft persists");

        let persisted = store
            .snapshot_with_entries("snapshot-policy")
            .expect("snapshot loads")
            .expect("snapshot exists");

        assert_eq!(persisted.entries[0].policy_decision, excluded);
        assert_eq!(persisted.entries[1].policy_decision, deferred);

        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "policy_evaluations"), 2);
    }

    #[test]
    fn duplicate_snapshot_id_is_reported_clearly() {
        let mut store = migrated_store();
        let include = PolicyDecision::Include;
        let readme_path = Path::new("README.md");
        let entries = vec![NewSnapshotManifestEntry {
            relative_path: readme_path,
            kind: ManifestEntryKind::File,
            size_bytes: 0,
            blob_id: None,
            object_ref: None,
            policy_decision: &include,
        }];

        store
            .persist_draft_snapshot(&draft("snapshot-duplicate", &entries))
            .expect("first draft persists");
        let error = store
            .persist_draft_snapshot(&draft("snapshot-duplicate", &entries))
            .expect_err("duplicate snapshot id fails");

        assert_eq!(
            error.to_string(),
            "snapshot already exists: snapshot-duplicate"
        );
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let store = migrated_store();

        let result = store.insert_snapshot(&NewSnapshot {
            id: "snapshot-without-project",
            project_id: "missing-project",
            parent_snapshot_id: None,
            created_at: "2026-06-18T10:01:00Z",
            reason: "manual",
            manifest_entry_count: 0,
            total_size_bytes: 0,
        });

        assert!(matches!(
            result,
            Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
                error,
                _
            ))) if error.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }

    fn migrated_store() -> Store {
        let store = Store::open_in_memory().expect("store opens");
        store.apply_migrations().expect("migrations apply");
        store
    }

    fn draft<'a>(
        snapshot_id: &'a str,
        entries: &'a [NewSnapshotManifestEntry<'a>],
    ) -> NewSnapshotDraft<'a> {
        NewSnapshotDraft {
            project: NewProject {
                id: "project-1",
                root_path: "/workspace/devbox",
                kind: "Rust",
                display_name: "devbox",
                discovered_at: "2026-06-18T10:00:00Z",
            },
            snapshot: NewSnapshot {
                id: snapshot_id,
                project_id: "project-1",
                parent_snapshot_id: None,
                created_at: "2026-06-18T10:01:00Z",
                reason: "manual",
                manifest_entry_count: entries.len() as u64,
                total_size_bytes: entries.iter().map(|entry| entry.size_bytes).sum(),
            },
            entries: entries.to_vec(),
        }
    }

    fn blob_id_for(content: &[u8]) -> BlobId {
        BlobId::from_blake3_hex(blake3::hash(content).to_hex().to_string())
            .expect("BLAKE3 returns valid blob ids")
    }

    fn count(summary: &SchemaSummary, table: &str) -> u64 {
        summary
            .tables
            .iter()
            .find(|entry| entry.table == table)
            .expect("table is present")
            .rows
    }
}
