//! SQLite-backed local metadata boundary for Devbox.

mod blob_cache;

pub use blob_cache::{BlobCache, BlobCacheError, BlobCacheResult, BlobRef};

use rusqlite::{params, Connection, OptionalExtension};
use std::fmt;
use std::path::Path;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

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
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::UnsupportedSchemaVersion { .. } | Self::InvalidMigrationState(_) => None,
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

    fn count(summary: &SchemaSummary, table: &str) -> u64 {
        summary
            .tables
            .iter()
            .find(|entry| entry.table == table)
            .expect("table is present")
            .rows
    }
}
