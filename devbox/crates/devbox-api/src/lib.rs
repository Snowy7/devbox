//! Devbox hosted API boundary.
//!
//! This crate provides the PR6 local hosted API used by Loom's hosted remote.
//! Devbox owns sessions, devices, shared-folder membership, pack storage, and
//! cursor metadata. Loom still owns folder-state semantics inside the packs.

use devbox_platform::{AccountId, DeviceId, SharedFolderRole};
use loom_core::{CursorId, FolderRevisionId, SharedFolderId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
            Self::Io { .. } => 500,
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
            | Self::Json(_) => None,
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Clone)]
pub struct LocalDevboxApi {
    root: Arc<PathBuf>,
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

#[derive(Debug, Deserialize)]
struct DevSessionRequest {
    account_hint: Option<String>,
    device_id: Option<String>,
    device_display_name: Option<String>,
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
        create_dir_all(&root)?;
        create_dir_all(root.join("metadata"))?;
        create_dir_all(root.join("packs"))?;
        create_dir_all(root.join("objects"))?;
        create_dir_all(root.join("cursors"))?;
        Ok(Self {
            root: Arc::new(root),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
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

        let mut sessions = self.sessions()?;
        upsert_session(
            &mut sessions,
            SessionRecord {
                session_id: session_id.clone(),
                account_id: account_id.clone(),
                token_hash: digest_hex(&session_token),
                created_at: now.clone(),
            },
        );
        self.write_sessions(&sessions)?;

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
            .sessions()?
            .into_iter()
            .find(|session| session.token_hash == token_hash)
            .ok_or(ApiError::Unauthorized)?;
        let device = self
            .devices()?
            .into_iter()
            .find(|device| device.device_id == device_id && device.account_id == session.account_id)
            .ok_or(ApiError::Unauthorized)?;

        Ok(AuthContext {
            account_id: session.account_id,
            session_id: session.session_id,
            device_id: device.device_id,
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
        let mut folders = self.folders()?;
        match folders
            .iter()
            .find(|folder| folder.folder_id == folder_id)
            .cloned()
        {
            Some(folder) if folder.owner_account_id != auth.account_id => {
                return Err(ApiError::Forbidden(
                    "shared folder belongs to another account".to_string(),
                ));
            }
            Some(_) => {}
            None => {
                folders.push(FolderRecord {
                    folder_id: folder_id.clone(),
                    owner_account_id: auth.account_id.clone(),
                    display_name: display_name.clone(),
                    created_at: timestamp(),
                });
                self.write_folders(&folders)?;
            }
        }

        let mut memberships = self.memberships()?;
        if !memberships.iter().any(|membership| {
            membership.account_id == auth.account_id && membership.folder_id == folder_id
        }) {
            memberships.push(MembershipRecord {
                account_id: auth.account_id.clone(),
                folder_id: folder_id.clone(),
                role: "owner".to_string(),
            });
            self.write_memberships(&memberships)?;
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
            .folders()?
            .into_iter()
            .find(|folder| folder.folder_id == folder_id)
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
        let folders = self.folders()?;
        let memberships = self.memberships()?;
        let mut responses = Vec::new();

        for membership in memberships
            .into_iter()
            .filter(|membership| membership.account_id == auth.account_id)
        {
            let Some(folder) = folders
                .iter()
                .find(|folder| folder.folder_id == membership.folder_id)
            else {
                continue;
            };
            responses.push(SharedFolderResponse {
                id: SharedFolderId::new(folder.folder_id.clone())
                    .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                account_id: AccountId::new(auth.account_id.clone())
                    .map_err(|error| ApiError::BadRequest(error.to_string()))?,
                role: role_from_string(&membership.role)?,
                display_name: folder.display_name.clone(),
            });
        }

        responses.sort_by(|left, right| {
            left.display_name
                .cmp(&right.display_name)
                .then_with(|| left.id.as_str().cmp(right.id.as_str()))
        });
        Ok(responses)
    }

    pub fn put_pack(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        revision_id: &str,
        bytes: &[u8],
    ) -> ApiResult<bool> {
        self.require_membership(auth, folder_id)?;
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

    pub fn get_pack(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        revision_id: &str,
    ) -> ApiResult<Vec<u8>> {
        self.require_membership(auth, folder_id)?;
        let path = self.pack_path(folder_id, revision_id)?;
        fs::read(&path).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                ApiError::NotFound("pack not found".to_string())
            } else {
                ApiError::Io { path, source }
            }
        })
    }

    pub fn get_cursor(
        &self,
        auth: &AuthContext,
        folder_id: &str,
        cursor_id: &str,
    ) -> ApiResult<Option<String>> {
        self.require_membership(auth, folder_id)?;
        let path = self.cursor_path(folder_id, cursor_id)?;
        match fs::read_to_string(&path) {
            Ok(contents) => {
                let value = contents.trim();
                if value.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(value.to_string()))
                }
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(ApiError::Io { path, source }),
        }
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
        let next = validate_id(next, "folder revision id")?;
        let current = self.get_cursor(auth, folder_id, cursor_id)?;
        if current.as_deref() != expected {
            return Err(ApiError::Conflict {
                expected: expected.map(ToString::to_string),
                actual: current,
            });
        }
        let path = self.cursor_path(folder_id, cursor_id)?;
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        fs::write(&path, format!("{next}\n")).map_err(|source| ApiError::Io { path, source })
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
        let mut devices = self.devices()?;
        match devices
            .iter_mut()
            .find(|device| device.device_id == device_id)
        {
            Some(device) if device.account_id != account_id => {
                return Err(ApiError::Forbidden(
                    "device belongs to another account".to_string(),
                ));
            }
            Some(device) => {
                device.display_name = display_name;
            }
            None => devices.push(DeviceRecord {
                account_id,
                device_id,
                display_name,
                registered_at: timestamp(),
            }),
        }
        self.write_devices(&devices)
    }

    fn require_membership(&self, auth: &AuthContext, folder_id: &str) -> ApiResult<()> {
        let folder_id = validate_id(folder_id, "shared folder id")?;
        let has_membership = self.memberships()?.iter().any(|membership| {
            membership.account_id == auth.account_id && membership.folder_id == folder_id
        });
        if !has_membership {
            return Err(ApiError::Forbidden(
                "session is not allowed to access this shared folder".to_string(),
            ));
        }
        Ok(())
    }

    fn metadata_path(&self, name: &str) -> PathBuf {
        self.root.join("metadata").join(name)
    }

    fn sessions(&self) -> ApiResult<Vec<SessionRecord>> {
        read_rows(&self.metadata_path("sessions.tsv")).map(|rows| {
            rows.into_iter()
                .filter_map(|row| {
                    Some(SessionRecord {
                        session_id: row.get("session_id")?.clone(),
                        account_id: row.get("account_id")?.clone(),
                        token_hash: row.get("token_hash")?.clone(),
                        created_at: row.get("created_at")?.clone(),
                    })
                })
                .collect()
        })
    }

    fn write_sessions(&self, sessions: &[SessionRecord]) -> ApiResult<()> {
        write_rows(
            &self.metadata_path("sessions.tsv"),
            &["session_id", "account_id", "token_hash", "created_at"],
            sessions.iter().map(|session| {
                vec![
                    session.session_id.clone(),
                    session.account_id.clone(),
                    session.token_hash.clone(),
                    session.created_at.clone(),
                ]
            }),
        )
    }

    fn devices(&self) -> ApiResult<Vec<DeviceRecord>> {
        read_rows(&self.metadata_path("devices.tsv")).map(|rows| {
            rows.into_iter()
                .filter_map(|row| {
                    Some(DeviceRecord {
                        account_id: row.get("account_id")?.clone(),
                        device_id: row.get("device_id")?.clone(),
                        display_name: row.get("display_name")?.clone(),
                        registered_at: row.get("registered_at")?.clone(),
                    })
                })
                .collect()
        })
    }

    fn write_devices(&self, devices: &[DeviceRecord]) -> ApiResult<()> {
        write_rows(
            &self.metadata_path("devices.tsv"),
            &["account_id", "device_id", "display_name", "registered_at"],
            devices.iter().map(|device| {
                vec![
                    device.account_id.clone(),
                    device.device_id.clone(),
                    device.display_name.clone(),
                    device.registered_at.clone(),
                ]
            }),
        )
    }

    fn folders(&self) -> ApiResult<Vec<FolderRecord>> {
        read_rows(&self.metadata_path("folders.tsv")).map(|rows| {
            rows.into_iter()
                .filter_map(|row| {
                    Some(FolderRecord {
                        folder_id: row.get("folder_id")?.clone(),
                        owner_account_id: row.get("owner_account_id")?.clone(),
                        display_name: row.get("display_name")?.clone(),
                        created_at: row.get("created_at")?.clone(),
                    })
                })
                .collect()
        })
    }

    fn write_folders(&self, folders: &[FolderRecord]) -> ApiResult<()> {
        write_rows(
            &self.metadata_path("folders.tsv"),
            &[
                "folder_id",
                "owner_account_id",
                "display_name",
                "created_at",
            ],
            folders.iter().map(|folder| {
                vec![
                    folder.folder_id.clone(),
                    folder.owner_account_id.clone(),
                    folder.display_name.clone(),
                    folder.created_at.clone(),
                ]
            }),
        )
    }

    fn memberships(&self) -> ApiResult<Vec<MembershipRecord>> {
        read_rows(&self.metadata_path("memberships.tsv")).map(|rows| {
            rows.into_iter()
                .filter_map(|row| {
                    Some(MembershipRecord {
                        account_id: row.get("account_id")?.clone(),
                        folder_id: row.get("folder_id")?.clone(),
                        role: row.get("role")?.clone(),
                    })
                })
                .collect()
        })
    }

    fn write_memberships(&self, memberships: &[MembershipRecord]) -> ApiResult<()> {
        write_rows(
            &self.metadata_path("memberships.tsv"),
            &["account_id", "folder_id", "role"],
            memberships.iter().map(|membership| {
                vec![
                    membership.account_id.clone(),
                    membership.folder_id.clone(),
                    membership.role.clone(),
                ]
            }),
        )
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

    fn cursor_path(&self, folder_id: &str, cursor_id: &str) -> ApiResult<PathBuf> {
        let folder_id = validate_id(folder_id, "shared folder id")?;
        let cursor_id = validate_id(cursor_id, "cursor id")?;
        Ok(self
            .root
            .join("cursors")
            .join(folder_id)
            .join(format!("{cursor_id}.txt")))
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
                "storage": "local-file"
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

fn upsert_session(sessions: &mut Vec<SessionRecord>, next: SessionRecord) {
    if let Some(session) = sessions
        .iter_mut()
        .find(|session| session.session_id == next.session_id)
    {
        *session = next;
    } else {
        sessions.push(next);
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

fn validate_non_empty(value: &str, label: &'static str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::BadRequest(format!("{label} cannot be empty")));
    }
    Ok(value.to_string())
}

fn read_rows(path: &Path) -> ApiResult<Vec<BTreeMap<String, String>>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ApiError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    let mut lines = contents.lines();
    let Some(header) = lines.next() else {
        return Ok(Vec::new());
    };
    let fields = header
        .split('\t')
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let values = line
            .split('\t')
            .map(decode_field)
            .collect::<ApiResult<Vec<_>>>()?;
        let mut row = BTreeMap::new();
        for (name, value) in fields.iter().zip(values) {
            row.insert(name.clone(), value);
        }
        rows.push(row);
    }
    Ok(rows)
}

fn write_rows(
    path: &Path,
    headers: &[&str],
    rows: impl IntoIterator<Item = Vec<String>>,
) -> ApiResult<()> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|source| ApiError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(format!("{}\n", headers.join("\t")).as_bytes())
        .map_err(|source| ApiError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    for row in rows {
        let line = row
            .iter()
            .map(|value| encode_field(value))
            .collect::<Vec<_>>()
            .join("\t");
        file.write_all(format!("{line}\n").as_bytes())
            .map_err(|source| ApiError::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }
    Ok(())
}

fn encode_field(value: &str) -> String {
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

fn decode_field(value: &str) -> ApiResult<String> {
    let mut decoded = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(ApiError::Json("truncated percent escape".to_string()));
            }
            let hex = &value[index + 1..index + 3];
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| ApiError::Json("invalid percent escape".to_string()))?;
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

fn create_dir_all(path: impl AsRef<Path>) -> ApiResult<()> {
    let path = path.as_ref();
    fs::create_dir_all(path).map_err(|source| ApiError::Io {
        path: path.to_path_buf(),
        source,
    })
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

        assert_eq!(folder.id.as_str(), "shared-folder-1");
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].display_name, "Code");
        assert!(api
            .put_pack(&auth, "shared-folder-1", "folder-revision-1", b"pack")
            .expect("pack writes"));
        assert_eq!(
            api.get_pack(&auth, "shared-folder-1", "folder-revision-1")
                .expect("pack reads"),
            b"pack"
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
            api.ensure_shared_folder(&bob_auth, "shared-folder-1", "Code"),
            Err(ApiError::Forbidden(_))
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
    }
}
