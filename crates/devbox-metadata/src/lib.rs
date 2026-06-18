//! Hosted metadata API foundation for Phase 1.
//!
//! This crate is intentionally production-shaped but not production-authenticated. It models the
//! hosted metadata service boundary for accounts, devices, projects, published snapshot manifests,
//! and server-side device/project cursors while keeping tests and local development SQLite-only.
//! The `MockDevIdentity` header boundary is for local tests/dev only; production OAuth, account
//! ownership proof, managed object credentials, billing, and deployment hardening remain deferred.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use url::Url;

const MOCK_ACCOUNT_HEADER: &str = "x-devbox-mock-account-id";
const MOCK_DEVICE_HEADER: &str = "x-devbox-mock-device-id";
const SECRET_MARKERS: &[&str] = &[
    "sync_key",
    "sync-key",
    "device_key",
    "device-key",
    "secret",
    "token",
    "credential",
    "r2_",
    "aws_",
    "private_key",
];

pub type MetadataResult<T> = Result<T, MetadataError>;

#[derive(Debug)]
pub enum MetadataError {
    Sqlite(rusqlite::Error),
    PoisonedStore,
    MissingMockDevIdentity,
    IdentityMismatch,
    NotFound { entity: &'static str, id: String },
    CursorPreconditionFailed { current_cursor: Option<String> },
    InvalidRequest(String),
}

impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "{error}"),
            Self::PoisonedStore => f.write_str("metadata store lock is poisoned"),
            Self::MissingMockDevIdentity => {
                write!(
                    f,
                    "mock-dev identity headers are required: {MOCK_ACCOUNT_HEADER}, {MOCK_DEVICE_HEADER}"
                )
            }
            Self::IdentityMismatch => f.write_str("mock-dev identity mismatch"),
            Self::NotFound { entity, id } => write!(f, "{entity} not found: {id}"),
            Self::CursorPreconditionFailed { current_cursor } => write!(
                f,
                "cursor precondition failed; current cursor is {}",
                current_cursor.as_deref().unwrap_or("-")
            ),
            Self::InvalidRequest(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for MetadataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::PoisonedStore
            | Self::MissingMockDevIdentity
            | Self::IdentityMismatch
            | Self::NotFound { .. }
            | Self::CursorPreconditionFailed { .. }
            | Self::InvalidRequest(_) => None,
        }
    }
}

impl From<rusqlite::Error> for MetadataError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl IntoResponse for MetadataError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::MissingMockDevIdentity | Self::IdentityMismatch => StatusCode::UNAUTHORIZED,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::CursorPreconditionFailed { .. } => StatusCode::CONFLICT,
            Self::Sqlite(error) if is_sqlite_constraint(error) => StatusCode::BAD_REQUEST,
            Self::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            Self::Sqlite(_) | Self::PoisonedStore => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = ErrorResponse {
            error: self.public_message(),
        };
        (status, Json(body)).into_response()
    }
}

impl MetadataError {
    fn public_message(&self) -> String {
        match self {
            Self::Sqlite(error) if is_sqlite_constraint(error) => {
                "metadata relationship precondition failed".to_string()
            }
            Self::Sqlite(_) => "metadata storage error".to_string(),
            Self::PoisonedStore => "metadata storage error".to_string(),
            _ => self.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
    pub storage: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockDevIdentity {
    pub account_id: String,
    pub device_id: String,
}

impl MockDevIdentity {
    pub fn from_headers(headers: &HeaderMap) -> MetadataResult<Self> {
        let account_id = required_header(headers, MOCK_ACCOUNT_HEADER)?;
        let device_id = required_header(headers, MOCK_DEVICE_HEADER)?;
        Ok(Self {
            account_id,
            device_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRecord {
    pub account_id: String,
    pub display_name: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub account_id: String,
    pub device_id: String,
    pub display_name: String,
    pub trust_state: String,
    pub last_seen_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub account_id: String,
    pub project_id: String,
    pub display_name: String,
    pub root_hint: String,
    pub project_kind: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedSnapshotRecord {
    pub account_id: String,
    pub project_id: String,
    pub snapshot_id: String,
    pub parent_snapshot_id: Option<String>,
    pub manifest_object_key: String,
    pub manifest_hash: String,
    pub manifest_entry_count: u64,
    pub total_size_bytes: u64,
    pub published_by_device_id: String,
    pub published_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceProjectCursorRecord {
    pub account_id: String,
    pub device_id: String,
    pub project_id: String,
    pub cursor_value: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertDeviceRequest {
    pub account_id: String,
    pub device_id: String,
    pub display_name: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsertProjectRequest {
    pub account_id: String,
    pub project_id: String,
    pub display_name: String,
    pub root_hint: String,
    pub project_kind: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishSnapshotRequest {
    pub account_id: String,
    pub project_id: String,
    pub snapshot_id: String,
    pub parent_snapshot_id: Option<String>,
    pub manifest_object_key: String,
    pub manifest_hash: String,
    pub manifest_entry_count: u64,
    pub total_size_bytes: u64,
    pub published_by_device_id: String,
    pub published_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateCursorRequest {
    pub account_id: String,
    pub device_id: String,
    pub project_id: String,
    pub expected_cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataServiceConfig {
    pub endpoint: String,
    pub auth_mode: MetadataAuthMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetadataAuthMode {
    MockDevHeaders,
}

impl MetadataServiceConfig {
    pub fn validate(&self) -> MetadataResult<MetadataServiceCheck> {
        let endpoint = self.endpoint.trim();
        if endpoint.is_empty() {
            return Err(MetadataError::InvalidRequest(
                "metadata endpoint must not be empty".to_string(),
            ));
        }
        let sanitized_endpoint = sanitize_metadata_endpoint(endpoint)?;
        if contains_secret_marker(&sanitized_endpoint) {
            return Err(MetadataError::InvalidRequest(
                "metadata endpoint must not contain secret-looking material".to_string(),
            ));
        }

        Ok(MetadataServiceCheck {
            endpoint: sanitized_endpoint,
            auth_mode: self.auth_mode,
            network_check: "skipped".to_string(),
            production_ready: false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataServiceCheck {
    pub endpoint: String,
    pub auth_mode: MetadataAuthMode,
    pub network_check: String,
    pub production_ready: bool,
}

pub trait MetadataStore {
    fn upsert_device(&mut self, request: UpsertDeviceRequest) -> MetadataResult<DeviceRecord>;
    fn upsert_project(&mut self, request: UpsertProjectRequest) -> MetadataResult<ProjectRecord>;
    fn publish_snapshot(
        &mut self,
        request: PublishSnapshotRequest,
    ) -> MetadataResult<PublishedSnapshotRecord>;
    fn snapshot(
        &self,
        account_id: &str,
        project_id: &str,
        snapshot_id: &str,
    ) -> MetadataResult<Option<PublishedSnapshotRecord>>;
    fn cursor(
        &self,
        account_id: &str,
        device_id: &str,
        project_id: &str,
    ) -> MetadataResult<Option<DeviceProjectCursorRecord>>;
    fn compare_and_set_cursor(
        &mut self,
        request: UpdateCursorRequest,
    ) -> MetadataResult<DeviceProjectCursorRecord>;
}

#[derive(Debug, Default)]
pub struct InMemoryMetadataStore {
    accounts: BTreeMap<String, AccountRecord>,
    devices: BTreeMap<(String, String), DeviceRecord>,
    projects: BTreeMap<(String, String), ProjectRecord>,
    snapshots: BTreeMap<(String, String, String), PublishedSnapshotRecord>,
    cursors: BTreeMap<(String, String, String), DeviceProjectCursorRecord>,
}

impl MetadataStore for InMemoryMetadataStore {
    fn upsert_device(&mut self, request: UpsertDeviceRequest) -> MetadataResult<DeviceRecord> {
        ensure_no_secret_material(&request)?;
        let account = self
            .accounts
            .entry(request.account_id.clone())
            .or_insert_with(|| AccountRecord {
                account_id: request.account_id.clone(),
                display_name: "mock-dev account".to_string(),
                created_at: request.last_seen_at.clone(),
                updated_at: request.last_seen_at.clone(),
            });
        account.updated_at = request.last_seen_at.clone();

        let record = DeviceRecord {
            account_id: request.account_id,
            device_id: request.device_id,
            display_name: request.display_name,
            trust_state: "mock-dev-trusted".to_string(),
            last_seen_at: request.last_seen_at.clone(),
            updated_at: request.last_seen_at,
        };
        self.devices.insert(
            (record.account_id.clone(), record.device_id.clone()),
            record.clone(),
        );
        Ok(record)
    }

    fn upsert_project(&mut self, request: UpsertProjectRequest) -> MetadataResult<ProjectRecord> {
        ensure_no_secret_material(&request)?;
        if !self.accounts.contains_key(&request.account_id) {
            return Err(MetadataError::InvalidRequest(
                "account must be registered before project".to_string(),
            ));
        }
        let record = ProjectRecord {
            account_id: request.account_id,
            project_id: request.project_id,
            display_name: request.display_name,
            root_hint: request.root_hint,
            project_kind: request.project_kind,
            updated_at: request.updated_at,
        };
        self.projects.insert(
            (record.account_id.clone(), record.project_id.clone()),
            record.clone(),
        );
        Ok(record)
    }

    fn publish_snapshot(
        &mut self,
        request: PublishSnapshotRequest,
    ) -> MetadataResult<PublishedSnapshotRecord> {
        ensure_no_secret_material(&request)?;
        ensure_in_memory_snapshot_dependencies(self, &request)?;
        let record = PublishedSnapshotRecord {
            account_id: request.account_id,
            project_id: request.project_id,
            snapshot_id: request.snapshot_id,
            parent_snapshot_id: request.parent_snapshot_id,
            manifest_object_key: request.manifest_object_key,
            manifest_hash: request.manifest_hash,
            manifest_entry_count: request.manifest_entry_count,
            total_size_bytes: request.total_size_bytes,
            published_by_device_id: request.published_by_device_id,
            published_at: request.published_at,
        };
        self.snapshots.insert(
            (
                record.account_id.clone(),
                record.project_id.clone(),
                record.snapshot_id.clone(),
            ),
            record.clone(),
        );
        Ok(record)
    }

    fn snapshot(
        &self,
        account_id: &str,
        project_id: &str,
        snapshot_id: &str,
    ) -> MetadataResult<Option<PublishedSnapshotRecord>> {
        Ok(self
            .snapshots
            .get(&(
                account_id.to_string(),
                project_id.to_string(),
                snapshot_id.to_string(),
            ))
            .cloned())
    }

    fn cursor(
        &self,
        account_id: &str,
        device_id: &str,
        project_id: &str,
    ) -> MetadataResult<Option<DeviceProjectCursorRecord>> {
        Ok(self
            .cursors
            .get(&(
                account_id.to_string(),
                device_id.to_string(),
                project_id.to_string(),
            ))
            .cloned())
    }

    fn compare_and_set_cursor(
        &mut self,
        request: UpdateCursorRequest,
    ) -> MetadataResult<DeviceProjectCursorRecord> {
        ensure_no_secret_material(&request)?;
        ensure_in_memory_cursor_dependencies(self, &request)?;
        let key = (
            request.account_id.clone(),
            request.device_id.clone(),
            request.project_id.clone(),
        );
        let current = self
            .cursors
            .get(&key)
            .and_then(|record| record.cursor_value.clone());
        if current != request.expected_cursor {
            return Err(MetadataError::CursorPreconditionFailed {
                current_cursor: current,
            });
        }

        let record = DeviceProjectCursorRecord {
            account_id: request.account_id,
            device_id: request.device_id,
            project_id: request.project_id,
            cursor_value: request.next_cursor,
            updated_at: request.updated_at,
        };
        self.cursors.insert(key, record.clone());
        Ok(record)
    }
}

#[derive(Debug)]
pub struct SqliteMetadataStore {
    conn: Connection,
}

impl SqliteMetadataStore {
    pub fn open_in_memory() -> MetadataResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub fn open_file(path: impl AsRef<std::path::Path>) -> MetadataResult<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    fn from_connection(conn: Connection) -> MetadataResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.apply_migrations()?;
        Ok(store)
    }

    pub fn apply_migrations(&self) -> MetadataResult<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS metadata_accounts (
                account_id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS metadata_devices (
                account_id TEXT NOT NULL,
                device_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                trust_state TEXT NOT NULL,
                last_seen_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (account_id, device_id),
                FOREIGN KEY (account_id) REFERENCES metadata_accounts(account_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS metadata_projects (
                account_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                root_hint TEXT NOT NULL,
                project_kind TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (account_id, project_id),
                FOREIGN KEY (account_id) REFERENCES metadata_accounts(account_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS metadata_snapshots (
                account_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                snapshot_id TEXT NOT NULL,
                parent_snapshot_id TEXT,
                manifest_object_key TEXT NOT NULL,
                manifest_hash TEXT NOT NULL,
                manifest_entry_count INTEGER NOT NULL,
                total_size_bytes INTEGER NOT NULL,
                published_by_device_id TEXT NOT NULL,
                published_at TEXT NOT NULL,
                PRIMARY KEY (account_id, project_id, snapshot_id),
                FOREIGN KEY (account_id, project_id) REFERENCES metadata_projects(account_id, project_id) ON DELETE CASCADE,
                FOREIGN KEY (account_id, published_by_device_id) REFERENCES metadata_devices(account_id, device_id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS metadata_device_project_cursors (
                account_id TEXT NOT NULL,
                device_id TEXT NOT NULL,
                project_id TEXT NOT NULL,
                cursor_value TEXT,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (account_id, device_id, project_id),
                FOREIGN KEY (account_id, device_id) REFERENCES metadata_devices(account_id, device_id) ON DELETE CASCADE,
                FOREIGN KEY (account_id, project_id) REFERENCES metadata_projects(account_id, project_id) ON DELETE CASCADE
            );
            "#,
        )?;
        Ok(())
    }
}

impl MetadataStore for SqliteMetadataStore {
    fn upsert_device(&mut self, request: UpsertDeviceRequest) -> MetadataResult<DeviceRecord> {
        ensure_no_secret_material(&request)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO metadata_accounts (account_id, display_name, created_at, updated_at)
            VALUES (?1, 'mock-dev account', ?2, ?2)
            ON CONFLICT(account_id) DO UPDATE SET updated_at = excluded.updated_at
            "#,
            params![request.account_id, request.last_seen_at],
        )?;
        tx.execute(
            r#"
            INSERT INTO metadata_devices (
                account_id,
                device_id,
                display_name,
                trust_state,
                last_seen_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, 'mock-dev-trusted', ?4, ?4)
            ON CONFLICT(account_id, device_id) DO UPDATE SET
                display_name = excluded.display_name,
                trust_state = excluded.trust_state,
                last_seen_at = excluded.last_seen_at,
                updated_at = excluded.updated_at
            "#,
            params![
                request.account_id,
                request.device_id,
                request.display_name,
                request.last_seen_at
            ],
        )?;
        tx.commit()?;
        self.device(&request.account_id, &request.device_id)?
            .ok_or_else(|| MetadataError::NotFound {
                entity: "device",
                id: request.device_id,
            })
    }

    fn upsert_project(&mut self, request: UpsertProjectRequest) -> MetadataResult<ProjectRecord> {
        ensure_no_secret_material(&request)?;
        self.ensure_account_exists(&request.account_id)?;
        self.conn.execute(
            r#"
            INSERT INTO metadata_projects (
                account_id,
                project_id,
                display_name,
                root_hint,
                project_kind,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(account_id, project_id) DO UPDATE SET
                display_name = excluded.display_name,
                root_hint = excluded.root_hint,
                project_kind = excluded.project_kind,
                updated_at = excluded.updated_at
            "#,
            params![
                request.account_id,
                request.project_id,
                request.display_name,
                request.root_hint,
                request.project_kind,
                request.updated_at
            ],
        )?;
        self.project(&request.account_id, &request.project_id)?
            .ok_or_else(|| MetadataError::NotFound {
                entity: "project",
                id: request.project_id,
            })
    }

    fn publish_snapshot(
        &mut self,
        request: PublishSnapshotRequest,
    ) -> MetadataResult<PublishedSnapshotRecord> {
        ensure_no_secret_material(&request)?;
        self.ensure_project_exists(&request.account_id, &request.project_id)?;
        self.ensure_device_exists(&request.account_id, &request.published_by_device_id)?;
        self.conn.execute(
            r#"
            INSERT INTO metadata_snapshots (
                account_id,
                project_id,
                snapshot_id,
                parent_snapshot_id,
                manifest_object_key,
                manifest_hash,
                manifest_entry_count,
                total_size_bytes,
                published_by_device_id,
                published_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(account_id, project_id, snapshot_id) DO UPDATE SET
                parent_snapshot_id = excluded.parent_snapshot_id,
                manifest_object_key = excluded.manifest_object_key,
                manifest_hash = excluded.manifest_hash,
                manifest_entry_count = excluded.manifest_entry_count,
                total_size_bytes = excluded.total_size_bytes,
                published_by_device_id = excluded.published_by_device_id,
                published_at = excluded.published_at
            "#,
            params![
                request.account_id,
                request.project_id,
                request.snapshot_id,
                request.parent_snapshot_id,
                request.manifest_object_key,
                request.manifest_hash,
                request.manifest_entry_count,
                request.total_size_bytes,
                request.published_by_device_id,
                request.published_at
            ],
        )?;
        self.snapshot(
            &request.account_id,
            &request.project_id,
            &request.snapshot_id,
        )?
        .ok_or_else(|| MetadataError::NotFound {
            entity: "snapshot",
            id: request.snapshot_id,
        })
    }

    fn snapshot(
        &self,
        account_id: &str,
        project_id: &str,
        snapshot_id: &str,
    ) -> MetadataResult<Option<PublishedSnapshotRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    account_id,
                    project_id,
                    snapshot_id,
                    parent_snapshot_id,
                    manifest_object_key,
                    manifest_hash,
                    manifest_entry_count,
                    total_size_bytes,
                    published_by_device_id,
                    published_at
                FROM metadata_snapshots
                WHERE account_id = ?1 AND project_id = ?2 AND snapshot_id = ?3
                "#,
                params![account_id, project_id, snapshot_id],
                snapshot_from_row,
            )
            .optional()
            .map_err(MetadataError::from)
    }

    fn cursor(
        &self,
        account_id: &str,
        device_id: &str,
        project_id: &str,
    ) -> MetadataResult<Option<DeviceProjectCursorRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT account_id, device_id, project_id, cursor_value, updated_at
                FROM metadata_device_project_cursors
                WHERE account_id = ?1 AND device_id = ?2 AND project_id = ?3
                "#,
                params![account_id, device_id, project_id],
                cursor_from_row,
            )
            .optional()
            .map_err(MetadataError::from)
    }

    fn compare_and_set_cursor(
        &mut self,
        request: UpdateCursorRequest,
    ) -> MetadataResult<DeviceProjectCursorRecord> {
        ensure_no_secret_material(&request)?;
        self.ensure_device_exists(&request.account_id, &request.device_id)?;
        self.ensure_project_exists(&request.account_id, &request.project_id)?;
        let tx = self.conn.transaction()?;
        let current = tx
            .query_row(
                r#"
                SELECT cursor_value
                FROM metadata_device_project_cursors
                WHERE account_id = ?1 AND device_id = ?2 AND project_id = ?3
                "#,
                params![request.account_id, request.device_id, request.project_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();

        if current != request.expected_cursor {
            return Err(MetadataError::CursorPreconditionFailed {
                current_cursor: current,
            });
        }

        tx.execute(
            r#"
            INSERT INTO metadata_device_project_cursors (
                account_id,
                device_id,
                project_id,
                cursor_value,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(account_id, device_id, project_id) DO UPDATE SET
                cursor_value = excluded.cursor_value,
                updated_at = excluded.updated_at
            "#,
            params![
                request.account_id,
                request.device_id,
                request.project_id,
                request.next_cursor,
                request.updated_at
            ],
        )?;
        tx.commit()?;
        self.cursor(&request.account_id, &request.device_id, &request.project_id)?
            .ok_or_else(|| MetadataError::NotFound {
                entity: "cursor",
                id: format!(
                    "{}/{}/{}",
                    request.account_id, request.device_id, request.project_id
                ),
            })
    }
}

impl SqliteMetadataStore {
    fn device(&self, account_id: &str, device_id: &str) -> MetadataResult<Option<DeviceRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT account_id, device_id, display_name, trust_state, last_seen_at, updated_at
                FROM metadata_devices
                WHERE account_id = ?1 AND device_id = ?2
                "#,
                params![account_id, device_id],
                |row| {
                    Ok(DeviceRecord {
                        account_id: row.get(0)?,
                        device_id: row.get(1)?,
                        display_name: row.get(2)?,
                        trust_state: row.get(3)?,
                        last_seen_at: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(MetadataError::from)
    }

    fn project(&self, account_id: &str, project_id: &str) -> MetadataResult<Option<ProjectRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT account_id, project_id, display_name, root_hint, project_kind, updated_at
                FROM metadata_projects
                WHERE account_id = ?1 AND project_id = ?2
                "#,
                params![account_id, project_id],
                |row| {
                    Ok(ProjectRecord {
                        account_id: row.get(0)?,
                        project_id: row.get(1)?,
                        display_name: row.get(2)?,
                        root_hint: row.get(3)?,
                        project_kind: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(MetadataError::from)
    }

    fn ensure_account_exists(&self, account_id: &str) -> MetadataResult<()> {
        let exists = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM metadata_accounts WHERE account_id = ?1)",
            params![account_id],
            |row| row.get::<_, bool>(0),
        )?;
        if exists {
            Ok(())
        } else {
            Err(MetadataError::InvalidRequest(
                "account must be registered before project".to_string(),
            ))
        }
    }

    fn ensure_device_exists(&self, account_id: &str, device_id: &str) -> MetadataResult<()> {
        if self.device(account_id, device_id)?.is_some() {
            Ok(())
        } else {
            Err(MetadataError::InvalidRequest(
                "device must be registered before this metadata write".to_string(),
            ))
        }
    }

    fn ensure_project_exists(&self, account_id: &str, project_id: &str) -> MetadataResult<()> {
        if self.project(account_id, project_id)?.is_some() {
            Ok(())
        } else {
            Err(MetadataError::InvalidRequest(
                "project must be registered before this metadata write".to_string(),
            ))
        }
    }
}

pub type SharedMetadataStore<S> = Arc<Mutex<S>>;

pub fn app<S>(store: S) -> Router
where
    S: MetadataStore + Send + 'static,
{
    Router::new()
        .route("/health", get(health))
        .route("/v1/devices", put(upsert_device::<S>))
        .route("/v1/projects", put(upsert_project::<S>))
        .route(
            "/v1/projects/:project_id/snapshots",
            put(publish_snapshot::<S>),
        )
        .route(
            "/v1/projects/:project_id/snapshots/:snapshot_id",
            get(get_snapshot::<S>),
        )
        .route(
            "/v1/cursors/:project_id/:device_id",
            get(get_cursor::<S>).put(update_cursor::<S>),
        )
        .with_state(Arc::new(Mutex::new(store)))
}

pub async fn serve_sqlite(path: &str, addr: SocketAddr) -> MetadataResult<()> {
    let store = SqliteMetadataStore::open_file(path)?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| MetadataError::InvalidRequest(error.to_string()))?;
    axum::serve(listener, app(store))
        .await
        .map_err(|error| MetadataError::InvalidRequest(error.to_string()))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        service: "devbox-metadata".to_string(),
        storage: "sqlite-dev".to_string(),
    })
}

async fn upsert_device<S>(
    State(store): State<SharedMetadataStore<S>>,
    headers: HeaderMap,
    Json(request): Json<UpsertDeviceRequest>,
) -> MetadataResult<Json<DeviceRecord>>
where
    S: MetadataStore,
{
    authorize(&headers, &request.account_id, &request.device_id)?;
    let mut store = store.lock().map_err(|_| MetadataError::PoisonedStore)?;
    Ok(Json(store.upsert_device(request)?))
}

async fn upsert_project<S>(
    State(store): State<SharedMetadataStore<S>>,
    headers: HeaderMap,
    Json(request): Json<UpsertProjectRequest>,
) -> MetadataResult<Json<ProjectRecord>>
where
    S: MetadataStore,
{
    let identity = MockDevIdentity::from_headers(&headers)?;
    authorize_identity(&identity, &request.account_id, &identity.device_id)?;
    let mut store = store.lock().map_err(|_| MetadataError::PoisonedStore)?;
    Ok(Json(store.upsert_project(request)?))
}

async fn publish_snapshot<S>(
    State(store): State<SharedMetadataStore<S>>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<PublishSnapshotRequest>,
) -> MetadataResult<Json<PublishedSnapshotRecord>>
where
    S: MetadataStore,
{
    if request.project_id != project_id {
        return Err(MetadataError::InvalidRequest(
            "snapshot path and body project must match".to_string(),
        ));
    }
    authorize(
        &headers,
        &request.account_id,
        &request.published_by_device_id,
    )?;
    let mut store = store.lock().map_err(|_| MetadataError::PoisonedStore)?;
    Ok(Json(store.publish_snapshot(request)?))
}

async fn get_snapshot<S>(
    State(store): State<SharedMetadataStore<S>>,
    headers: HeaderMap,
    Path((project_id, snapshot_id)): Path<(String, String)>,
) -> MetadataResult<Json<PublishedSnapshotRecord>>
where
    S: MetadataStore,
{
    let identity = MockDevIdentity::from_headers(&headers)?;
    let store = store.lock().map_err(|_| MetadataError::PoisonedStore)?;
    let record = store
        .snapshot(&identity.account_id, &project_id, &snapshot_id)?
        .ok_or_else(|| MetadataError::NotFound {
            entity: "snapshot",
            id: snapshot_id,
        })?;
    Ok(Json(record))
}

async fn get_cursor<S>(
    State(store): State<SharedMetadataStore<S>>,
    headers: HeaderMap,
    Path((project_id, device_id)): Path<(String, String)>,
) -> MetadataResult<Json<DeviceProjectCursorRecord>>
where
    S: MetadataStore,
{
    let identity = MockDevIdentity::from_headers(&headers)?;
    authorize_identity(&identity, &identity.account_id, &device_id)?;
    let store = store.lock().map_err(|_| MetadataError::PoisonedStore)?;
    let record = store.cursor(&identity.account_id, &device_id, &project_id)?;
    Ok(Json(record.unwrap_or(DeviceProjectCursorRecord {
        account_id: identity.account_id,
        device_id,
        project_id,
        cursor_value: None,
        updated_at: "-".to_string(),
    })))
}

async fn update_cursor<S>(
    State(store): State<SharedMetadataStore<S>>,
    headers: HeaderMap,
    Path((project_id, device_id)): Path<(String, String)>,
    Json(request): Json<UpdateCursorRequest>,
) -> MetadataResult<Json<DeviceProjectCursorRecord>>
where
    S: MetadataStore,
{
    authorize(&headers, &request.account_id, &request.device_id)?;
    if request.project_id != project_id || request.device_id != device_id {
        return Err(MetadataError::InvalidRequest(
            "cursor path and body identity must match".to_string(),
        ));
    }
    let mut store = store.lock().map_err(|_| MetadataError::PoisonedStore)?;
    Ok(Json(store.compare_and_set_cursor(request)?))
}

fn authorize(headers: &HeaderMap, account_id: &str, device_id: &str) -> MetadataResult<()> {
    let identity = MockDevIdentity::from_headers(headers)?;
    authorize_identity(&identity, account_id, device_id)
}

fn authorize_identity(
    identity: &MockDevIdentity,
    account_id: &str,
    device_id: &str,
) -> MetadataResult<()> {
    if identity.account_id != account_id || identity.device_id != device_id {
        return Err(MetadataError::IdentityMismatch);
    }
    Ok(())
}

fn required_header(headers: &HeaderMap, name: &'static str) -> MetadataResult<String> {
    let value = headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string())
        .ok_or(MetadataError::MissingMockDevIdentity)?;
    if contains_secret_marker(&value) {
        return Err(MetadataError::InvalidRequest(
            "mock-dev identity headers must not contain secret-looking material".to_string(),
        ));
    }
    Ok(value)
}

fn ensure_no_secret_material<T: Serialize>(value: &T) -> MetadataResult<()> {
    let encoded = serde_json::to_string(value)
        .map_err(|error| MetadataError::InvalidRequest(error.to_string()))?;
    if contains_secret_marker(&encoded) {
        return Err(MetadataError::InvalidRequest(
            "metadata requests must not contain raw keys, tokens, credentials, or secret material"
                .to_string(),
        ));
    }
    Ok(())
}

fn ensure_in_memory_snapshot_dependencies(
    store: &InMemoryMetadataStore,
    request: &PublishSnapshotRequest,
) -> MetadataResult<()> {
    if !store
        .projects
        .contains_key(&(request.account_id.clone(), request.project_id.clone()))
    {
        return Err(MetadataError::InvalidRequest(
            "project must be registered before this metadata write".to_string(),
        ));
    }
    if !store.devices.contains_key(&(
        request.account_id.clone(),
        request.published_by_device_id.clone(),
    )) {
        return Err(MetadataError::InvalidRequest(
            "device must be registered before this metadata write".to_string(),
        ));
    }
    Ok(())
}

fn ensure_in_memory_cursor_dependencies(
    store: &InMemoryMetadataStore,
    request: &UpdateCursorRequest,
) -> MetadataResult<()> {
    if !store
        .devices
        .contains_key(&(request.account_id.clone(), request.device_id.clone()))
    {
        return Err(MetadataError::InvalidRequest(
            "device must be registered before this metadata write".to_string(),
        ));
    }
    if !store
        .projects
        .contains_key(&(request.account_id.clone(), request.project_id.clone()))
    {
        return Err(MetadataError::InvalidRequest(
            "project must be registered before this metadata write".to_string(),
        ));
    }
    Ok(())
}

fn sanitize_metadata_endpoint(endpoint: &str) -> MetadataResult<String> {
    let url = Url::parse(endpoint).map_err(|_| {
        MetadataError::InvalidRequest(
            "metadata endpoint must be an absolute HTTP or HTTPS URL".to_string(),
        )
    })?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(MetadataError::InvalidRequest(
                "metadata endpoint must use http or https".to_string(),
            ));
        }
    }
    if url.host_str().is_none() {
        return Err(MetadataError::InvalidRequest(
            "metadata endpoint must include a host".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(MetadataError::InvalidRequest(
            "metadata endpoint must not include userinfo".to_string(),
        ));
    }
    if url.query().is_some() {
        return Err(MetadataError::InvalidRequest(
            "metadata endpoint must not include a query string".to_string(),
        ));
    }
    if url.fragment().is_some() {
        return Err(MetadataError::InvalidRequest(
            "metadata endpoint must not include a fragment".to_string(),
        ));
    }

    let sanitized = url.to_string();
    if contains_secret_marker(&sanitized) {
        return Err(MetadataError::InvalidRequest(
            "metadata endpoint must not contain secret-looking material".to_string(),
        ));
    }
    Ok(sanitized)
}

fn contains_secret_marker(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    SECRET_MARKERS.iter().any(|marker| lowered.contains(marker))
}

fn is_sqlite_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(sqlite_error, _)
            if sqlite_error.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn snapshot_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PublishedSnapshotRecord> {
    Ok(PublishedSnapshotRecord {
        account_id: row.get(0)?,
        project_id: row.get(1)?,
        snapshot_id: row.get(2)?,
        parent_snapshot_id: row.get(3)?,
        manifest_object_key: row.get(4)?,
        manifest_hash: row.get(5)?,
        manifest_entry_count: row.get(6)?,
        total_size_bytes: row.get(7)?,
        published_by_device_id: row.get(8)?,
        published_at: row.get(9)?,
    })
}

fn cursor_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceProjectCursorRecord> {
    Ok(DeviceProjectCursorRecord {
        account_id: row.get(0)?,
        device_id: row.get(1)?,
        project_id: row.get(2)?,
        cursor_value: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower::ServiceExt;

    const ACCOUNT: &str = "account-alpha";
    const DEVICE: &str = "device-laptop";
    const PROJECT: &str = "project-devbox";

    #[test]
    fn in_memory_cursor_compare_and_set_requires_expected_cursor() {
        let mut store = seeded_store();

        let created = store
            .compare_and_set_cursor(UpdateCursorRequest {
                account_id: ACCOUNT.to_string(),
                device_id: DEVICE.to_string(),
                project_id: PROJECT.to_string(),
                expected_cursor: None,
                next_cursor: Some("snapshot-a".to_string()),
                updated_at: "2026-06-18T10:04:00Z".to_string(),
            })
            .expect("initial cursor set succeeds");
        assert_eq!(created.cursor_value.as_deref(), Some("snapshot-a"));

        let conflict = store
            .compare_and_set_cursor(UpdateCursorRequest {
                account_id: ACCOUNT.to_string(),
                device_id: DEVICE.to_string(),
                project_id: PROJECT.to_string(),
                expected_cursor: None,
                next_cursor: Some("snapshot-b".to_string()),
                updated_at: "2026-06-18T10:05:00Z".to_string(),
            })
            .expect_err("stale cursor precondition fails");
        assert!(matches!(
            conflict,
            MetadataError::CursorPreconditionFailed {
                current_cursor: Some(_)
            }
        ));

        let advanced = store
            .compare_and_set_cursor(UpdateCursorRequest {
                account_id: ACCOUNT.to_string(),
                device_id: DEVICE.to_string(),
                project_id: PROJECT.to_string(),
                expected_cursor: Some("snapshot-a".to_string()),
                next_cursor: Some("snapshot-b".to_string()),
                updated_at: "2026-06-18T10:06:00Z".to_string(),
            })
            .expect("matching expected cursor advances");
        assert_eq!(advanced.cursor_value.as_deref(), Some("snapshot-b"));
    }

    #[test]
    fn sqlite_store_round_trips_devices_projects_snapshots_and_cursors() {
        let mut store = SqliteMetadataStore::open_in_memory().expect("sqlite store opens");
        seed_store(&mut store);
        let snapshot = publish_request();
        let persisted = store
            .publish_snapshot(snapshot.clone())
            .expect("snapshot metadata persists");
        assert_eq!(persisted, PublishedSnapshotRecord::from(snapshot));

        let fetched = store
            .snapshot(ACCOUNT, PROJECT, "snapshot-a")
            .expect("snapshot lookup works")
            .expect("snapshot exists");
        assert_eq!(fetched.manifest_object_key, "manifests/snapshot-a.json.enc");

        let cursor = store
            .compare_and_set_cursor(UpdateCursorRequest {
                account_id: ACCOUNT.to_string(),
                device_id: DEVICE.to_string(),
                project_id: PROJECT.to_string(),
                expected_cursor: None,
                next_cursor: Some("snapshot-a".to_string()),
                updated_at: "2026-06-18T10:04:00Z".to_string(),
            })
            .expect("cursor persists");
        assert_eq!(cursor.cursor_value.as_deref(), Some("snapshot-a"));
    }

    #[test]
    fn sqlite_snapshot_identity_is_project_scoped() {
        let mut store = SqliteMetadataStore::open_in_memory().expect("sqlite store opens");
        seed_store(&mut store);
        store
            .upsert_project(UpsertProjectRequest {
                account_id: ACCOUNT.to_string(),
                project_id: "project-other".to_string(),
                display_name: "other".to_string(),
                root_hint: "~/Code/other".to_string(),
                project_kind: "rust".to_string(),
                updated_at: "2026-06-18T10:02:00Z".to_string(),
            })
            .expect("second project upserts");

        store
            .publish_snapshot(publish_request())
            .expect("first project snapshot persists");
        store
            .publish_snapshot(PublishSnapshotRequest {
                project_id: "project-other".to_string(),
                manifest_object_key: "manifests/other/snapshot-a.json.enc".to_string(),
                ..publish_request()
            })
            .expect("second project can reuse snapshot id");

        let first = store
            .snapshot(ACCOUNT, PROJECT, "snapshot-a")
            .expect("first lookup works")
            .expect("first snapshot exists");
        let second = store
            .snapshot(ACCOUNT, "project-other", "snapshot-a")
            .expect("second lookup works")
            .expect("second snapshot exists");

        assert_eq!(first.project_id, PROJECT);
        assert_eq!(first.manifest_object_key, "manifests/snapshot-a.json.enc");
        assert_eq!(second.project_id, "project-other");
        assert_eq!(
            second.manifest_object_key,
            "manifests/other/snapshot-a.json.enc"
        );
    }

    #[test]
    fn secret_marker_requests_are_rejected_and_redacted_in_debug() {
        let mut store = seeded_store();
        let error = store
            .publish_snapshot(PublishSnapshotRequest {
                manifest_object_key: "manifests/raw-sync-key.json".to_string(),
                ..publish_request()
            })
            .expect_err("secret-looking request is rejected");
        assert_eq!(
            error.to_string(),
            "metadata requests must not contain raw keys, tokens, credentials, or secret material"
        );

        let identity = MockDevIdentity {
            account_id: ACCOUNT.to_string(),
            device_id: DEVICE.to_string(),
        };
        assert!(!format!("{identity:?}").to_ascii_lowercase().contains("key"));
    }

    #[test]
    fn endpoint_validation_rejects_unsafe_urls_and_returns_sanitized_output() {
        let safe = MetadataServiceConfig {
            endpoint: "https://metadata.example:8443/devbox".to_string(),
            auth_mode: MetadataAuthMode::MockDevHeaders,
        }
        .validate()
        .expect("safe endpoint validates");
        assert_eq!(safe.endpoint, "https://metadata.example:8443/devbox");

        for (endpoint, expected) in [
            ("", "metadata endpoint must not be empty"),
            (
                "metadata.example/devbox",
                "metadata endpoint must be an absolute HTTP or HTTPS URL",
            ),
            (
                "ftp://metadata.example",
                "metadata endpoint must use http or https",
            ),
            (
                "https://user:password@metadata.example",
                "metadata endpoint must not include userinfo",
            ),
            (
                "https://metadata.example/path?access=abc",
                "metadata endpoint must not include a query string",
            ),
            (
                "https://metadata.example/path#access",
                "metadata endpoint must not include a fragment",
            ),
            (
                "https://metadata.example/sync-key",
                "metadata endpoint must not contain secret-looking material",
            ),
        ] {
            let error = MetadataServiceConfig {
                endpoint: endpoint.to_string(),
                auth_mode: MetadataAuthMode::MockDevHeaders,
            }
            .validate()
            .expect_err("unsafe endpoint is rejected");
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn in_memory_store_rejects_missing_parent_metadata() {
        let mut store = InMemoryMetadataStore::default();
        let project = store
            .upsert_project(project_request())
            .expect_err("project requires account");
        assert_eq!(
            project.to_string(),
            "account must be registered before project"
        );

        store
            .upsert_device(device_request())
            .expect("device upserts");
        let snapshot = store
            .publish_snapshot(publish_request())
            .expect_err("snapshot requires project");
        assert_eq!(
            snapshot.to_string(),
            "project must be registered before this metadata write"
        );

        let cursor = store
            .compare_and_set_cursor(UpdateCursorRequest {
                account_id: ACCOUNT.to_string(),
                device_id: DEVICE.to_string(),
                project_id: PROJECT.to_string(),
                expected_cursor: None,
                next_cursor: Some("snapshot-a".to_string()),
                updated_at: "2026-06-18T10:04:00Z".to_string(),
            })
            .expect_err("cursor requires project");
        assert_eq!(
            cursor.to_string(),
            "project must be registered before this metadata write"
        );
    }

    #[tokio::test]
    async fn handlers_require_mock_dev_identity_and_return_cursor_conflicts() {
        let app = app(seeded_store());
        let missing_auth = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/cursors/project-devbox/device-laptop",
                &UpdateCursorRequest {
                    account_id: ACCOUNT.to_string(),
                    device_id: DEVICE.to_string(),
                    project_id: PROJECT.to_string(),
                    expected_cursor: None,
                    next_cursor: Some("snapshot-a".to_string()),
                    updated_at: "2026-06-18T10:04:00Z".to_string(),
                },
                false,
            ))
            .await
            .expect("response returns");
        assert_eq!(missing_auth.status(), StatusCode::UNAUTHORIZED);

        let ok = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/cursors/project-devbox/device-laptop",
                &UpdateCursorRequest {
                    account_id: ACCOUNT.to_string(),
                    device_id: DEVICE.to_string(),
                    project_id: PROJECT.to_string(),
                    expected_cursor: None,
                    next_cursor: Some("snapshot-a".to_string()),
                    updated_at: "2026-06-18T10:04:00Z".to_string(),
                },
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(ok.status(), StatusCode::OK);

        let conflict = app
            .oneshot(json_request(
                Method::PUT,
                "/v1/cursors/project-devbox/device-laptop",
                &UpdateCursorRequest {
                    account_id: ACCOUNT.to_string(),
                    device_id: DEVICE.to_string(),
                    project_id: PROJECT.to_string(),
                    expected_cursor: None,
                    next_cursor: Some("snapshot-b".to_string()),
                    updated_at: "2026-06-18T10:05:00Z".to_string(),
                },
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn handlers_cover_health_registration_publish_and_fetch() {
        let app = app(InMemoryMetadataStore::default());

        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("response returns");
        assert_eq!(health.status(), StatusCode::OK);

        let device = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/devices",
                &device_request(),
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(device.status(), StatusCode::OK);

        let project = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/projects",
                &project_request(),
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(project.status(), StatusCode::OK);

        let publish = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/projects/project-devbox/snapshots",
                &publish_request(),
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(publish.status(), StatusCode::OK);

        let fetched = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/projects/project-devbox/snapshots/snapshot-a")
                    .header(MOCK_ACCOUNT_HEADER, ACCOUNT)
                    .header(MOCK_DEVICE_HEADER, DEVICE)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("response returns");
        assert_eq!(fetched.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn sqlite_handlers_cover_registration_publish_fetch_and_cursor_flow() {
        let app = app(SqliteMetadataStore::open_in_memory().expect("sqlite store opens"));

        let device = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/devices",
                &device_request(),
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(device.status(), StatusCode::OK);

        let project = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/projects",
                &project_request(),
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(project.status(), StatusCode::OK);

        let publish = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/projects/project-devbox/snapshots",
                &publish_request(),
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(publish.status(), StatusCode::OK);

        let fetched = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/projects/project-devbox/snapshots/snapshot-a")
                    .header(MOCK_ACCOUNT_HEADER, ACCOUNT)
                    .header(MOCK_DEVICE_HEADER, DEVICE)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("response returns");
        assert_eq!(fetched.status(), StatusCode::OK);

        let cursor = app
            .oneshot(json_request(
                Method::PUT,
                "/v1/cursors/project-devbox/device-laptop",
                &UpdateCursorRequest {
                    account_id: ACCOUNT.to_string(),
                    device_id: DEVICE.to_string(),
                    project_id: PROJECT.to_string(),
                    expected_cursor: None,
                    next_cursor: Some("snapshot-a".to_string()),
                    updated_at: "2026-06-18T10:04:00Z".to_string(),
                },
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(cursor.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn sqlite_handlers_return_sanitized_4xx_for_out_of_order_calls() {
        let app = app(SqliteMetadataStore::open_in_memory().expect("sqlite store opens"));

        let project = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/projects",
                &project_request(),
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(project.status(), StatusCode::BAD_REQUEST);
        let body = response_text(project).await;
        assert!(body.contains("account must be registered before project"));
        assert!(!body.contains("FOREIGN KEY"));
        assert!(!body.contains("constraint failed"));

        let snapshot = app
            .clone()
            .oneshot(json_request(
                Method::PUT,
                "/v1/projects/project-devbox/snapshots",
                &publish_request(),
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(snapshot.status(), StatusCode::BAD_REQUEST);
        let body = response_text(snapshot).await;
        assert!(body.contains("project must be registered before this metadata write"));
        assert!(!body.contains("FOREIGN KEY"));
        assert!(!body.contains("constraint failed"));

        let cursor = app
            .oneshot(json_request(
                Method::PUT,
                "/v1/cursors/project-devbox/device-laptop",
                &UpdateCursorRequest {
                    account_id: ACCOUNT.to_string(),
                    device_id: DEVICE.to_string(),
                    project_id: PROJECT.to_string(),
                    expected_cursor: None,
                    next_cursor: Some("snapshot-a".to_string()),
                    updated_at: "2026-06-18T10:04:00Z".to_string(),
                },
                true,
            ))
            .await
            .expect("response returns");
        assert_eq!(cursor.status(), StatusCode::BAD_REQUEST);
        let body = response_text(cursor).await;
        assert!(body.contains("device must be registered before this metadata write"));
        assert!(!body.contains("FOREIGN KEY"));
        assert!(!body.contains("constraint failed"));
    }

    #[tokio::test]
    async fn secret_like_mock_headers_are_not_reflected_in_error_bodies() {
        let app = app(seeded_store());
        let secret_like = "sync-key-should-not-echo";
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/v1/projects")
                    .header(MOCK_ACCOUNT_HEADER, secret_like)
                    .header(MOCK_DEVICE_HEADER, DEVICE)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&project_request()).expect("json encodes"),
                    ))
                    .expect("request builds"),
            )
            .await
            .expect("response returns");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_text(response).await;
        assert!(body.contains("mock-dev identity headers must not contain secret-looking material"));
        assert!(!body.contains(secret_like));

        let mismatch = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/v1/projects")
                    .header(MOCK_ACCOUNT_HEADER, "account-other")
                    .header(MOCK_DEVICE_HEADER, DEVICE)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_vec(&project_request()).expect("json encodes"),
                    ))
                    .expect("request builds"),
            )
            .await
            .expect("response returns");
        assert_eq!(mismatch.status(), StatusCode::UNAUTHORIZED);
        let body = response_text(mismatch).await;
        assert_eq!(body, "{\"error\":\"mock-dev identity mismatch\"}");
        assert!(!body.contains("account-other"));
    }

    #[test]
    fn metadata_service_check_is_local_and_non_production() {
        let check = MetadataServiceConfig {
            endpoint: "http://127.0.0.1:8787".to_string(),
            auth_mode: MetadataAuthMode::MockDevHeaders,
        }
        .validate()
        .expect("config validates");

        assert_eq!(check.network_check, "skipped");
        assert!(!check.production_ready);
    }

    fn seeded_store() -> InMemoryMetadataStore {
        let mut store = InMemoryMetadataStore::default();
        seed_store(&mut store);
        store
    }

    fn seed_store<S: MetadataStore>(store: &mut S) {
        store
            .upsert_device(device_request())
            .expect("device upserts");
        store
            .upsert_project(project_request())
            .expect("project upserts");
    }

    fn device_request() -> UpsertDeviceRequest {
        UpsertDeviceRequest {
            account_id: ACCOUNT.to_string(),
            device_id: DEVICE.to_string(),
            display_name: "Laptop".to_string(),
            last_seen_at: "2026-06-18T10:00:00Z".to_string(),
        }
    }

    fn project_request() -> UpsertProjectRequest {
        UpsertProjectRequest {
            account_id: ACCOUNT.to_string(),
            project_id: PROJECT.to_string(),
            display_name: "devbox".to_string(),
            root_hint: "~/Code/devbox".to_string(),
            project_kind: "rust".to_string(),
            updated_at: "2026-06-18T10:01:00Z".to_string(),
        }
    }

    fn publish_request() -> PublishSnapshotRequest {
        PublishSnapshotRequest {
            account_id: ACCOUNT.to_string(),
            project_id: PROJECT.to_string(),
            snapshot_id: "snapshot-a".to_string(),
            parent_snapshot_id: None,
            manifest_object_key: "manifests/snapshot-a.json.enc".to_string(),
            manifest_hash: "blake3:abc123".to_string(),
            manifest_entry_count: 7,
            total_size_bytes: 42,
            published_by_device_id: DEVICE.to_string(),
            published_at: "2026-06-18T10:03:00Z".to_string(),
        }
    }

    fn json_request<T: Serialize>(
        method: Method,
        uri: &str,
        body: &T,
        include_auth: bool,
    ) -> Request<Body> {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if include_auth {
            builder = builder
                .header(MOCK_ACCOUNT_HEADER, ACCOUNT)
                .header(MOCK_DEVICE_HEADER, DEVICE);
        }
        builder
            .body(Body::from(serde_json::to_vec(body).expect("json encodes")))
            .expect("request builds")
    }

    async fn response_text(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body reads");
        String::from_utf8(bytes.to_vec()).expect("response is utf8")
    }

    impl From<PublishSnapshotRequest> for PublishedSnapshotRecord {
        fn from(value: PublishSnapshotRequest) -> Self {
            Self {
                account_id: value.account_id,
                project_id: value.project_id,
                snapshot_id: value.snapshot_id,
                parent_snapshot_id: value.parent_snapshot_id,
                manifest_object_key: value.manifest_object_key,
                manifest_hash: value.manifest_hash,
                manifest_entry_count: value.manifest_entry_count,
                total_size_bytes: value.total_size_bytes,
                published_by_device_id: value.published_by_device_id,
                published_at: value.published_at,
            }
        }
    }
}
