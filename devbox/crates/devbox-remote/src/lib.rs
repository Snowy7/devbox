//! Devbox-hosted Loom remote implementation.
//!
//! Loom owns the remote trait and folder-state sync semantics. Devbox owns the
//! hosted API that stores packs, cursors, sessions, devices, and shared-folder
//! membership.

use loom_core::{FolderRevisionId, SharedFolderId};
use loom_pack::LoomPack;
use loom_sync::{LoomRemote, SyncError, SyncResult};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use ureq::{Agent, Error as UreqError};
use url::Url;

pub const DEVBOX_HOSTED_REMOTE_KIND: &str = "devbox";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevboxHostedRemoteConfig {
    api: Url,
    shared_folder_id: SharedFolderId,
    session_token: String,
    device_id: String,
}

impl DevboxHostedRemoteConfig {
    pub fn new(
        api: impl AsRef<str>,
        shared_folder_id: SharedFolderId,
        session_token: impl Into<String>,
        device_id: impl Into<String>,
    ) -> SyncResult<Self> {
        let api = parse_http_url(api.as_ref())?;
        let session_token = non_empty_config_value(session_token.into(), "session token")?;
        let device_id = non_empty_config_value(device_id.into(), "device id")?;
        Ok(Self {
            api,
            shared_folder_id,
            session_token,
            device_id,
        })
    }

    pub fn from_clone_url(value: &str) -> SyncResult<Self> {
        let url = Url::parse(value).map_err(|error| {
            SyncError::RemoteConfig(format!("devbox remote URL is invalid: {error}"))
        })?;
        if url.scheme() != "devbox" {
            return Err(SyncError::RemoteConfig(
                "devbox remote URL must use the devbox scheme".to_string(),
            ));
        }
        let shared_folder_id = url
            .host_str()
            .ok_or_else(|| {
                SyncError::RemoteConfig(
                    "devbox remote URL is missing a shared folder id".to_string(),
                )
            })
            .and_then(|value| SharedFolderId::new(value.to_string()).map_err(SyncError::Loom))?;
        let query = url.query_pairs().collect::<BTreeMap<_, _>>();
        let api = query.get("api").ok_or_else(|| {
            SyncError::RemoteConfig("devbox remote URL is missing api".to_string())
        })?;
        let session = query.get("session").ok_or_else(|| {
            SyncError::RemoteConfig("devbox remote URL is missing session".to_string())
        })?;
        let device = query.get("device").ok_or_else(|| {
            SyncError::RemoteConfig("devbox remote URL is missing device".to_string())
        })?;
        Self::new(
            api.as_ref(),
            shared_folder_id,
            session.as_ref(),
            device.as_ref(),
        )
    }

    pub fn clone_url(&self) -> String {
        let mut url = Url::parse(&format!("devbox://{}", self.shared_folder_id))
            .expect("shared folder ids form a URL host");
        url.query_pairs_mut()
            .append_pair("api", self.api.as_str())
            .append_pair("session", &self.session_token)
            .append_pair("device", &self.device_id);
        url.to_string()
    }

    pub fn api(&self) -> &Url {
        &self.api
    }

    pub fn shared_folder_id(&self) -> &SharedFolderId {
        &self.shared_folder_id
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    fn url(&self, path: &str) -> String {
        let mut url = self.api.clone();
        url.set_path(path);
        url.set_query(None);
        url.to_string()
    }

    fn loom_path(&self, suffix: &str) -> String {
        format!(
            "/v1/loom/shared-folders/{}/{}",
            self.shared_folder_id.as_str(),
            suffix
        )
    }
}

#[derive(Clone)]
pub struct DevboxHostedRemote {
    config: DevboxHostedRemoteConfig,
    agent: Agent,
}

impl fmt::Debug for DevboxHostedRemote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevboxHostedRemote")
            .field("api", &self.config.api.to_string())
            .field("shared_folder_id", &self.config.shared_folder_id)
            .field("session_token", &"<redacted>")
            .field("device_id", &self.config.device_id)
            .finish()
    }
}

impl DevboxHostedRemote {
    pub fn new(config: DevboxHostedRemoteConfig) -> Self {
        Self {
            config,
            agent: Agent::new(),
        }
    }

    pub fn config(&self) -> &DevboxHostedRemoteConfig {
        &self.config
    }

    fn request(&self, method: &str, url: &str) -> ureq::Request {
        self.agent
            .request(method, url)
            .set(
                "authorization",
                &format!("Bearer {}", self.config.session_token),
            )
            .set("x-devbox-device-id", &self.config.device_id)
    }

    fn call(&self, method: &str, url: &str, body: Option<&[u8]>) -> SyncResult<HttpResponse> {
        let response = match (method, body) {
            ("PUT", Some(bytes)) => self
                .request(method, url)
                .set("content-type", "application/octet-stream")
                .send_bytes(bytes),
            ("PUT", None) => self.request(method, url).send_bytes(&[]),
            _ => self.request(method, url).call(),
        };
        match response {
            Ok(response) => read_http_response(response),
            Err(UreqError::Status(status, response)) => {
                let mut response = read_http_response(response)?;
                response.status = status;
                Ok(response)
            }
            Err(UreqError::Transport(error)) => Err(SyncError::RemoteTransport(format!(
                "devbox hosted remote transport failed: {error}"
            ))),
        }
    }
}

impl LoomRemote for DevboxHostedRemote {
    fn get_cursor(&self, cursor_id: &str) -> SyncResult<Option<FolderRevisionId>> {
        let url = self.config.url(&self.config.loom_path(&format!(
            "cursors/{}",
            validate_remote_segment(cursor_id, "cursor id")?
        )));
        let response = self.call("GET", &url, None)?;
        match response.status {
            200 => {
                let body: CursorResponse = serde_json::from_slice(&response.bytes)
                    .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
                body.revision_id
                    .map(FolderRevisionId::new)
                    .transpose()
                    .map_err(SyncError::Loom)
            }
            401 | 403 => Err(SyncError::RemoteAuth(format!(
                "devbox hosted cursor read failed with HTTP status {}",
                response.status
            ))),
            404 => Ok(None),
            status => Err(SyncError::RemoteTransport(format!(
                "devbox hosted cursor read failed with HTTP status {status}"
            ))),
        }
    }

    fn compare_and_set_cursor(
        &self,
        cursor_id: &str,
        expected: Option<&FolderRevisionId>,
        next: &FolderRevisionId,
    ) -> SyncResult<()> {
        let url = self.config.url(&self.config.loom_path(&format!(
            "cursors/{}",
            validate_remote_segment(cursor_id, "cursor id")?
        )));
        let body = serde_json::json!({
            "expected_revision_id": expected.map(FolderRevisionId::as_str),
            "next_revision_id": next.as_str(),
        });
        let body = serde_json::to_vec(&body)
            .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
        let response = self.call("PUT", &url, Some(&body))?;
        match response.status {
            200 => Ok(()),
            409 => {
                let error: ErrorResponse =
                    serde_json::from_slice(&response.bytes).unwrap_or(ErrorResponse {
                        actual_revision_id: None,
                    });
                let actual = error
                    .actual_revision_id
                    .map(FolderRevisionId::new)
                    .transpose()
                    .map_err(SyncError::Loom)?;
                Err(SyncError::CursorConflict {
                    cursor_id: cursor_id.to_string(),
                    expected: expected.cloned(),
                    actual,
                    attempted: next.clone(),
                })
            }
            401 | 403 => Err(SyncError::RemoteAuth(format!(
                "devbox hosted cursor update failed with HTTP status {}",
                response.status
            ))),
            status => Err(SyncError::RemoteTransport(format!(
                "devbox hosted cursor update failed with HTTP status {status}"
            ))),
        }
    }

    fn put_pack(&self, pack: &LoomPack) -> SyncResult<()> {
        let revision = validate_remote_segment(
            pack.manifest.latest_revision_id.as_str(),
            "folder revision id",
        )?;
        let url = self
            .config
            .url(&self.config.loom_path(&format!("packs/{revision}")));
        let response = self.call("PUT", &url, Some(&pack.encode()))?;
        match response.status {
            200 | 201 => Ok(()),
            401 | 403 => Err(SyncError::RemoteAuth(format!(
                "devbox hosted pack upload failed with HTTP status {}",
                response.status
            ))),
            status => Err(SyncError::RemoteTransport(format!(
                "devbox hosted pack upload failed with HTTP status {status}"
            ))),
        }
    }

    fn get_pack(&self, revision_id: &FolderRevisionId) -> SyncResult<LoomPack> {
        let revision = validate_remote_segment(revision_id.as_str(), "folder revision id")?;
        let url = self
            .config
            .url(&self.config.loom_path(&format!("packs/{revision}")));
        let response = self.call("GET", &url, None)?;
        match response.status {
            200 => LoomPack::decode(&response.bytes).map_err(SyncError::Pack),
            401 | 403 => Err(SyncError::RemoteAuth(format!(
                "devbox hosted pack download failed with HTTP status {}",
                response.status
            ))),
            404 => Err(SyncError::MissingRemotePack(revision_id.clone())),
            status => Err(SyncError::RemoteTransport(format!(
                "devbox hosted pack download failed with HTTP status {status}"
            ))),
        }
    }
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct CursorResponse {
    revision_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    actual_revision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevboxProvisionedRemote {
    pub config: DevboxHostedRemoteConfig,
    pub account_id: String,
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
struct DevSessionResponse {
    account_id: String,
    session_id: String,
    session_token: String,
    device_id: String,
}

pub fn provision_devbox_hosted_remote(
    api: impl AsRef<str>,
    shared_folder_id: &SharedFolderId,
    display_name: &str,
) -> SyncResult<DevboxProvisionedRemote> {
    let api = parse_http_url(api.as_ref())?;
    let agent = Agent::new();
    let login_url = api_url(&api, "/v1/auth/dev-session");
    let login_body = serde_json::json!({
        "account_hint": "local-dev",
        "device_id": "loom-cli-local",
        "device_display_name": "Loom CLI local device",
    });
    let login = agent
        .post(&login_url)
        .set("content-type", "application/json")
        .send_bytes(
            &serde_json::to_vec(&login_body)
                .map_err(|error| SyncError::RemoteTransport(error.to_string()))?,
        );
    let login = match login {
        Ok(response) => read_http_response(response)?,
        Err(UreqError::Status(status, response)) => {
            let mut response = read_http_response(response)?;
            response.status = status;
            response
        }
        Err(UreqError::Transport(error)) => {
            return Err(SyncError::RemoteTransport(format!(
                "devbox local session request failed: {error}"
            )))
        }
    };
    if login.status != 200 {
        return Err(SyncError::RemoteAuth(format!(
            "devbox local session request failed with HTTP status {}",
            login.status
        )));
    }
    let session: DevSessionResponse = serde_json::from_slice(&login.bytes)
        .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;

    let folder_url = api_url(
        &api,
        &format!("/v1/shared-folders/{}", shared_folder_id.as_str()),
    );
    let folder_body = serde_json::json!({ "display_name": display_name });
    let folder = agent
        .put(&folder_url)
        .set(
            "authorization",
            &format!("Bearer {}", session.session_token),
        )
        .set("x-devbox-device-id", &session.device_id)
        .set("content-type", "application/json")
        .send_bytes(
            &serde_json::to_vec(&folder_body)
                .map_err(|error| SyncError::RemoteTransport(error.to_string()))?,
        );
    let folder = match folder {
        Ok(response) => read_http_response(response)?,
        Err(UreqError::Status(status, response)) => {
            let mut response = read_http_response(response)?;
            response.status = status;
            response
        }
        Err(UreqError::Transport(error)) => {
            return Err(SyncError::RemoteTransport(format!(
                "devbox shared folder registration failed: {error}"
            )))
        }
    };
    if folder.status != 200 {
        return Err(SyncError::RemoteAuth(format!(
            "devbox shared folder registration failed with HTTP status {}",
            folder.status
        )));
    }

    Ok(DevboxProvisionedRemote {
        config: DevboxHostedRemoteConfig::new(
            api.as_str(),
            shared_folder_id.clone(),
            session.session_token,
            session.device_id,
        )?,
        account_id: session.account_id,
        session_id: session.session_id,
    })
}

fn parse_http_url(value: &str) -> SyncResult<Url> {
    let url = Url::parse(value)
        .map_err(|error| SyncError::RemoteConfig(format!("API URL is invalid: {error}")))?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(SyncError::RemoteConfig(
                "API URL must use http or https".to_string(),
            ))
        }
    }
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err(SyncError::RemoteConfig(
            "API URL must include a host and no userinfo".to_string(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(SyncError::RemoteConfig(
            "API URL must not include query or fragment".to_string(),
        ));
    }
    Ok(url)
}

fn api_url(api: &Url, path: &str) -> String {
    let mut url = api.clone();
    url.set_path(path);
    url.set_query(None);
    url.to_string()
}

fn non_empty_config_value(value: String, label: &'static str) -> SyncResult<String> {
    if value.trim().is_empty() {
        return Err(SyncError::RemoteConfig(format!("{label} cannot be empty")));
    }
    Ok(value)
}

fn validate_remote_segment(value: &str, label: &'static str) -> SyncResult<String> {
    if value.trim().is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value.contains("..")
        || value
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.'))
    {
        return Err(SyncError::RemoteConfig(format!(
            "{label} must be a safe remote path segment"
        )));
    }
    Ok(value.to_string())
}

fn read_http_response(response: ureq::Response) -> SyncResult<HttpResponse> {
    let status = response.status();
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
    Ok(HttpResponse { status, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::RevisionBoundary;
    use loom_store::LocalStore;
    use loom_sync::{sync_store_to_remote, DEFAULT_CURSOR_ID};
    use std::fs;

    #[test]
    fn devbox_hosted_remote_moves_pack_and_cursor_through_api() {
        let dir = tempfile::tempdir().expect("temp dir");
        let api =
            devbox_api::spawn_local_test_server(dir.path().join("api")).expect("api server starts");
        let folder = dir.path().join("source");
        fs::create_dir_all(&folder).expect("folder creates");
        let store = LocalStore::open_or_init(&folder)
            .expect("store initializes")
            .into_store();
        let object = store
            .object_cache()
            .write_bytes(b"hello\n")
            .expect("object");
        let version = loom_core::FileVersion::new(
            loom_core::FileVersionId::new("file-version-hosted").expect("file version id"),
            "README.md",
            loom_core::FileKind::File,
            Some(object.id().clone()),
            Some(object.size_bytes()),
            "unix:1",
        )
        .expect("file version");
        let revision = store
            .coalesce_folder_revision(RevisionBoundary::Sync, &[version])
            .expect("revision")
            .revision()
            .clone();
        let provisioned = provision_devbox_hosted_remote(
            api.base_url(),
            store.shared_folder().id(),
            store.shared_folder().display_name(),
        )
        .expect("remote provisions");
        let remote = DevboxHostedRemote::new(provisioned.config.clone());

        let report = sync_store_to_remote(&store, &remote).expect("sync succeeds");
        let pack = remote.get_pack(revision.id()).expect("pack reads");

        assert_eq!(report.latest_revision_id, *revision.id());
        assert_eq!(
            remote
                .get_cursor(DEFAULT_CURSOR_ID)
                .expect("cursor reads")
                .as_ref(),
            Some(revision.id())
        );
        assert_eq!(pack.manifest.latest_revision_id, *revision.id());
        assert!(provisioned.config.clone_url().starts_with("devbox://"));
        assert_eq!(
            DevboxHostedRemoteConfig::from_clone_url(&provisioned.config.clone_url())
                .expect("clone URL parses")
                .shared_folder_id(),
            store.shared_folder().id()
        );
    }

    #[test]
    fn devbox_hosted_remote_reports_cursor_conflict() {
        let dir = tempfile::tempdir().expect("temp dir");
        let api =
            devbox_api::spawn_local_test_server(dir.path().join("api")).expect("api server starts");
        let folder_id = SharedFolderId::new("shared-folder-hosted").expect("folder id");
        let provisioned =
            provision_devbox_hosted_remote(api.base_url(), &folder_id, "Hosted").expect("remote");
        let remote = DevboxHostedRemote::new(provisioned.config);
        let first = FolderRevisionId::new("folder-revision-first").expect("revision id");
        let second = FolderRevisionId::new("folder-revision-second").expect("revision id");
        remote
            .compare_and_set_cursor(DEFAULT_CURSOR_ID, None, &first)
            .expect("first cursor writes");

        let error = remote
            .compare_and_set_cursor(DEFAULT_CURSOR_ID, None, &second)
            .expect_err("stale cursor fails");

        assert!(matches!(
            error,
            SyncError::CursorConflict {
                expected,
                actual,
                attempted,
                ..
            } if expected.is_none() && actual == Some(first) && attempted == second
        ));
    }
}
