//! Devbox hosted API boundary.
//!
//! This crate provides the PR6 local hosted API used by Loom's hosted remote.
//! Devbox owns sessions, devices, shared-folder membership, pack storage, and
//! cursor metadata. Loom still owns folder-state semantics inside the packs.

use devbox_platform::{AccountId, DeviceId, SharedFolderRole};
use devbox_sync::{
    ObjectKey, RemoteBlobProvider, S3CompatibleBlobProvider, S3CompatibleConfig,
    S3CredentialsSource, SyncError,
};
use loom_core::{CursorId, FolderRevisionId, ObjectId, SharedFolderId};
use postgres::{Client as PostgresClient, NoTls, Row};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiArea {
    Auth,
    Devices,
    SharedFolders,
    LoomRemote,
    ObjectAccess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRoute {
    pub area: ApiArea,
    pub path: &'static str,
}

impl ApiRoute {
    pub const fn new(area: ApiArea, path: &'static str) -> Self {
        Self { area, path }
    }
}

pub const API_ROUTES: &[ApiRoute] = &[
    ApiRoute::new(ApiArea::Auth, "/v1/auth"),
    ApiRoute::new(ApiArea::Devices, "/v1/devices"),
    ApiRoute::new(ApiArea::SharedFolders, "/v1/shared-folders"),
    ApiRoute::new(ApiArea::LoomRemote, "/v1/loom"),
    ApiRoute::new(ApiArea::ObjectAccess, "/v1/object-access"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevSessionResponse {
    pub account_id: String,
    pub session_id: String,
    pub session_token: String,
    pub device_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWorkOsSession {
    pub user_id: String,
    pub session_id: String,
    pub organization_id: Option<String>,
    pub device_id: String,
    pub device_display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedFolderResponse {
    pub id: SharedFolderId,
    pub account_id: AccountId,
    pub role: SharedFolderRole,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceResponse {
    pub id: DeviceId,
    pub account_id: AccountId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorUpdateRequest {
    pub shared_folder_id: SharedFolderId,
    pub cursor_id: CursorId,
    pub expected_revision_id: Option<FolderRevisionId>,
    pub next_revision_id: FolderRevisionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiBoundary {
    pub owns_accounts: bool,
    pub owns_devices: bool,
    pub owns_shared_folder_membership: bool,
    pub owns_folder_state_semantics: bool,
}

impl ApiBoundary {
    pub fn devbox_hosted_api() -> Self {
        Self {
            owns_accounts: true,
            owns_devices: true,
            owns_shared_folder_membership: true,
            owns_folder_state_semantics: false,
        }
    }
}

#[derive(Debug)]
pub enum ApiError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    BadRequest(String),
    Unauthorized,
    Forbidden(String),
    NotFound(String),
    Conflict {
        expected: Option<String>,
        actual: Option<String>,
    },
    RemoteStorage(String),
    Database(String),
    Json(String),
}

impl ApiError {
    pub fn status_code(&self) -> u16 {
        match self {
            Self::BadRequest(_) | Self::Json(_) => 400,
            Self::Unauthorized => 401,
            Self::Forbidden(_) => 403,
            Self::NotFound(_) => 404,
            Self::Conflict { .. } => 409,
            Self::RemoteStorage(_) => 502,
            Self::Io { .. } | Self::Database(_) => 500,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "could not access {}: {source}", path.display()),
            Self::BadRequest(message) => write!(f, "{message}"),
            Self::Unauthorized => f.write_str("missing or invalid Devbox session"),
            Self::Forbidden(message) => write!(f, "{message}"),
            Self::NotFound(message) => write!(f, "{message}"),
            Self::Conflict { expected, actual } => write!(
                f,
                "cursor compare-and-set refused: expected {}, found {}",
                expected.as_deref().unwrap_or("-"),
                actual.as_deref().unwrap_or("-")
            ),
            Self::RemoteStorage(message) => write!(f, "hosted storage error: {message}"),
            Self::Database(message) => write!(f, "metadata database error: {message}"),
            Self::Json(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::BadRequest(_)
            | Self::Unauthorized
            | Self::Forbidden(_)
            | Self::NotFound(_)
            | Self::Conflict { .. }
            | Self::RemoteStorage(_)
            | Self::Database(_)
            | Self::Json(_) => None,
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Clone)]
pub struct LocalDevboxApi {
    root: Arc<PathBuf>,
    metadata: Arc<dyn ApiMetadataStore>,
    pack_storage: Arc<dyn PackStorage>,
}

trait ApiMetadataStore: fmt::Debug + Send + Sync {
    fn label(&self) -> &'static str;
    fn upsert_session(&self, session: SessionRecord) -> ApiResult<()>;
    fn session_by_token_hash(&self, token_hash: &str) -> ApiResult<Option<SessionRecord>>;
    fn device(&self, device_id: &str) -> ApiResult<Option<DeviceRecord>>;
    fn upsert_device(&self, device: DeviceRecord) -> ApiResult<()>;
    fn devices_for_account(&self, account_id: &str) -> ApiResult<Vec<DeviceRecord>>;
    fn folder(&self, folder_id: &str) -> ApiResult<Option<FolderRecord>>;
    fn insert_folder(&self, folder: FolderRecord) -> ApiResult<()>;
    fn membership(&self, account_id: &str, folder_id: &str) -> ApiResult<Option<MembershipRecord>>;
    fn insert_membership(&self, membership: MembershipRecord) -> ApiResult<()>;
    fn folders_for_account(
        &self,
        account_id: &str,
    ) -> ApiResult<Vec<(MembershipRecord, FolderRecord)>>;
    fn get_cursor(&self, folder_id: &str, cursor_id: &str) -> ApiResult<Option<String>>;
    fn compare_and_set_cursor(
        &self,
        folder_id: &str,
        cursor_id: &str,
        expected: Option<&str>,
        next: &str,
    ) -> ApiResult<()>;
}

trait PackStorage: fmt::Debug + Send + Sync {
    fn label(&self) -> &'static str;
    fn put_pack(&self, folder_id: &str, revision_id: &str, bytes: &[u8]) -> ApiResult<bool>;
    fn get_pack(&self, folder_id: &str, revision_id: &str) -> ApiResult<Vec<u8>>;
    fn has_object(&self, folder_id: &str, object_id: &str) -> ApiResult<bool>;
    fn put_object(&self, folder_id: &str, object_id: &str, bytes: &[u8]) -> ApiResult<bool>;
    fn get_object(&self, folder_id: &str, object_id: &str) -> ApiResult<Vec<u8>>;
}

#[derive(Debug, Clone)]
struct LocalFilePackStorage {
    root: PathBuf,
}

impl LocalFilePackStorage {
    fn open(root: impl AsRef<Path>) -> ApiResult<Self> {
        let root = root.as_ref().to_path_buf();
        create_dir_all(root.join("packs"))?;
        Ok(Self { root })
    }

    fn pack_path(&self, folder_id: &str, revision_id: &str) -> ApiResult<PathBuf> {
        let folder_id = validate_id(folder_id, "shared folder id")?;
        let revision_id = validate_id(revision_id, "folder revision id")?;
        Ok(self
            .root
            .join("packs")
            .join(folder_id)
            .join(format!("{revision_id}.loompack")))
    }

    fn object_path(&self, folder_id: &str, object_id: &str) -> ApiResult<PathBuf> {
        let folder_id = validate_id(folder_id, "shared folder id")?;
        let object_id = validate_object_id(object_id)?;
        Ok(self.root.join("objects").join(folder_id).join(object_id))
    }
}

impl PackStorage for LocalFilePackStorage {
    fn label(&self) -> &'static str {
        "local-file"
    }

    fn put_pack(&self, folder_id: &str, revision_id: &str, bytes: &[u8]) -> ApiResult<bool> {
        let revision_id = validate_id(revision_id, "folder revision id")?;
        let path = self.pack_path(folder_id, &revision_id)?;
        if let Ok(existing) = fs::read(&path) {
            return if existing == bytes {
                Ok(false)
            } else {
                Err(ApiError::Conflict {
                    expected: Some(revision_id),
                    actual: Some("different pack bytes".to_string()),
                })
            };
        }
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        fs::write(&path, bytes).map_err(|source| ApiError::Io { path, source })?;
        Ok(true)
    }

    fn get_pack(&self, folder_id: &str, revision_id: &str) -> ApiResult<Vec<u8>> {
        let path = self.pack_path(folder_id, revision_id)?;
        fs::read(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                ApiError::NotFound("pack not found".to_string())
            } else {
                ApiError::Io { path, source }
            }
        })
    }

    fn has_object(&self, folder_id: &str, object_id: &str) -> ApiResult<bool> {
        let path = self.object_path(folder_id, object_id)?;
        match fs::metadata(&path) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(ApiError::Io { path, source }),
        }
    }

    fn put_object(&self, folder_id: &str, object_id: &str, bytes: &[u8]) -> ApiResult<bool> {
        let object_id = validate_object_payload(object_id, bytes)?;
        let path = self.object_path(folder_id, &object_id)?;
        if let Ok(existing) = fs::read(&path) {
            return if existing == bytes {
                Ok(false)
            } else {
                Err(ApiError::Conflict {
                    expected: Some(object_id),
                    actual: Some("different object bytes".to_string()),
                })
            };
        }
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        fs::write(&path, bytes).map_err(|source| ApiError::Io { path, source })?;
        Ok(true)
    }

    fn get_object(&self, folder_id: &str, object_id: &str) -> ApiResult<Vec<u8>> {
        let path = self.object_path(folder_id, object_id)?;
        fs::read(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                ApiError::NotFound("object not found".to_string())
            } else {
                ApiError::Io { path, source }
            }
        })
    }
}

#[derive(Debug)]
struct RemotePackStorage {
    provider: S3CompatibleBlobProvider,
}

impl RemotePackStorage {
    fn from_r2_env() -> ApiResult<Self> {
        let endpoint = optional_env_value("DEVBOX_R2_ENDPOINT");
        let bucket = optional_env_value("DEVBOX_R2_BUCKET");
        let (endpoint, bucket) = match (endpoint, bucket) {
            (Some(endpoint), Some(bucket)) => (endpoint, bucket),
            (None, None) => {
                return Err(ApiError::BadRequest(
                    "DEVBOX_R2_ENDPOINT and DEVBOX_R2_BUCKET are not configured".to_string(),
                ));
            }
            _ => {
                return Err(ApiError::BadRequest(
                    "DEVBOX_R2_ENDPOINT and DEVBOX_R2_BUCKET must be set together".to_string(),
                ));
            }
        };
        let region = optional_env_value("DEVBOX_R2_REGION").unwrap_or_else(|| "auto".to_string());
        let prefix = optional_env_value("DEVBOX_R2_PREFIX");
        let credentials = S3CredentialsSource::env(
            "DEVBOX_R2_ACCESS_KEY_ID",
            "DEVBOX_R2_SECRET_ACCESS_KEY",
            Some("DEVBOX_R2_SESSION_TOKEN"),
        )
        .map_err(sync_error)?;
        let config = S3CompatibleConfig::new(endpoint, bucket, region, prefix, credentials)
            .map_err(sync_error)?;
        let provider = S3CompatibleBlobProvider::from_env(config).map_err(sync_error)?;
        Ok(Self { provider })
    }

    fn pack_key(folder_id: &str, revision_id: &str) -> ApiResult<ObjectKey> {
        let folder_id = validate_id(folder_id, "shared folder id")?;
        let revision_id = validate_id(revision_id, "folder revision id")?;
        ObjectKey::new(format!("packs/{folder_id}/{revision_id}.loompack")).map_err(sync_error)
    }

    fn object_key(folder_id: &str, object_id: &str) -> ApiResult<ObjectKey> {
        let folder_id = validate_id(folder_id, "shared folder id")?;
        let object_id = validate_object_id(object_id)?;
        ObjectKey::new(format!("objects/{folder_id}/{object_id}")).map_err(sync_error)
    }
}

impl PackStorage for RemotePackStorage {
    fn label(&self) -> &'static str {
        "r2-packs"
    }

    fn put_pack(&self, folder_id: &str, revision_id: &str, bytes: &[u8]) -> ApiResult<bool> {
        let key = Self::pack_key(folder_id, revision_id)?;
        self.provider
            .put(&key, bytes)
            .map(|outcome| outcome.uploaded)
            .map_err(sync_error)
    }

    fn get_pack(&self, folder_id: &str, revision_id: &str) -> ApiResult<Vec<u8>> {
        let key = Self::pack_key(folder_id, revision_id)?;
        self.provider
            .get(&key)
            .map_err(sync_error)?
            .ok_or_else(|| ApiError::NotFound("pack not found".to_string()))
    }

    fn has_object(&self, folder_id: &str, object_id: &str) -> ApiResult<bool> {
        let key = Self::object_key(folder_id, object_id)?;
        self.provider
            .head(&key)
            .map(|metadata| metadata.is_some())
            .map_err(sync_error)
    }

    fn put_object(&self, folder_id: &str, object_id: &str, bytes: &[u8]) -> ApiResult<bool> {
        validate_object_payload(object_id, bytes)?;
        let key = Self::object_key(folder_id, object_id)?;
        self.provider
            .put(&key, bytes)
            .map(|outcome| outcome.uploaded)
            .map_err(sync_error)
    }

    fn get_object(&self, folder_id: &str, object_id: &str) -> ApiResult<Vec<u8>> {
        let key = Self::object_key(folder_id, object_id)?;
        self.provider
            .get(&key)
            .map_err(sync_error)?
            .ok_or_else(|| ApiError::NotFound("object not found".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    pub account_id: String,
    pub session_id: String,
    pub device_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRecord {
    session_id: String,
    account_id: String,
    token_hash: String,
    created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceRecord {
    account_id: String,
    device_id: String,
    display_name: String,
    registered_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderRecord {
    folder_id: String,
    owner_account_id: String,
    display_name: String,
    created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MembershipRecord {
    account_id: String,
    folder_id: String,
    role: String,
}

#[derive(Debug, Default)]
struct MemoryMetadataStore {
    state: Mutex<MemoryMetadataState>,
}

#[derive(Debug, Default)]
struct MemoryMetadataState {
    sessions: Vec<SessionRecord>,
    devices: Vec<DeviceRecord>,
    folders: Vec<FolderRecord>,
    memberships: Vec<MembershipRecord>,
    cursors: BTreeMap<String, String>,
}

impl MemoryMetadataStore {
    fn cursor_key(folder_id: &str, cursor_id: &str) -> String {
        format!("{folder_id}/{cursor_id}")
    }
}

impl ApiMetadataStore for MemoryMetadataStore {
    fn label(&self) -> &'static str {
        "memory"
    }

    fn upsert_session(&self, session: SessionRecord) -> ApiResult<()> {
        let mut state = self.state.lock().map_err(lock_error)?;
        if let Some(existing) = state
            .sessions
            .iter_mut()
            .find(|existing| existing.session_id == session.session_id)
        {
            *existing = session;
        } else {
            state.sessions.push(session);
        }
        Ok(())
    }

    fn session_by_token_hash(&self, token_hash: &str) -> ApiResult<Option<SessionRecord>> {
        Ok(self
            .state
            .lock()
            .map_err(lock_error)?
            .sessions
            .iter()
            .find(|session| session.token_hash == token_hash)
            .cloned())
    }

    fn device(&self, device_id: &str) -> ApiResult<Option<DeviceRecord>> {
        Ok(self
            .state
            .lock()
            .map_err(lock_error)?
            .devices
            .iter()
            .find(|device| device.device_id == device_id)
            .cloned())
    }

    fn upsert_device(&self, device: DeviceRecord) -> ApiResult<()> {
        let mut state = self.state.lock().map_err(lock_error)?;
        if let Some(existing) = state
            .devices
            .iter_mut()
            .find(|existing| existing.device_id == device.device_id)
        {
            *existing = device;
        } else {
            state.devices.push(device);
        }
        Ok(())
    }

    fn devices_for_account(&self, account_id: &str) -> ApiResult<Vec<DeviceRecord>> {
        let mut devices = self
            .state
            .lock()
            .map_err(lock_error)?
            .devices
            .iter()
            .filter(|device| device.account_id == account_id)
            .cloned()
            .collect::<Vec<_>>();
        devices.sort_by(|left, right| {
            left.display_name
                .cmp(&right.display_name)
                .then_with(|| left.device_id.cmp(&right.device_id))
        });
        Ok(devices)
    }

    fn folder(&self, folder_id: &str) -> ApiResult<Option<FolderRecord>> {
        Ok(self
            .state
            .lock()
            .map_err(lock_error)?
            .folders
            .iter()
            .find(|folder| folder.folder_id == folder_id)
            .cloned())
    }

    fn insert_folder(&self, folder: FolderRecord) -> ApiResult<()> {
        let mut state = self.state.lock().map_err(lock_error)?;
        if !state
            .folders
            .iter()
            .any(|existing| existing.folder_id == folder.folder_id)
        {
            state.folders.push(folder);
        }
        Ok(())
    }

    fn membership(&self, account_id: &str, folder_id: &str) -> ApiResult<Option<MembershipRecord>> {
        Ok(self
            .state
            .lock()
            .map_err(lock_error)?
            .memberships
            .iter()
            .find(|membership| {
                membership.account_id == account_id && membership.folder_id == folder_id
            })
            .cloned())
    }

    fn insert_membership(&self, membership: MembershipRecord) -> ApiResult<()> {
        let mut state = self.state.lock().map_err(lock_error)?;
        if !state.memberships.iter().any(|existing| {
            existing.account_id == membership.account_id
                && existing.folder_id == membership.folder_id
        }) {
            state.memberships.push(membership);
        }
        Ok(())
    }

    fn folders_for_account(
        &self,
        account_id: &str,
    ) -> ApiResult<Vec<(MembershipRecord, FolderRecord)>> {
        let state = self.state.lock().map_err(lock_error)?;
        Ok(state
            .memberships
            .iter()
            .filter(|membership| membership.account_id == account_id)
            .filter_map(|membership| {
                state
                    .folders
                    .iter()
                    .find(|folder| folder.folder_id == membership.folder_id)
                    .cloned()
                    .map(|folder| (membership.clone(), folder))
            })
            .collect())
    }

    fn get_cursor(&self, folder_id: &str, cursor_id: &str) -> ApiResult<Option<String>> {
        Ok(self
            .state
            .lock()
            .map_err(lock_error)?
            .cursors
            .get(&Self::cursor_key(folder_id, cursor_id))
            .cloned())
    }

    fn compare_and_set_cursor(
        &self,
        folder_id: &str,
        cursor_id: &str,
        expected: Option<&str>,
        next: &str,
    ) -> ApiResult<()> {
        let mut state = self.state.lock().map_err(lock_error)?;
        let key = Self::cursor_key(folder_id, cursor_id);
        let current = state.cursors.get(&key).cloned();
        if current.as_deref() != expected {
            return Err(ApiError::Conflict {
                expected: expected.map(ToString::to_string),
                actual: current,
            });
        }
        state.cursors.insert(key, next.to_string());
        Ok(())
    }
}

struct PostgresMetadataStore {
    client: Mutex<PostgresClient>,
}

impl fmt::Debug for PostgresMetadataStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresMetadataStore")
            .finish_non_exhaustive()
    }
}

impl PostgresMetadataStore {
    fn connect(database_url: &str) -> ApiResult<Self> {
        let attempts = optional_env_value("DEVBOX_API_DATABASE_CONNECT_ATTEMPTS")
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|attempts| *attempts > 0)
            .unwrap_or(20);
        let mut last_error = None;
        for attempt in 1..=attempts {
            match PostgresClient::connect(database_url, NoTls) {
                Ok(mut client) => {
                    Self::migrate(&mut client)?;
                    return Ok(Self {
                        client: Mutex::new(client),
                    });
                }
                Err(error) if attempt < attempts => {
                    last_error = Some(error);
                    thread::sleep(std::time::Duration::from_millis(500));
                }
                Err(error) => return Err(postgres_error(error)),
            }
        }
        Err(postgres_error(
            last_error.expect("database connect attempts is nonzero"),
        ))
    }

    fn migrate(client: &mut PostgresClient) -> ApiResult<()> {
        client
            .batch_execute(
                "
                CREATE TABLE IF NOT EXISTS api_schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS api_sessions (
                    session_id TEXT PRIMARY KEY,
                    account_id TEXT NOT NULL,
                    token_hash TEXT NOT NULL UNIQUE,
                    created_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS api_devices (
                    device_id TEXT PRIMARY KEY,
                    account_id TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    registered_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS api_shared_folders (
                    folder_id TEXT PRIMARY KEY,
                    owner_account_id TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS api_memberships (
                    account_id TEXT NOT NULL,
                    folder_id TEXT NOT NULL REFERENCES api_shared_folders(folder_id) ON DELETE CASCADE,
                    role TEXT NOT NULL,
                    PRIMARY KEY(account_id, folder_id)
                );
                CREATE TABLE IF NOT EXISTS api_cursors (
                    folder_id TEXT NOT NULL,
                    cursor_id TEXT NOT NULL,
                    revision_id TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY(folder_id, cursor_id)
                );
                INSERT INTO api_schema_migrations(version, applied_at)
                VALUES (1, 'devbox-api-postgres-v1')
                ON CONFLICT (version) DO NOTHING;
                ",
            )
            .map_err(postgres_error)
    }

    fn client(&self) -> ApiResult<std::sync::MutexGuard<'_, PostgresClient>> {
        self.client.lock().map_err(lock_error)
    }
}

impl ApiMetadataStore for PostgresMetadataStore {
    fn label(&self) -> &'static str {
        "postgres"
    }

    fn upsert_session(&self, session: SessionRecord) -> ApiResult<()> {
        self.client()?
            .execute(
                "
                INSERT INTO api_sessions(session_id, account_id, token_hash, created_at)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (session_id) DO UPDATE
                SET account_id = EXCLUDED.account_id,
                    token_hash = EXCLUDED.token_hash,
                    created_at = EXCLUDED.created_at
                ",
                &[
                    &session.session_id,
                    &session.account_id,
                    &session.token_hash,
                    &session.created_at,
                ],
            )
            .map_err(postgres_error)?;
        Ok(())
    }

    fn session_by_token_hash(&self, token_hash: &str) -> ApiResult<Option<SessionRecord>> {
        Ok(self
            .client()?
            .query_opt(
                "
                SELECT session_id, account_id, token_hash, created_at
                FROM api_sessions
                WHERE token_hash = $1
                ",
                &[&token_hash],
            )
            .map_err(postgres_error)?
            .map(|row| SessionRecord {
                session_id: row.get("session_id"),
                account_id: row.get("account_id"),
                token_hash: row.get("token_hash"),
                created_at: row.get("created_at"),
            }))
    }

    fn device(&self, device_id: &str) -> ApiResult<Option<DeviceRecord>> {
        Ok(self
            .client()?
            .query_opt(
                "
                SELECT account_id, device_id, display_name, registered_at
                FROM api_devices
                WHERE device_id = $1
                ",
                &[&device_id],
            )
            .map_err(postgres_error)?
            .map(device_from_row))
    }

    fn upsert_device(&self, device: DeviceRecord) -> ApiResult<()> {
        self.client()?
            .execute(
                "
                INSERT INTO api_devices(device_id, account_id, display_name, registered_at)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (device_id) DO UPDATE
                SET account_id = EXCLUDED.account_id,
                    display_name = EXCLUDED.display_name,
                    registered_at = EXCLUDED.registered_at
                ",
                &[
                    &device.device_id,
                    &device.account_id,
                    &device.display_name,
                    &device.registered_at,
                ],
            )
            .map_err(postgres_error)?;
        Ok(())
    }

    fn devices_for_account(&self, account_id: &str) -> ApiResult<Vec<DeviceRecord>> {
        Ok(self
            .client()?
            .query(
                "
                SELECT account_id, device_id, display_name, registered_at
                FROM api_devices
                WHERE account_id = $1
                ORDER BY display_name, device_id
                ",
                &[&account_id],
            )
            .map_err(postgres_error)?
            .into_iter()
            .map(device_from_row)
            .collect())
    }

    fn folder(&self, folder_id: &str) -> ApiResult<Option<FolderRecord>> {
        Ok(self
            .client()?
            .query_opt(
                "
                SELECT folder_id, owner_account_id, display_name, created_at
                FROM api_shared_folders
                WHERE folder_id = $1
                ",
                &[&folder_id],
            )
            .map_err(postgres_error)?
            .map(folder_from_row))
    }

    fn insert_folder(&self, folder: FolderRecord) -> ApiResult<()> {
        self.client()?
            .execute(
                "
                INSERT INTO api_shared_folders(folder_id, owner_account_id, display_name, created_at)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (folder_id) DO NOTHING
                ",
                &[
                    &folder.folder_id,
                    &folder.owner_account_id,
                    &folder.display_name,
                    &folder.created_at,
                ],
            )
            .map_err(postgres_error)?;
        Ok(())
    }

    fn membership(&self, account_id: &str, folder_id: &str) -> ApiResult<Option<MembershipRecord>> {
        Ok(self
            .client()?
            .query_opt(
                "
                SELECT account_id, folder_id, role
                FROM api_memberships
                WHERE account_id = $1 AND folder_id = $2
                ",
                &[&account_id, &folder_id],
            )
            .map_err(postgres_error)?
            .map(membership_from_row))
    }

    fn insert_membership(&self, membership: MembershipRecord) -> ApiResult<()> {
        self.client()?
            .execute(
                "
                INSERT INTO api_memberships(account_id, folder_id, role)
                VALUES ($1, $2, $3)
                ON CONFLICT (account_id, folder_id) DO NOTHING
                ",
                &[
                    &membership.account_id,
                    &membership.folder_id,
                    &membership.role,
                ],
            )
            .map_err(postgres_error)?;
        Ok(())
    }

    fn folders_for_account(
        &self,
        account_id: &str,
    ) -> ApiResult<Vec<(MembershipRecord, FolderRecord)>> {
        self.client()?
            .query(
                "
                SELECT
                    memberships.account_id,
                    memberships.folder_id,
                    memberships.role,
                    folders.owner_account_id,
                    folders.display_name,
                    folders.created_at
                FROM api_memberships memberships
                INNER JOIN api_shared_folders folders
                    ON folders.folder_id = memberships.folder_id
                WHERE memberships.account_id = $1
                ORDER BY folders.display_name, folders.folder_id
                ",
                &[&account_id],
            )
            .map_err(postgres_error)?
            .into_iter()
            .map(|row| {
                Ok((
                    MembershipRecord {
                        account_id: row.get("account_id"),
                        folder_id: row.get("folder_id"),
                        role: row.get("role"),
                    },
                    FolderRecord {
                        folder_id: row.get("folder_id"),
                        owner_account_id: row.get("owner_account_id"),
                        display_name: row.get("display_name"),
                        created_at: row.get("created_at"),
                    },
                ))
            })
            .collect()
    }

    fn get_cursor(&self, folder_id: &str, cursor_id: &str) -> ApiResult<Option<String>> {
        Ok(self
            .client()?
            .query_opt(
                "
                SELECT revision_id
                FROM api_cursors
                WHERE folder_id = $1 AND cursor_id = $2
                ",
                &[&folder_id, &cursor_id],
            )
            .map_err(postgres_error)?
            .map(|row| row.get("revision_id")))
    }

    fn compare_and_set_cursor(
        &self,
        folder_id: &str,
        cursor_id: &str,
        expected: Option<&str>,
        next: &str,
    ) -> ApiResult<()> {
        let mut client = self.client()?;
        let updated = match expected {
            Some(expected) => client
                .execute(
                    "
                    UPDATE api_cursors
                    SET revision_id = $4,
                        updated_at = $5
                    WHERE folder_id = $1
                        AND cursor_id = $2
                        AND revision_id = $3
                    ",
                    &[&folder_id, &cursor_id, &expected, &next, &timestamp()],
                )
                .map_err(postgres_error)?,
            None => client
                .execute(
                    "
                    INSERT INTO api_cursors(folder_id, cursor_id, revision_id, updated_at)
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (folder_id, cursor_id) DO NOTHING
                    ",
                    &[&folder_id, &cursor_id, &next, &timestamp()],
                )
                .map_err(postgres_error)?,
        };
        if updated == 1 {
            return Ok(());
        }
        let actual = client
            .query_opt(
                "
                SELECT revision_id
                FROM api_cursors
                WHERE folder_id = $1 AND cursor_id = $2
                ",
                &[&folder_id, &cursor_id],
            )
            .map_err(postgres_error)?
            .map(|row| row.get("revision_id"));
        Err(ApiError::Conflict {
            expected: expected.map(ToString::to_string),
            actual,
        })
    }
}

fn device_from_row(row: Row) -> DeviceRecord {
    DeviceRecord {
        account_id: row.get("account_id"),
        device_id: row.get("device_id"),
        display_name: row.get("display_name"),
        registered_at: row.get("registered_at"),
    }
}

fn folder_from_row(row: Row) -> FolderRecord {
    FolderRecord {
        folder_id: row.get("folder_id"),
        owner_account_id: row.get("owner_account_id"),
        display_name: row.get("display_name"),
        created_at: row.get("created_at"),
    }
}

fn membership_from_row(row: Row) -> MembershipRecord {
    MembershipRecord {
        account_id: row.get("account_id"),
        folder_id: row.get("folder_id"),
        role: row.get("role"),
    }
}

#[derive(Debug, Deserialize)]
struct DevSessionRequest {
    account_hint: Option<String>,
    device_id: Option<String>,
    device_display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkOsSessionRequest {
    user_id: String,
    session_id: String,
    organization_id: Option<String>,
    device_id: String,
    device_display_name: String,
}

#[derive(Debug, Deserialize)]
struct DeviceRequest {
    device_id: Option<String>,
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SharedFolderRequest {
    display_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct SharedFolderWire {
    id: String,
    account_id: String,
    role: String,
    display_name: String,
}

#[derive(Debug, Serialize)]
struct DeviceWire {
    id: String,
    account_id: String,
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct CursorCasRequest {
    expected_revision_id: Option<String>,
    next_revision_id: String,
}

#[derive(Debug, Serialize)]
struct CursorWire {
    revision_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ErrorWire {
    error: String,
    actual_revision_id: Option<String>,
}

impl LocalDevboxApi {
    pub fn open(root: impl AsRef<Path>) -> ApiResult<Self> {
        let root = root.as_ref().to_path_buf();
        let metadata = Arc::new(MemoryMetadataStore::default());
        let pack_storage = Arc::new(LocalFilePackStorage::open(&root)?);
        Self::open_with_stores(root, metadata, pack_storage)
    }

    pub fn open_from_env(root: impl AsRef<Path>) -> ApiResult<Self> {
        let root = root.as_ref().to_path_buf();
        let metadata: Arc<dyn ApiMetadataStore> =
            match optional_env_value("DEVBOX_API_METADATA_MODE").as_deref() {
                Some("memory") => Arc::new(MemoryMetadataStore::default()),
                Some(other) => {
                    return Err(ApiError::BadRequest(format!(
                        "DEVBOX_API_METADATA_MODE must be 'memory' or unset, got '{other}'"
                    )))
                }
                None => {
                    let database_url = metadata_database_url()?;
                    Arc::new(PostgresMetadataStore::connect(&database_url)?)
                }
            };
        let pack_storage: Arc<dyn PackStorage> = if optional_env_value("DEVBOX_R2_ENDPOINT")
            .is_some()
            || optional_env_value("DEVBOX_R2_BUCKET").is_some()
        {
            Arc::new(RemotePackStorage::from_r2_env()?)
        } else {
            Arc::new(LocalFilePackStorage::open(&root)?)
        };
        Self::open_with_stores(root, metadata, pack_storage)
    }

    fn open_with_stores(
        root: impl AsRef<Path>,
        metadata: Arc<dyn ApiMetadataStore>,
        pack_storage: Arc<dyn PackStorage>,
    ) -> ApiResult<Self> {
        let root = root.as_ref().to_path_buf();
        create_dir_all(&root)?;
        Ok(Self {
            root: Arc::new(root),
            metadata,
            pack_storage,
        })
    }

    #[cfg(test)]
    fn open_with_pack_storage(
        root: impl AsRef<Path>,
        pack_storage: Arc<dyn PackStorage>,
    ) -> ApiResult<Self> {
        let metadata = Arc::new(MemoryMetadataStore::default());
        Self::open_with_stores(root, metadata, pack_storage)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn pack_storage_label(&self) -> &'static str {
        self.pack_storage.label()
    }

    pub fn metadata_storage_label(&self) -> &'static str {
        self.metadata.label()
    }

    pub fn create_dev_session(
        &self,
        account_hint: Option<&str>,
        device_id: Option<&str>,
        device_display_name: Option<&str>,
    ) -> ApiResult<DevSessionResponse> {
        let account_id = safe_prefixed_id(
            "account",
            account_hint.unwrap_or("local-dev"),
            "account hint",
        )?;
        let device_id = safe_prefixed_id("device", device_id.unwrap_or("local-dev"), "device id")?;
        let session_id = format!(
            "session-{}",
            digest_hex(&format!("{account_id}:{device_id}"))
        );
        let session_token = format!(
            "devbox-local-session-{}",
            digest_hex(&format!("{account_id}:{device_id}:token"))
        );
        let now = timestamp();

        self.metadata.upsert_session(SessionRecord {
            session_id: session_id.clone(),
            account_id: account_id.clone(),
            token_hash: digest_hex(&session_token),
            created_at: now.clone(),
        })?;

        self.register_device_for_account(
            &account_id,
            &device_id,
            device_display_name.unwrap_or("Local dev device"),
        )?;

        Ok(DevSessionResponse {
            account_id,
            session_id,
            session_token,
            device_id,
        })
    }

    pub fn authenticate(&self, session_token: &str, device_id: &str) -> ApiResult<AuthContext> {
        if session_token.trim().is_empty() || device_id.trim().is_empty() {
            return Err(ApiError::Unauthorized);
        }
        let token_hash = digest_hex(session_token);
        let session = self
            .metadata
            .session_by_token_hash(&token_hash)?
            .ok_or(ApiError::Unauthorized)?;
        let device = self
            .metadata
            .device(device_id)?
            .filter(|device| device.account_id == session.account_id)
            .ok_or(ApiError::Unauthorized)?;

        Ok(AuthContext {
            account_id: session.account_id,
            session_id: session.session_id,
            device_id: device.device_id,
        })
    }

    pub fn associate_verified_workos_session(
        &self,
        verified: VerifiedWorkOsSession,
    ) -> ApiResult<DevSessionResponse> {
        let account_source = verified
            .organization_id
            .as_deref()
            .unwrap_or(verified.user_id.as_str());
        let account_id = safe_prefixed_id("account", account_source, "WorkOS account identity")?;
        let session_id =
            safe_prefixed_id("session", &verified.session_id, "WorkOS session identity")?;
        let device_id = safe_prefixed_id("device", &verified.device_id, "device id")?;
        let session_token = format!(
            "devbox-workos-session-{}",
            digest_hex(&format!(
                "{}:{}:{}:{}",
                account_id,
                session_id,
                device_id,
                timestamp()
            ))
        );

        self.metadata.upsert_session(SessionRecord {
            session_id: session_id.clone(),
            account_id: account_id.clone(),
            token_hash: digest_hex(&session_token),
            created_at: timestamp(),
        })?;
        self.register_device_for_account(&account_id, &device_id, &verified.device_display_name)?;

        Ok(DevSessionResponse {
            account_id,
            session_id,
            session_token,
            device_id,
        })
    }

    pub fn register_device(
        &self,
        auth: &AuthContext,
        device_id: Option<&str>,
        display_name: Option<&str>,
    ) -> ApiResult<DeviceResponse> {
        let device_id =
            safe_prefixed_id("device", device_id.unwrap_or(&auth.device_id), "device id")?;
        self.register_device_for_account(
            &auth.account_id,
            &device_id,
            display_name.unwrap_or("Devbox device"),
        )?;
        Ok(DeviceResponse {
            id: DeviceId::new(device_id)
                .map_err(|error| ApiError::BadRequest(error.to_string()))?,
            account_id: AccountId::new(auth.account_id.clone())
                .map_err(|error| ApiError::BadRequest(error.to_string()))?,
            display_name: display_name.unwrap_or("Devbox device").to_string(),
        })
    }

    pub fn ensure_shared_folder(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        display_name: &str,
    ) -> ApiResult<SharedFolderResponse> {
        let folder_id = validate_id(folder_id, "shared folder id")?;
        let display_name = validate_non_empty(display_name, "shared folder display name")?;
        match self.metadata.folder(&folder_id)? {
            Some(folder) if folder.owner_account_id != auth.account_id => {
                return Err(ApiError::Forbidden(
                    "shared folder belongs to another account".to_string(),
                ));
            }
            Some(_) => {}
            None => {
                self.metadata.insert_folder(FolderRecord {
                    folder_id: folder_id.clone(),
                    owner_account_id: auth.account_id.clone(),
                    display_name: display_name.clone(),
                    created_at: timestamp(),
                })?;
            }
        }

        if self
            .metadata
            .membership(&auth.account_id, &folder_id)?
            .is_none()
        {
            self.metadata.insert_membership(MembershipRecord {
                account_id: auth.account_id.clone(),
                folder_id: folder_id.clone(),
                role: "owner".to_string(),
            })?;
        }

        Ok(SharedFolderResponse {
            id: SharedFolderId::new(folder_id)
                .map_err(|error| ApiError::BadRequest(error.to_string()))?,
            account_id: AccountId::new(auth.account_id.clone())
                .map_err(|error| ApiError::BadRequest(error.to_string()))?,
            role: SharedFolderRole::Owner,
            display_name,
        })
    }

    pub fn shared_folder(
        &self,
        auth: &AuthContext,
        folder_id: &str,
    ) -> ApiResult<SharedFolderResponse> {
        self.require_membership(auth, folder_id)?;
        let folder = self
            .metadata
            .folder(folder_id)?
            .ok_or_else(|| ApiError::NotFound("shared folder not found".to_string()))?;
        Ok(SharedFolderResponse {
            id: SharedFolderId::new(folder.folder_id)
                .map_err(|error| ApiError::BadRequest(error.to_string()))?,
            account_id: AccountId::new(auth.account_id.clone())
                .map_err(|error| ApiError::BadRequest(error.to_string()))?,
            role: SharedFolderRole::Owner,
            display_name: folder.display_name,
        })
    }

    pub fn list_shared_folders(&self, auth: &AuthContext) -> ApiResult<Vec<SharedFolderResponse>> {
        let mut responses = Vec::new();

        for (membership, folder) in self.metadata.folders_for_account(&auth.account_id)? {
            responses.push(SharedFolderResponse {
                id: SharedFolderId::new(folder.folder_id)
                    .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                account_id: AccountId::new(auth.account_id.clone())
                    .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                role: role_from_string(&membership.role)?,
                display_name: folder.display_name,
            });
        }

        responses.sort_by(|left, right| {
            left.display_name
                .cmp(&right.display_name)
                .then_with(|| left.id.as_str().cmp(right.id.as_str()))
        });
        Ok(responses)
    }

    pub fn list_devices(&self, auth: &AuthContext) -> ApiResult<Vec<DeviceResponse>> {
        self.metadata
            .devices_for_account(&auth.account_id)?
            .into_iter()
            .map(|device| {
                Ok(DeviceResponse {
                    id: DeviceId::new(device.device_id)
                        .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                    account_id: AccountId::new(device.account_id)
                        .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                    display_name: device.display_name,
                })
            })
            .collect()
    }

    pub fn put_pack(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        revision_id: &str,
        bytes: &[u8],
    ) -> ApiResult<bool> {
        self.require_membership(auth, folder_id)?;
        self.pack_storage.put_pack(folder_id, revision_id, bytes)
    }

    pub fn get_pack(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        revision_id: &str,
    ) -> ApiResult<Vec<u8>> {
        self.require_membership(auth, folder_id)?;
        self.pack_storage.get_pack(folder_id, revision_id)
    }

    pub fn has_object(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        object_id: &str,
    ) -> ApiResult<bool> {
        self.require_membership(auth, folder_id)?;
        self.pack_storage.has_object(folder_id, object_id)
    }

    pub fn put_object(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        object_id: &str,
        bytes: &[u8],
    ) -> ApiResult<bool> {
        self.require_membership(auth, folder_id)?;
        self.pack_storage.put_object(folder_id, object_id, bytes)
    }

    pub fn get_object(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        object_id: &str,
    ) -> ApiResult<Vec<u8>> {
        self.require_membership(auth, folder_id)?;
        self.pack_storage.get_object(folder_id, object_id)
    }

    pub fn get_cursor(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        cursor_id: &str,
    ) -> ApiResult<Option<String>> {
        self.require_membership(auth, folder_id)?;
        let folder_id = validate_id(folder_id, "shared folder id")?;
        let cursor_id = validate_id(cursor_id, "cursor id")?;
        self.metadata.get_cursor(&folder_id, &cursor_id)
    }

    pub fn compare_and_set_cursor(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        cursor_id: &str,
        expected: Option<&str>,
        next: &str,
    ) -> ApiResult<()> {
        self.require_membership(auth, folder_id)?;
        let folder_id = validate_id(folder_id, "shared folder id")?;
        let cursor_id = validate_id(cursor_id, "cursor id")?;
        let next = validate_id(next, "folder revision id")?;
        self.metadata
            .compare_and_set_cursor(&folder_id, &cursor_id, expected, &next)
    }

    pub fn serve(self, listener: TcpListener) -> ApiResult<()> {
        let api = Arc::new(self);
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let api = api.clone();
                    if let Err(error) = handle_stream(&api, stream) {
                        eprintln!("devbox-api request failed: {error}");
                    }
                }
                Err(source) => {
                    return Err(ApiError::Io {
                        path: PathBuf::from("<tcp-listener>"),
                        source,
                    });
                }
            }
        }
        Ok(())
    }

    fn register_device_for_account(
        &self,
        account_id: &str,
        device_id: &str,
        display_name: &str,
    ) -> ApiResult<()> {
        let account_id = validate_id(account_id, "account id")?;
        let device_id = validate_id(device_id, "device id")?;
        let display_name = validate_non_empty(display_name, "device display name")?;
        match self.metadata.device(&device_id)? {
            Some(device) if device.account_id != account_id => {
                return Err(ApiError::Forbidden(
                    "device belongs to another account".to_string(),
                ));
            }
            Some(_) | None => self.metadata.upsert_device(DeviceRecord {
                account_id,
                device_id,
                display_name,
                registered_at: timestamp(),
            })?,
        }
        Ok(())
    }

    fn require_membership(&self, auth: &AuthContext, folder_id: &str) -> ApiResult<()> {
        let folder_id = validate_id(folder_id, "shared folder id")?;
        if self
            .metadata
            .membership(&auth.account_id, &folder_id)?
            .is_none()
        {
            return Err(ApiError::Forbidden(
                "session is not allowed to access this shared folder".to_string(),
            ));
        }
        Ok(())
    }
}

pub struct LocalApiServer {
    addr: SocketAddr,
    join: Option<thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
}

impl LocalApiServer {
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for LocalApiServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub fn spawn_local_test_server(root: impl AsRef<Path>) -> ApiResult<LocalApiServer> {
    let api = LocalDevboxApi::open(root)?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|source| ApiError::Io {
        path: PathBuf::from("<tcp-listener>"),
        source,
    })?;
    let addr = listener.local_addr().map_err(|source| ApiError::Io {
        path: PathBuf::from("<tcp-listener>"),
        source,
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|source| ApiError::Io {
            path: PathBuf::from("<tcp-listener>"),
            source,
        })?;
    let running = Arc::new(AtomicBool::new(true));
    let thread_running = running.clone();
    let join = thread::spawn(move || {
        while thread_running.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = handle_stream(&Arc::new(api.clone()), stream);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    Ok(LocalApiServer {
        addr,
        join: Some(join),
        running,
    })
}

fn handle_stream(api: &Arc<LocalDevboxApi>, mut stream: TcpStream) -> ApiResult<()> {
    let request = match read_http_request(&mut stream) {
        Ok(Some(request)) => request,
        Ok(None) => return Ok(()),
        Err(error) => {
            let response = http_error(error);
            let _ = stream.write_all(&response);
            return Ok(());
        }
    };
    let response = match route_request(api, request) {
        Ok(response) => response,
        Err(error) => http_error(error),
    };
    stream.write_all(&response).map_err(|source| ApiError::Io {
        path: PathBuf::from("<tcp-stream>"),
        source,
    })?;
    Ok(())
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn route_request(api: &LocalDevboxApi, request: HttpRequest) -> ApiResult<Vec<u8>> {
    let path = request
        .path
        .split('?')
        .next()
        .unwrap_or(request.path.as_str())
        .trim_end_matches('/')
        .to_string();
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    match (request.method.as_str(), parts.as_slice()) {
        ("GET", ["ready"]) => json_response(
            200,
            &serde_json::json!({
                "status": "ok",
                "service": "devbox-api",
                "metadata": api.metadata_storage_label(),
                "storage": api.pack_storage_label()
            }),
        ),
        ("POST", ["v1", "auth", "dev-session"]) => {
            let body: DevSessionRequest = json_body(&request.body)?;
            let response = api.create_dev_session(
                body.account_hint.as_deref(),
                body.device_id.as_deref(),
                body.device_display_name.as_deref(),
            )?;
            json_response(200, &response)
        }
        ("POST", ["v1", "auth", "workos-session"]) => {
            require_service_token(&request)?;
            let body: WorkOsSessionRequest = json_body(&request.body)?;
            let response = api.associate_verified_workos_session(VerifiedWorkOsSession {
                user_id: body.user_id,
                session_id: body.session_id,
                organization_id: body.organization_id,
                device_id: body.device_id,
                device_display_name: body.device_display_name,
            })?;
            json_response(200, &response)
        }
        ("POST", ["v1", "devices"]) => {
            let auth = auth_from_request(api, &request)?;
            let body: DeviceRequest = json_body(&request.body)?;
            let response = api.register_device(
                &auth,
                body.device_id.as_deref(),
                body.display_name.as_deref(),
            )?;
            json_response(
                200,
                &DeviceWire {
                    id: response.id.to_string(),
                    account_id: response.account_id.to_string(),
                    display_name: response.display_name,
                },
            )
        }
        ("GET", ["v1", "devices"]) => {
            let auth = auth_from_request(api, &request)?;
            let response = api
                .list_devices(&auth)?
                .into_iter()
                .map(|device| DeviceWire {
                    id: device.id.to_string(),
                    account_id: device.account_id.to_string(),
                    display_name: device.display_name,
                })
                .collect::<Vec<_>>();
            json_response(200, &response)
        }
        ("PUT", ["v1", "shared-folders", folder_id]) => {
            let auth = auth_from_request(api, &request)?;
            let body: SharedFolderRequest = json_body(&request.body)?;
            let response = api.ensure_shared_folder(
                &auth,
                folder_id,
                body.display_name.as_deref().unwrap_or(folder_id),
            )?;
            json_response(200, &shared_folder_wire(response))
        }
        ("GET", ["v1", "shared-folders"]) => {
            let auth = auth_from_request(api, &request)?;
            let response = api
                .list_shared_folders(&auth)?
                .into_iter()
                .map(shared_folder_wire)
                .collect::<Vec<_>>();
            json_response(200, &response)
        }
        ("GET", ["v1", "shared-folders", folder_id]) => {
            let auth = auth_from_request(api, &request)?;
            let response = api.shared_folder(&auth, folder_id)?;
            json_response(200, &shared_folder_wire(response))
        }
        ("PUT", ["v1", "loom", "shared-folders", folder_id, "packs", revision_id]) => {
            let auth = auth_from_request(api, &request)?;
            let uploaded = api.put_pack(&auth, folder_id, revision_id, &request.body)?;
            json_response(
                if uploaded { 201 } else { 200 },
                &serde_json::json!({ "uploaded": uploaded, "size_bytes": request.body.len() }),
            )
        }
        ("GET", ["v1", "loom", "shared-folders", folder_id, "packs", revision_id]) => {
            let auth = auth_from_request(api, &request)?;
            let bytes = api.get_pack(&auth, folder_id, revision_id)?;
            bytes_response(200, "application/octet-stream", bytes)
        }
        ("HEAD", ["v1", "loom", "shared-folders", folder_id, "objects", object_id]) => {
            let auth = auth_from_request(api, &request)?;
            if api.has_object(&auth, folder_id, object_id)? {
                bytes_response(200, "application/octet-stream", Vec::new())
            } else {
                Err(ApiError::NotFound("object not found".to_string()))
            }
        }
        ("PUT", ["v1", "loom", "shared-folders", folder_id, "objects", object_id]) => {
            let auth = auth_from_request(api, &request)?;
            let uploaded = api.put_object(&auth, folder_id, object_id, &request.body)?;
            json_response(
                if uploaded { 201 } else { 200 },
                &serde_json::json!({ "uploaded": uploaded, "size_bytes": request.body.len() }),
            )
        }
        ("GET", ["v1", "loom", "shared-folders", folder_id, "objects", object_id]) => {
            let auth = auth_from_request(api, &request)?;
            let bytes = api.get_object(&auth, folder_id, object_id)?;
            bytes_response(200, "application/octet-stream", bytes)
        }
        ("GET", ["v1", "loom", "shared-folders", folder_id, "cursors", cursor_id]) => {
            let auth = auth_from_request(api, &request)?;
            let revision_id = api.get_cursor(&auth, folder_id, cursor_id)?;
            json_response(200, &CursorWire { revision_id })
        }
        ("PUT", ["v1", "loom", "shared-folders", folder_id, "cursors", cursor_id]) => {
            let auth = auth_from_request(api, &request)?;
            let body: CursorCasRequest = json_body(&request.body)?;
            api.compare_and_set_cursor(
                &auth,
                folder_id,
                cursor_id,
                body.expected_revision_id.as_deref(),
                &body.next_revision_id,
            )?;
            json_response(
                200,
                &CursorWire {
                    revision_id: Some(body.next_revision_id),
                },
            )
        }
        _ => Err(ApiError::NotFound("route not found".to_string())),
    }
}

fn auth_from_request(api: &LocalDevboxApi, request: &HttpRequest) -> ApiResult<AuthContext> {
    let token = request
        .headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ApiError::Unauthorized)?;
    let device_id = request
        .headers
        .get("x-devbox-device-id")
        .ok_or(ApiError::Unauthorized)?;
    api.authenticate(token, device_id)
}

fn require_service_token(request: &HttpRequest) -> ApiResult<()> {
    let expected = optional_env_value("DEVBOX_API_SERVICE_TOKEN").ok_or(ApiError::Unauthorized)?;
    let actual = request
        .headers
        .get("x-devbox-api-service-token")
        .ok_or(ApiError::Unauthorized)?;
    if actual == &expected {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

fn read_http_request(stream: &mut TcpStream) -> ApiResult<Option<HttpRequest>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).map_err(|source| ApiError::Io {
            path: PathBuf::from("<tcp-stream>"),
            source,
        })?;
        if read == 0 && bytes.is_empty() {
            return Ok(None);
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(header_end) = find_header_end(&bytes) {
            let header_bytes = &bytes[..header_end];
            let header_text = std::str::from_utf8(header_bytes)
                .map_err(|_| ApiError::BadRequest("request headers must be UTF-8".to_string()))?;
            let mut lines = header_text.split("\r\n");
            let request_line = lines
                .next()
                .ok_or_else(|| ApiError::BadRequest("missing request line".to_string()))?;
            let request_parts = request_line.split_whitespace().collect::<Vec<_>>();
            if request_parts.len() != 3 {
                return Err(ApiError::BadRequest("invalid request line".to_string()));
            }
            let method = request_parts[0].to_string();
            let path = request_parts[1].to_string();
            let mut headers = BTreeMap::new();
            for line in lines {
                if line.is_empty() {
                    continue;
                }
                let Some((name, value)) = line.split_once(':') else {
                    return Err(ApiError::BadRequest("invalid header".to_string()));
                };
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
            let content_length = headers
                .get("content-length")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let body_start = header_end + 4;
            while bytes.len() < body_start + content_length {
                let read = stream.read(&mut buffer).map_err(|source| ApiError::Io {
                    path: PathBuf::from("<tcp-stream>"),
                    source,
                })?;
                if read == 0 {
                    return Err(ApiError::BadRequest("request body ended early".to_string()));
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            return Ok(Some(HttpRequest {
                method,
                path,
                headers,
                body: bytes[body_start..body_start + content_length].to_vec(),
            }));
        }
        if read == 0 {
            return Err(ApiError::BadRequest("incomplete request".to_string()));
        }
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn json_body<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> ApiResult<T> {
    if bytes.is_empty() {
        serde_json::from_slice(b"{}").map_err(|error| ApiError::Json(error.to_string()))
    } else {
        serde_json::from_slice(bytes).map_err(|error| ApiError::Json(error.to_string()))
    }
}

fn shared_folder_wire(response: SharedFolderResponse) -> SharedFolderWire {
    SharedFolderWire {
        id: response.id.to_string(),
        account_id: response.account_id.to_string(),
        role: role_to_string(response.role),
        display_name: response.display_name,
    }
}

fn role_to_string(role: SharedFolderRole) -> String {
    match role {
        SharedFolderRole::Owner => "owner",
        SharedFolderRole::Editor => "editor",
        SharedFolderRole::Viewer => "viewer",
    }
    .to_string()
}

fn role_from_string(value: &str) -> ApiResult<SharedFolderRole> {
    match value {
        "owner" => Ok(SharedFolderRole::Owner),
        "editor" => Ok(SharedFolderRole::Editor),
        "viewer" => Ok(SharedFolderRole::Viewer),
        _ => Err(ApiError::BadRequest(
            "shared folder role is invalid".to_string(),
        )),
    }
}

fn json_response<T: Serialize>(status: u16, body: &T) -> ApiResult<Vec<u8>> {
    let bytes = serde_json::to_vec(body).map_err(|error| ApiError::Json(error.to_string()))?;
    bytes_response(status, "application/json", bytes)
}

fn bytes_response(status: u16, content_type: &str, body: Vec<u8>) -> ApiResult<Vec<u8>> {
    let reason = status_reason(status);
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);
    Ok(response)
}

fn http_error(error: ApiError) -> Vec<u8> {
    let status = error.status_code();
    let actual_revision_id = match &error {
        ApiError::Conflict { actual, .. } => actual.clone(),
        _ => None,
    };
    let body = ErrorWire {
        error: error.to_string(),
        actual_revision_id,
    };
    json_response(status, &body).unwrap_or_else(|_| {
        b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            .to_vec()
    })
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Internal Server Error",
    }
}

fn safe_prefixed_id(prefix: &str, value: &str, label: &'static str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::BadRequest(format!("{label} cannot be empty")));
    }
    let safe = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let suffix = if safe.is_empty() {
        digest_hex(value)[..16].to_string()
    } else {
        safe
    };
    Ok(if suffix.starts_with(&format!("{prefix}-")) {
        suffix
    } else {
        format!("{prefix}-{suffix}")
    })
}

fn validate_id(value: &str, label: &'static str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value.contains("..")
        || value
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.'))
    {
        return Err(ApiError::BadRequest(format!(
            "{label} must be a safe identifier"
        )));
    }
    Ok(value.to_string())
}

fn validate_object_id(value: &str) -> ApiResult<String> {
    ObjectId::from_blake3_hex(value)
        .map(|object_id| object_id.to_string())
        .map_err(|error| ApiError::BadRequest(error.to_string()))
}

fn validate_object_payload(object_id: &str, bytes: &[u8]) -> ApiResult<String> {
    let object_id = validate_object_id(object_id)?;
    let actual = blake3::hash(bytes).to_hex().to_string();
    if actual != object_id {
        return Err(ApiError::BadRequest(
            "object bytes do not match object id".to_string(),
        ));
    }
    Ok(object_id)
}

fn validate_non_empty(value: &str, label: &'static str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::BadRequest(format!("{label} cannot be empty")));
    }
    Ok(value.to_string())
}

fn create_dir_all(path: impl AsRef<Path>) -> ApiResult<()> {
    let path = path.as_ref();
    fs::create_dir_all(path).map_err(|source| ApiError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn optional_env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn metadata_database_url() -> ApiResult<String> {
    optional_env_value("DEVBOX_API_DATABASE_URL")
        .or_else(|| optional_env_value("DATABASE_URL"))
        .ok_or_else(|| {
            ApiError::BadRequest(
                "metadata database is required: set DATABASE_URL or DEVBOX_API_DATABASE_URL"
                    .to_string(),
            )
        })
}

fn postgres_error(error: postgres::Error) -> ApiError {
    ApiError::Database(error.to_string())
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> ApiError {
    ApiError::Database("metadata store lock was poisoned".to_string())
}

fn sync_error(error: SyncError) -> ApiError {
    ApiError::RemoteStorage(error.to_string())
}

fn digest_hex(value: &str) -> String {
    blake3::hash(value.as_bytes()).to_hex().to_string()
}

fn timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("unix:{}", duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Debug, Default)]
    struct MemoryPackStorage {
        packs: Mutex<BTreeMap<String, Vec<u8>>>,
        objects: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    impl MemoryPackStorage {
        fn key(folder_id: &str, revision_id: &str) -> String {
            format!("{folder_id}/{revision_id}")
        }

        fn object_key(folder_id: &str, object_id: &str) -> String {
            format!("{folder_id}/{object_id}")
        }
    }

    impl PackStorage for MemoryPackStorage {
        fn label(&self) -> &'static str {
            "test-remote"
        }

        fn put_pack(&self, folder_id: &str, revision_id: &str, bytes: &[u8]) -> ApiResult<bool> {
            validate_id(folder_id, "shared folder id")?;
            validate_id(revision_id, "folder revision id")?;
            let key = Self::key(folder_id, revision_id);
            let mut packs = self.packs.lock().expect("memory storage lock");
            if let Some(existing) = packs.get(&key) {
                return if existing.as_slice() == bytes {
                    Ok(false)
                } else {
                    Err(ApiError::Conflict {
                        expected: Some(revision_id.to_string()),
                        actual: Some("different pack bytes".to_string()),
                    })
                };
            }
            packs.insert(key, bytes.to_vec());
            Ok(true)
        }

        fn get_pack(&self, folder_id: &str, revision_id: &str) -> ApiResult<Vec<u8>> {
            validate_id(folder_id, "shared folder id")?;
            validate_id(revision_id, "folder revision id")?;
            self.packs
                .lock()
                .expect("memory storage lock")
                .get(&Self::key(folder_id, revision_id))
                .cloned()
                .ok_or_else(|| ApiError::NotFound("pack not found".to_string()))
        }

        fn has_object(&self, folder_id: &str, object_id: &str) -> ApiResult<bool> {
            validate_id(folder_id, "shared folder id")?;
            validate_object_id(object_id)?;
            Ok(self
                .objects
                .lock()
                .expect("memory storage lock")
                .contains_key(&Self::object_key(folder_id, object_id)))
        }

        fn put_object(&self, folder_id: &str, object_id: &str, bytes: &[u8]) -> ApiResult<bool> {
            validate_id(folder_id, "shared folder id")?;
            validate_object_payload(object_id, bytes)?;
            let key = Self::object_key(folder_id, object_id);
            let mut objects = self.objects.lock().expect("memory storage lock");
            if let Some(existing) = objects.get(&key) {
                return if existing.as_slice() == bytes {
                    Ok(false)
                } else {
                    Err(ApiError::Conflict {
                        expected: Some(object_id.to_string()),
                        actual: Some("different object bytes".to_string()),
                    })
                };
            }
            objects.insert(key, bytes.to_vec());
            Ok(true)
        }

        fn get_object(&self, folder_id: &str, object_id: &str) -> ApiResult<Vec<u8>> {
            validate_id(folder_id, "shared folder id")?;
            validate_object_id(object_id)?;
            self.objects
                .lock()
                .expect("memory storage lock")
                .get(&Self::object_key(folder_id, object_id))
                .cloned()
                .ok_or_else(|| ApiError::NotFound("object not found".to_string()))
        }
    }

    #[test]
    fn api_boundary_delegates_folder_state_to_loom() {
        let boundary = ApiBoundary::devbox_hosted_api();

        assert!(boundary.owns_accounts);
        assert!(boundary.owns_devices);
        assert!(boundary.owns_shared_folder_membership);
        assert!(!boundary.owns_folder_state_semantics);
    }

    #[test]
    fn api_routes_include_loom_remote_without_git_route_names() {
        let paths = API_ROUTES
            .iter()
            .map(|route| route.path)
            .collect::<Vec<_>>();

        assert!(paths.contains(&"/v1/loom"));
        assert!(!paths.iter().any(|path| path.contains("git")));
    }

    #[test]
    fn open_from_env_can_use_memory_metadata_for_local_smoke() {
        let _guard = ENV_LOCK.lock().expect("env test lock");
        let old_mode = std::env::var("DEVBOX_API_METADATA_MODE").ok();
        let old_database_url = std::env::var("DEVBOX_API_DATABASE_URL").ok();
        let old_database_url_fallback = std::env::var("DATABASE_URL").ok();
        std::env::set_var("DEVBOX_API_METADATA_MODE", "memory");
        std::env::remove_var("DEVBOX_API_DATABASE_URL");
        std::env::remove_var("DATABASE_URL");
        let dir = tempfile::tempdir().expect("temp dir");

        let api =
            LocalDevboxApi::open_from_env(dir.path()).expect("api opens with memory metadata");

        assert_eq!(api.metadata_storage_label(), "memory");
        assert_eq!(api.pack_storage_label(), "local-file");

        match old_mode {
            Some(value) => std::env::set_var("DEVBOX_API_METADATA_MODE", value),
            None => std::env::remove_var("DEVBOX_API_METADATA_MODE"),
        }
        match old_database_url {
            Some(value) => std::env::set_var("DEVBOX_API_DATABASE_URL", value),
            None => std::env::remove_var("DEVBOX_API_DATABASE_URL"),
        }
        match old_database_url_fallback {
            Some(value) => std::env::set_var("DATABASE_URL", value),
            None => std::env::remove_var("DATABASE_URL"),
        }
    }

    #[test]
    fn session_device_folder_pack_and_cursor_flow_is_scoped() {
        let dir = tempfile::tempdir().expect("temp dir");
        let api = LocalDevboxApi::open(dir.path()).expect("api opens");
        let session = api
            .create_dev_session(Some("alice"), Some("laptop"), Some("Laptop"))
            .expect("session creates");
        let auth = api
            .authenticate(&session.session_token, &session.device_id)
            .expect("auth works");
        let folder = api
            .ensure_shared_folder(&auth, "shared-folder-1", "Code")
            .expect("folder creates");
        let folders = api
            .list_shared_folders(&auth)
            .expect("folders list for account");
        let devices = api.list_devices(&auth).expect("devices list for account");

        assert_eq!(folder.id.as_str(), "shared-folder-1");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].display_name, "Code");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].display_name, "Laptop");
        assert!(api
            .put_pack(&auth, "shared-folder-1", "folder-revision-1", b"pack")
            .expect("pack writes"));
        assert_eq!(
            api.get_pack(&auth, "shared-folder-1", "folder-revision-1")
                .expect("pack reads"),
            b"pack"
        );
        let object_bytes = b"object-bytes";
        let object_id = object_id_for(object_bytes);
        assert!(!api
            .has_object(&auth, "shared-folder-1", &object_id)
            .expect("object head reads"));
        assert!(api
            .put_object(&auth, "shared-folder-1", &object_id, object_bytes)
            .expect("object writes"));
        assert!(api
            .has_object(&auth, "shared-folder-1", &object_id)
            .expect("object head reads"));
        assert_eq!(
            api.get_object(&auth, "shared-folder-1", &object_id)
                .expect("object reads"),
            object_bytes
        );
        assert_eq!(
            api.get_cursor(&auth, "shared-folder-1", "shared-folder")
                .expect("cursor reads"),
            None
        );
        api.compare_and_set_cursor(
            &auth,
            "shared-folder-1",
            "shared-folder",
            None,
            "folder-revision-1",
        )
        .expect("cursor advances");
        let stale = api
            .compare_and_set_cursor(
                &auth,
                "shared-folder-1",
                "shared-folder",
                None,
                "folder-revision-2",
            )
            .expect_err("stale cursor refuses");
        assert!(matches!(stale, ApiError::Conflict { .. }));
    }

    #[test]
    fn api_can_use_non_local_remote_storage() {
        let dir = tempfile::tempdir().expect("temp dir");
        let api = LocalDevboxApi::open_with_pack_storage(
            dir.path(),
            Arc::new(MemoryPackStorage::default()),
        )
        .expect("api opens");
        let session = api
            .create_dev_session(Some("alice"), Some("laptop"), Some("Laptop"))
            .expect("session creates");
        let auth = api
            .authenticate(&session.session_token, &session.device_id)
            .expect("auth works");
        api.ensure_shared_folder(&auth, "shared-folder-1", "Code")
            .expect("folder creates");

        assert_eq!(api.pack_storage_label(), "test-remote");
        assert!(api
            .put_pack(
                &auth,
                "shared-folder-1",
                "folder-revision-1",
                b"remote-pack"
            )
            .expect("pack writes"));
        assert_eq!(
            api.get_pack(&auth, "shared-folder-1", "folder-revision-1")
                .expect("pack reads"),
            b"remote-pack"
        );
        let object_bytes = b"remote-object";
        let object_id = object_id_for(object_bytes);
        assert!(api
            .put_object(&auth, "shared-folder-1", &object_id, object_bytes)
            .expect("object writes"));
        assert_eq!(
            api.get_object(&auth, "shared-folder-1", &object_id)
                .expect("object reads"),
            object_bytes
        );
        assert!(
            !dir.path().join("packs").exists(),
            "custom pack storage should not create local pack files"
        );
        assert!(
            !dir.path().join("objects").exists(),
            "custom object storage should not create local object files"
        );
    }

    #[test]
    fn wrong_session_and_wrong_account_fail_closed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let api = LocalDevboxApi::open(dir.path()).expect("api opens");
        let alice = api
            .create_dev_session(Some("alice"), Some("laptop"), Some("Laptop"))
            .expect("alice session");
        let bob = api
            .create_dev_session(Some("bob"), Some("desktop"), Some("Desktop"))
            .expect("bob session");
        let alice_auth = api
            .authenticate(&alice.session_token, &alice.device_id)
            .expect("alice auth");
        let bob_auth = api
            .authenticate(&bob.session_token, &bob.device_id)
            .expect("bob auth");
        api.ensure_shared_folder(&alice_auth, "shared-folder-1", "Code")
            .expect("alice folder");

        assert!(matches!(
            api.authenticate("wrong-token", &alice.device_id),
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            api.get_cursor(&bob_auth, "shared-folder-1", "shared-folder"),
            Err(ApiError::Forbidden(_))
        ));
        assert!(matches!(
            api.has_object(
                &bob_auth,
                "shared-folder-1",
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
            ),
            Err(ApiError::Forbidden(_))
        ));
        assert!(matches!(
            api.get_pack(&bob_auth, "shared-folder-1", "folder-revision-1"),
            Err(ApiError::Forbidden(_))
        ));
        assert!(matches!(
            api.ensure_shared_folder(&bob_auth, "shared-folder-1", "Code"),
            Err(ApiError::Forbidden(_))
        ));
    }

    #[test]
    fn workos_session_association_uses_verified_boundary() {
        let dir = tempfile::tempdir().expect("temp dir");
        let api = LocalDevboxApi::open(dir.path()).expect("api opens");
        let session = api
            .associate_verified_workos_session(VerifiedWorkOsSession {
                user_id: "user_123".to_string(),
                session_id: "session_123".to_string(),
                organization_id: Some("org_123".to_string()),
                device_id: "browser".to_string(),
                device_display_name: "Browser session".to_string(),
            })
            .expect("verified WorkOS session associates");
        let authenticated = api
            .authenticate(&session.session_token, &session.device_id)
            .expect("stored Devbox session token authenticates");
        let devices = api
            .list_devices(&authenticated)
            .expect("associated device lists");

        assert_ne!(session.session_token, "verified-workos-access-token");
        assert_eq!(authenticated.account_id, "account-org_123");
        assert_eq!(authenticated.session_id, "session-session_123");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].display_name, "Browser session");
        assert!(matches!(
            api.authenticate("verified-workos-access-token", &session.device_id),
            Err(ApiError::Unauthorized)
        ));
    }

    #[test]
    fn hosted_workos_exchange_requires_service_token_and_returns_devbox_session() {
        let _guard = ENV_LOCK.lock().expect("env test lock");
        let old_service_token = std::env::var("DEVBOX_API_SERVICE_TOKEN").ok();
        std::env::set_var("DEVBOX_API_SERVICE_TOKEN", "service-secret");
        let dir = tempfile::tempdir().expect("temp dir");
        let api = LocalDevboxApi::open(dir.path()).expect("api opens");

        let missing_service_token = route_request(
            &api,
            HttpRequest {
                method: "POST".to_string(),
                path: "/v1/auth/workos-session".to_string(),
                headers: BTreeMap::new(),
                body: br#"{
                    "user_id": "user_123",
                    "session_id": "session_123",
                    "organization_id": "org_123",
                    "device_id": "browser",
                    "device_display_name": "Browser session"
                }"#
                .to_vec(),
            },
        )
        .expect_err("service token is required");
        assert!(matches!(missing_service_token, ApiError::Unauthorized));

        let mut exchange_headers = BTreeMap::new();
        exchange_headers.insert(
            "x-devbox-api-service-token".to_string(),
            "service-secret".to_string(),
        );
        let response = route_request(
            &api,
            HttpRequest {
                method: "POST".to_string(),
                path: "/v1/auth/workos-session".to_string(),
                headers: exchange_headers,
                body: br#"{
                    "user_id": "user_123",
                    "session_id": "session_123",
                    "organization_id": "org_123",
                    "device_id": "browser",
                    "device_display_name": "Browser session"
                }"#
                .to_vec(),
            },
        )
        .expect("verified WorkOS exchange succeeds");
        let text = String::from_utf8(response).expect("response is utf8");
        let body = text.split("\r\n\r\n").nth(1).expect("response has body");
        let session: DevSessionResponse =
            serde_json::from_str(body).expect("session response parses");

        assert!(session.session_token.starts_with("devbox-workos-session-"));
        assert_ne!(session.session_token, "workos-access-token");

        let mut raw_workos_headers = BTreeMap::new();
        raw_workos_headers.insert(
            "authorization".to_string(),
            "Bearer workos-access-token".to_string(),
        );
        raw_workos_headers.insert("x-devbox-device-id".to_string(), session.device_id.clone());
        assert!(matches!(
            route_request(
                &api,
                HttpRequest {
                    method: "GET".to_string(),
                    path: "/v1/devices".to_string(),
                    headers: raw_workos_headers,
                    body: Vec::new(),
                },
            ),
            Err(ApiError::Unauthorized)
        ));

        let mut devbox_headers = BTreeMap::new();
        devbox_headers.insert(
            "authorization".to_string(),
            format!("Bearer {}", session.session_token),
        );
        devbox_headers.insert("x-devbox-device-id".to_string(), session.device_id);
        let devices_response = route_request(
            &api,
            HttpRequest {
                method: "GET".to_string(),
                path: "/v1/devices".to_string(),
                headers: devbox_headers,
                body: Vec::new(),
            },
        )
        .expect("Devbox session token can list devices");
        let devices_text = String::from_utf8(devices_response).expect("response is utf8");

        assert!(devices_text.starts_with("HTTP/1.1 200 OK"));
        assert!(devices_text.contains("Browser session"));

        match old_service_token {
            Some(value) => std::env::set_var("DEVBOX_API_SERVICE_TOKEN", value),
            None => std::env::remove_var("DEVBOX_API_SERVICE_TOKEN"),
        }
    }

    #[test]
    fn object_upload_rejects_hash_mismatch_without_persisting_bytes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let api = LocalDevboxApi::open(dir.path()).expect("api opens");
        let session = api
            .create_dev_session(Some("alice"), Some("laptop"), Some("Laptop"))
            .expect("session creates");
        let auth = api
            .authenticate(&session.session_token, &session.device_id)
            .expect("auth works");
        api.ensure_shared_folder(&auth, "shared-folder-1", "Code")
            .expect("folder creates");
        let object_id = object_id_for(b"correct bytes");

        let error = api
            .put_object(&auth, "shared-folder-1", &object_id, b"wrong bytes")
            .expect_err("mismatched object body is rejected");

        assert!(matches!(
            error,
            ApiError::BadRequest(message)
                if message.contains("object bytes do not match object id")
        ));
        assert!(!api
            .has_object(&auth, "shared-folder-1", &object_id)
            .expect("object head reads"));
        assert!(matches!(
            api.get_object(&auth, "shared-folder-1", &object_id),
            Err(ApiError::NotFound(_))
        ));
    }

    #[test]
    fn ready_route_reports_service_health_without_auth() {
        let dir = tempfile::tempdir().expect("temp dir");
        let api = LocalDevboxApi::open(dir.path()).expect("api opens");
        let response = route_request(
            &api,
            HttpRequest {
                method: "GET".to_string(),
                path: "/ready".to_string(),
                headers: BTreeMap::new(),
                body: Vec::new(),
            },
        )
        .expect("ready responds");
        let text = String::from_utf8(response).expect("response is utf8");

        assert!(text.starts_with("HTTP/1.1 200 OK"));
        assert!(text.contains("\"service\":\"devbox-api\""));
        assert!(text.contains("\"metadata\":\"memory\""));
        assert!(text.contains("\"storage\":\"local-file\""));
    }

    fn object_id_for(bytes: &[u8]) -> String {
        blake3::hash(bytes).to_hex().to_string()
    }
}
