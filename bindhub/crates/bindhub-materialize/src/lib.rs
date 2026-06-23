//! Local/mock second-device snapshot publish, import, and materialization.

use bindhub_auth::DeviceProjectCursor;
use bindhub_conflict::{
    compare_snapshots, ComparableEntry, ComparableSnapshot, ConflictCompareError, PathComparisonRow,
};
use bindhub_core::{BlobId, DomainIdError, ManifestEntryKind, PolicyDecision};
use bindhub_metadata::{
    DeviceProjectCursorRecord, ErrorResponse, MetadataAuthMode, MetadataError,
    MetadataServiceConfig, MetadataStore, PublishSnapshotRequest as MetadataPublishSnapshotRequest,
    PublishedSnapshotRecord, UpdateCursorRequest, UpsertDeviceRequest, UpsertProjectRequest,
};
use bindhub_snapshot::{RestoreMaterializer, RestorePlan, RestorePlanError, RestorePlanSummary};
use bindhub_store::{
    path_to_store_string, BlobCache, BlobCacheError, ConflictWithRows, ManifestEntryRecord,
    NewConflict, NewConflictRow, NewProject, NewSnapshot, NewSnapshotDraft,
    NewSnapshotManifestEntry, PersistedSnapshot, Store, StoreError,
};
use bindhub_sync::{
    decrypt_payload, download_blob_to_cache, encrypt_payload, encrypted_blob_object_key,
    upload_blob_from_cache, ObjectKey, RemoteBlobProvider, SyncError, SyncKey,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

const BUNDLE_VERSION: u32 = 1;

#[derive(Debug)]
pub enum MaterializeError {
    Store(StoreError),
    BlobCache(BlobCacheError),
    Sync(SyncError),
    Restore(RestorePlanError),
    Json(serde_json::Error),
    DomainId(DomainIdError),
    ConflictCompare(ConflictCompareError),
    Metadata(MetadataError),
    PreflightBlocked(Box<SyncPreflightOutcome>),
    SnapshotNotFound(String),
    LocalIdentityMissing,
    InvalidBundle(String),
    RemoteObjectAlreadyExists(ObjectKey),
}

impl fmt::Display for MaterializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => write!(f, "{error}"),
            Self::BlobCache(error) => write!(f, "{error}"),
            Self::Sync(error) => write!(f, "{error}"),
            Self::Restore(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::DomainId(error) => write!(f, "{error}"),
            Self::ConflictCompare(error) => write!(f, "{error}"),
            Self::Metadata(error) => write!(f, "hosted metadata error: {error}"),
            Self::PreflightBlocked(outcome) => write!(
                f,
                "sync preflight blocked project {}: conflict {}",
                outcome.project_id,
                outcome.conflict_id().unwrap_or("missing-conflict-id")
            ),
            Self::SnapshotNotFound(id) => write!(f, "snapshot not found: {id}"),
            Self::LocalIdentityMissing => {
                f.write_str("local identity is not initialized; run bindhub init --db <DB_PATH>")
            }
            Self::InvalidBundle(message) => {
                write!(f, "invalid published snapshot bundle: {message}")
            }
            Self::RemoteObjectAlreadyExists(key) => {
                write!(
                    f,
                    "remote object already exists with different plaintext: {key}"
                )
            }
        }
    }
}

impl std::error::Error for MaterializeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::BlobCache(error) => Some(error),
            Self::Sync(error) => Some(error),
            Self::Restore(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::DomainId(error) => Some(error),
            Self::ConflictCompare(error) => Some(error),
            Self::Metadata(error) => Some(error),
            Self::SnapshotNotFound(_)
            | Self::PreflightBlocked(_)
            | Self::LocalIdentityMissing
            | Self::InvalidBundle(_)
            | Self::RemoteObjectAlreadyExists(_) => None,
        }
    }
}

impl From<StoreError> for MaterializeError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<BlobCacheError> for MaterializeError {
    fn from(error: BlobCacheError) -> Self {
        Self::BlobCache(error)
    }
}

impl From<SyncError> for MaterializeError {
    fn from(error: SyncError) -> Self {
        Self::Sync(error)
    }
}

impl From<RestorePlanError> for MaterializeError {
    fn from(error: RestorePlanError) -> Self {
        Self::Restore(error)
    }
}

impl From<serde_json::Error> for MaterializeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<DomainIdError> for MaterializeError {
    fn from(error: DomainIdError) -> Self {
        Self::DomainId(error)
    }
}

impl From<ConflictCompareError> for MaterializeError {
    fn from(error: ConflictCompareError) -> Self {
        Self::ConflictCompare(error)
    }
}

impl From<MetadataError> for MaterializeError {
    fn from(error: MetadataError) -> Self {
        Self::Metadata(error)
    }
}

pub type MaterializeResult<T> = Result<T, MaterializeError>;

pub trait HostedMetadataClient {
    fn upsert_device(&mut self, request: UpsertDeviceRequest) -> MaterializeResult<()>;
    fn upsert_project(&mut self, request: UpsertProjectRequest) -> MaterializeResult<()>;
    fn publish_snapshot(
        &mut self,
        request: MetadataPublishSnapshotRequest,
    ) -> MaterializeResult<PublishedSnapshotRecord>;
    fn snapshot(
        &mut self,
        account_id: &str,
        project_id: &str,
        snapshot_id: &str,
    ) -> MaterializeResult<Option<PublishedSnapshotRecord>>;
    fn latest_snapshot(
        &mut self,
        account_id: &str,
        project_id: &str,
    ) -> MaterializeResult<Option<PublishedSnapshotRecord>>;
    fn compare_and_set_cursor(
        &mut self,
        request: UpdateCursorRequest,
    ) -> MaterializeResult<DeviceProjectCursorRecord>;
}

impl<T: MetadataStore + ?Sized> HostedMetadataClient for T {
    fn upsert_device(&mut self, request: UpsertDeviceRequest) -> MaterializeResult<()> {
        MetadataStore::upsert_device(self, request)?;
        Ok(())
    }

    fn upsert_project(&mut self, request: UpsertProjectRequest) -> MaterializeResult<()> {
        MetadataStore::upsert_project(self, request)?;
        Ok(())
    }

    fn publish_snapshot(
        &mut self,
        request: MetadataPublishSnapshotRequest,
    ) -> MaterializeResult<PublishedSnapshotRecord> {
        Ok(MetadataStore::publish_snapshot(self, request)?)
    }

    fn snapshot(
        &mut self,
        account_id: &str,
        project_id: &str,
        snapshot_id: &str,
    ) -> MaterializeResult<Option<PublishedSnapshotRecord>> {
        Ok(MetadataStore::snapshot(
            self,
            account_id,
            project_id,
            snapshot_id,
        )?)
    }

    fn latest_snapshot(
        &mut self,
        account_id: &str,
        project_id: &str,
    ) -> MaterializeResult<Option<PublishedSnapshotRecord>> {
        Ok(MetadataStore::latest_snapshot(
            self, account_id, project_id,
        )?)
    }

    fn compare_and_set_cursor(
        &mut self,
        request: UpdateCursorRequest,
    ) -> MaterializeResult<DeviceProjectCursorRecord> {
        Ok(MetadataStore::compare_and_set_cursor(self, request)?)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostedMetadataApiConfig {
    endpoint: String,
    session_token_env: String,
    session_token: String,
}

impl std::fmt::Debug for HostedMetadataApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedMetadataApiConfig")
            .field("endpoint", &self.endpoint)
            .field("session_token_env", &self.session_token_env)
            .field("session_token", &"<redacted>")
            .finish()
    }
}

impl HostedMetadataApiConfig {
    pub fn new(
        endpoint: impl Into<String>,
        session_token_env: impl Into<String>,
        session_token: impl Into<String>,
    ) -> MaterializeResult<Self> {
        let endpoint = endpoint.into();
        let session_token_env = session_token_env.into();
        validate_env_name(&session_token_env, "--metadata-session-token-env")?;
        let session_token = session_token.into();
        if session_token.trim().is_empty() {
            return Err(metadata_invalid_request("metadata session token is empty"));
        }
        let checked = MetadataServiceConfig {
            endpoint,
            auth_mode: MetadataAuthMode::AccountSession,
        }
        .validate()?;
        Ok(Self {
            endpoint: checked.endpoint,
            session_token_env,
            session_token,
        })
    }

    pub fn from_env(
        endpoint: impl Into<String>,
        session_token_env: impl Into<String>,
    ) -> MaterializeResult<Self> {
        let session_token_env = session_token_env.into();
        let session_token = std::env::var(&session_token_env).map_err(|_| {
            metadata_invalid_request(format!(
                "metadata session token env var {session_token_env} is not set"
            ))
        })?;
        Self::new(endpoint, session_token_env, session_token)
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn session_token_env(&self) -> &str {
        &self.session_token_env
    }
}

#[derive(Clone)]
pub struct HostedMetadataApiClient {
    config: HostedMetadataApiConfig,
}

impl std::fmt::Debug for HostedMetadataApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedMetadataApiClient")
            .field("config", &self.config)
            .finish()
    }
}

impl HostedMetadataApiClient {
    pub fn new(config: HostedMetadataApiConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &HostedMetadataApiConfig {
        &self.config
    }

    fn url(&self, path: &str) -> MaterializeResult<String> {
        Ok(format!(
            "{}{}",
            self.config.endpoint.trim_end_matches('/'),
            path
        ))
    }

    fn bearer(&self, request: ureq::Request) -> ureq::Request {
        request.set(
            "authorization",
            &format!("Bearer {}", self.config.session_token),
        )
    }
}

impl HostedMetadataClient for HostedMetadataApiClient {
    fn upsert_device(&mut self, request: UpsertDeviceRequest) -> MaterializeResult<()> {
        let url = self.url("/v1/devices")?;
        let _: bindhub_metadata::DeviceRecord = self
            .bearer(ureq::put(&url))
            .send_json(serde_json::to_value(request)?)
            .map_err(metadata_http_error)?
            .into_json()
            .map_err(metadata_response_error)?;
        Ok(())
    }

    fn upsert_project(&mut self, request: UpsertProjectRequest) -> MaterializeResult<()> {
        let url = self.url("/v1/projects")?;
        let _: bindhub_metadata::ProjectRecord = self
            .bearer(ureq::put(&url))
            .send_json(serde_json::to_value(request)?)
            .map_err(metadata_http_error)?
            .into_json()
            .map_err(metadata_response_error)?;
        Ok(())
    }

    fn publish_snapshot(
        &mut self,
        request: MetadataPublishSnapshotRequest,
    ) -> MaterializeResult<PublishedSnapshotRecord> {
        let path = format!(
            "/v1/projects/{}/snapshots",
            api_path_segment(&request.project_id, "project id")?
        );
        let url = self.url(&path)?;
        self.bearer(ureq::put(&url))
            .send_json(serde_json::to_value(request)?)
            .map_err(metadata_http_error)?
            .into_json()
            .map_err(metadata_response_error)
    }

    fn snapshot(
        &mut self,
        _account_id: &str,
        project_id: &str,
        snapshot_id: &str,
    ) -> MaterializeResult<Option<PublishedSnapshotRecord>> {
        let path = format!(
            "/v1/projects/{}/snapshots/{}",
            api_path_segment(project_id, "project id")?,
            api_path_segment(snapshot_id, "snapshot id")?
        );
        let url = self.url(&path)?;
        match self.bearer(ureq::get(&url)).call() {
            Ok(response) => response
                .into_json()
                .map(Some)
                .map_err(metadata_response_error),
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(error) => Err(metadata_http_error(error)),
        }
    }

    fn latest_snapshot(
        &mut self,
        _account_id: &str,
        project_id: &str,
    ) -> MaterializeResult<Option<PublishedSnapshotRecord>> {
        let path = format!(
            "/v1/projects/{}/snapshots/latest",
            api_path_segment(project_id, "project id")?
        );
        let url = self.url(&path)?;
        match self.bearer(ureq::get(&url)).call() {
            Ok(response) => response
                .into_json()
                .map(Some)
                .map_err(metadata_response_error),
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(error) => Err(metadata_http_error(error)),
        }
    }

    fn compare_and_set_cursor(
        &mut self,
        request: UpdateCursorRequest,
    ) -> MaterializeResult<DeviceProjectCursorRecord> {
        let path = format!(
            "/v1/cursors/{}/{}",
            api_path_segment(&request.project_id, "project id")?,
            api_path_segment(&request.device_id, "device id")?
        );
        let url = self.url(&path)?;
        self.bearer(ureq::put(&url))
            .send_json(serde_json::to_value(request)?)
            .map_err(metadata_http_error)?
            .into_json()
            .map_err(metadata_response_error)
    }
}

fn metadata_invalid_request(message: impl Into<String>) -> MaterializeError {
    MaterializeError::Metadata(MetadataError::InvalidRequest(message.into()))
}

fn metadata_response_error(error: std::io::Error) -> MaterializeError {
    metadata_invalid_request(format!("hosted metadata response was invalid: {error}"))
}

fn metadata_http_error(error: ureq::Error) -> MaterializeError {
    match error {
        ureq::Error::Status(status, response) => {
            if status == 409 {
                return MaterializeError::Metadata(MetadataError::CursorPreconditionFailed {
                    current_cursor: None,
                });
            }
            let message = response
                .into_json::<ErrorResponse>()
                .ok()
                .map(|body| body.error)
                .unwrap_or_else(|| format!("hosted metadata request failed: http_status_{status}"));
            MaterializeError::Metadata(MetadataError::InvalidRequest(message))
        }
        ureq::Error::Transport(_) => {
            metadata_invalid_request("hosted metadata request failed: transport_error")
        }
    }
}

fn validate_env_name(name: &str, flag: &str) -> MaterializeResult<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(metadata_invalid_request(format!(
            "{flag} requires an environment variable name"
        )));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(metadata_invalid_request(format!(
            "{flag} requires an environment variable name"
        )));
    }
    Ok(())
}

fn api_path_segment(value: &str, label: &'static str) -> MaterializeResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed == "*"
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.'))
    {
        return Err(metadata_invalid_request(format!(
            "{label} must be a safe API path segment"
        )));
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishSnapshotRequest {
    pub db_path: PathBuf,
    pub cache_root: PathBuf,
    pub snapshot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSnapshotRequest {
    pub db_path: PathBuf,
    pub cache_root: PathBuf,
    pub key_source_db_path: Option<PathBuf>,
    pub snapshot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializationRequest {
    pub db_path: PathBuf,
    pub cache_root: PathBuf,
    pub key_source_db_path: Option<PathBuf>,
    pub snapshot_id: String,
    pub target: PathBuf,
    pub apply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedMetadataImportOptions {
    pub account_id: String,
    pub project_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPreflightRequest {
    pub db_path: PathBuf,
    pub project_id: String,
    pub base_snapshot_id: Option<String>,
    pub local_snapshot_id: Option<String>,
    pub incoming_snapshot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedSnapshotBundle {
    pub account_id: String,
    pub device_id: String,
    pub project_id: String,
    pub snapshot_id: String,
    pub manifest_object_key: ObjectKey,
    pub manifest_plaintext_bytes: u64,
    pub manifest_remote_bytes: u64,
    pub manifest_uploaded: bool,
    pub blob_count: usize,
    pub uploaded_blob_count: usize,
    pub plaintext_blob_bytes: u64,
    pub remote_blob_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSnapshotBundle {
    pub source_account_id: String,
    pub receiver_account_id: String,
    pub receiver_device_id: String,
    pub project_id: String,
    pub snapshot_id: String,
    pub manifest_object_key: ObjectKey,
    pub snapshot_inserted: bool,
    pub blob_count: usize,
    pub downloaded_blob_count: usize,
    pub plaintext_blob_bytes: u64,
    pub remote_blob_bytes: u64,
    pub cursor_value: String,
    pub cursor_updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializationOutcome {
    pub import: ImportedSnapshotBundle,
    pub target: PathBuf,
    pub target_status: String,
    pub apply: bool,
    pub apply_allowed: bool,
    pub plan: RestorePlanSummary,
    pub applied: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPreflightStatus {
    Ok,
    Blocked,
}

impl SyncPreflightStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPreflightOutcome {
    pub status: SyncPreflightStatus,
    pub project_id: String,
    pub base_snapshot_id: Option<String>,
    pub local_snapshot_id: Option<String>,
    pub incoming_snapshot_id: String,
    pub conflict: Option<ConflictWithRows>,
}

impl SyncPreflightOutcome {
    pub fn ok(
        project_id: String,
        base_snapshot_id: Option<String>,
        local_snapshot_id: Option<String>,
        incoming_snapshot_id: String,
    ) -> Self {
        Self {
            status: SyncPreflightStatus::Ok,
            project_id,
            base_snapshot_id,
            local_snapshot_id,
            incoming_snapshot_id,
            conflict: None,
        }
    }

    pub fn blocked(
        project_id: String,
        base_snapshot_id: Option<String>,
        local_snapshot_id: Option<String>,
        incoming_snapshot_id: String,
        conflict: ConflictWithRows,
    ) -> Self {
        Self {
            status: SyncPreflightStatus::Blocked,
            project_id,
            base_snapshot_id,
            local_snapshot_id,
            incoming_snapshot_id,
            conflict: Some(conflict),
        }
    }

    pub fn is_blocked(&self) -> bool {
        self.status == SyncPreflightStatus::Blocked
    }

    pub fn conflict_id(&self) -> Option<&str> {
        self.conflict
            .as_ref()
            .map(|conflict| conflict.conflict.id.as_str())
    }
}

pub fn published_snapshot_object_key(snapshot_id: &str) -> MaterializeResult<ObjectKey> {
    let digest = blake3::hash(snapshot_id.as_bytes()).to_hex().to_string();
    Ok(ObjectKey::new(format!(
        "encrypted/snapshots/b3/{}/{}/{digest}/bundle-v1",
        &digest[0..2],
        &digest[2..4]
    ))?)
}

pub fn publish_snapshot(
    request: &PublishSnapshotRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
) -> MaterializeResult<PublishedSnapshotBundle> {
    publish_snapshot_inner(request, provider, None)
}

pub fn publish_snapshot_with_metadata(
    request: &PublishSnapshotRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
    metadata: &mut impl HostedMetadataClient,
) -> MaterializeResult<PublishedSnapshotBundle> {
    publish_snapshot_inner(request, provider, Some(metadata))
}

fn publish_snapshot_inner(
    request: &PublishSnapshotRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
    metadata: Option<&mut dyn HostedMetadataClient>,
) -> MaterializeResult<PublishedSnapshotBundle> {
    let store = open_store(&request.db_path)?;
    let identity = store
        .completed_local_identity()?
        .ok_or(MaterializeError::LocalIdentityMissing)?;
    let sync_key = SyncKey::from_hex(&identity.sync_key_hex)?;
    let persisted = store
        .snapshot_with_entries(&request.snapshot_id)?
        .ok_or_else(|| MaterializeError::SnapshotNotFound(request.snapshot_id.clone()))?;
    let cache = BlobCache::open(&request.cache_root)?;
    let included_blobs = included_file_blobs(&persisted)?;

    let mut uploaded_blob_count = 0;
    let mut plaintext_blob_bytes = 0;
    let mut remote_blob_bytes = 0;
    for blob in included_blobs.values() {
        let uploaded =
            upload_blob_from_cache(&cache, provider, &sync_key, &blob.blob_id, &blob.object_key)?;
        if uploaded.uploaded {
            uploaded_blob_count += 1;
        }
        plaintext_blob_bytes += uploaded.plaintext_bytes;
        remote_blob_bytes += uploaded.remote_bytes;
    }

    let envelope = SnapshotBundleEnvelope::from_persisted(
        BUNDLE_VERSION,
        &identity.account_id,
        &identity.device_id,
        &persisted,
        &included_blobs,
    );
    let plaintext = serde_json::to_vec(&envelope)?;
    let manifest_object_key = published_snapshot_object_key(&request.snapshot_id)?;
    let manifest_put =
        put_encrypted_manifest(provider, &sync_key, &manifest_object_key, &plaintext)?;

    let published_metadata = if let Some(metadata) = metadata {
        let published_at = store.current_timestamp()?;
        Some(register_published_snapshot_metadata(
            metadata,
            &identity,
            &persisted,
            &manifest_object_key,
            &plaintext,
            &published_at,
        )?)
    } else {
        None
    };

    Ok(PublishedSnapshotBundle {
        account_id: published_metadata
            .as_ref()
            .map(|record| record.account_id.clone())
            .unwrap_or(identity.account_id),
        device_id: identity.device_id,
        project_id: persisted.project.id,
        snapshot_id: persisted.snapshot.id,
        manifest_object_key,
        manifest_plaintext_bytes: plaintext.len() as u64,
        manifest_remote_bytes: manifest_put.remote_bytes,
        manifest_uploaded: manifest_put.uploaded,
        blob_count: included_blobs.len(),
        uploaded_blob_count,
        plaintext_blob_bytes,
        remote_blob_bytes,
    })
}

pub fn sync_preflight(request: &SyncPreflightRequest) -> MaterializeResult<SyncPreflightOutcome> {
    let mut store = open_store(&request.db_path)?;
    sync_preflight_in_store(
        &mut store,
        &request.project_id,
        request.base_snapshot_id.as_deref(),
        request.local_snapshot_id.as_deref(),
        &request.incoming_snapshot_id,
    )
}

pub fn import_snapshot(
    request: &ImportSnapshotRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
) -> MaterializeResult<ImportedSnapshotBundle> {
    import_snapshot_inner(request, provider, None, None, None, true)
}

pub fn import_snapshot_with_metadata(
    request: &ImportSnapshotRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
    metadata: &mut impl HostedMetadataClient,
    options: &HostedMetadataImportOptions,
) -> MaterializeResult<ImportedSnapshotBundle> {
    import_snapshot_inner(
        request,
        provider,
        Some(metadata),
        Some(options.account_id.as_str()),
        Some(options.project_id.as_str()),
        true,
    )
}

fn import_snapshot_inner(
    request: &ImportSnapshotRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
    mut metadata: Option<&mut dyn HostedMetadataClient>,
    metadata_account_id: Option<&str>,
    metadata_project_id: Option<&str>,
    advance_cursor: bool,
) -> MaterializeResult<ImportedSnapshotBundle> {
    let mut receiver_store = open_store(&request.db_path)?;
    let receiver_identity = receiver_store
        .completed_local_identity()?
        .ok_or(MaterializeError::LocalIdentityMissing)?;
    let sync_key = sync_key_for_import(request, &receiver_identity.sync_key_hex)?;
    let manifest_object_key = if let Some(client) = metadata.as_mut() {
        resolve_manifest_object_key(
            Some(&mut **client),
            &receiver_store,
            &receiver_identity,
            metadata_account_id,
            metadata_project_id,
            &request.snapshot_id,
        )?
    } else {
        resolve_manifest_object_key(
            None,
            &receiver_store,
            &receiver_identity,
            metadata_account_id,
            metadata_project_id,
            &request.snapshot_id,
        )?
    };
    let encrypted = provider
        .get(&manifest_object_key)?
        .ok_or_else(|| SyncError::MissingRemoteObject(manifest_object_key.clone()))?;
    let plaintext = decrypt_payload(&sync_key, &manifest_object_key, &encrypted)?;
    let envelope: SnapshotBundleEnvelope = serde_json::from_slice(&plaintext)?;
    envelope.validate(&request.snapshot_id)?;
    if let Some(project_id) = metadata_project_id {
        if envelope.project.id != project_id {
            return Err(MaterializeError::InvalidBundle(format!(
                "hosted metadata snapshot belongs to project {}, not {project_id}",
                envelope.project.id
            )));
        }
    }

    let receiver_cursor = receiver_store.device_project_cursor(
        &receiver_identity.account_id,
        &receiver_identity.device_id,
        &envelope.project.id,
    )?;
    let base_snapshot_id = receiver_cursor
        .as_ref()
        .map(|cursor| cursor.cursor_value.clone());
    let local_snapshot_id = receiver_store
        .latest_snapshot_for_project_excluding(&envelope.project.id, &envelope.snapshot.id)?
        .map(|snapshot| snapshot.snapshot.id);
    let incoming_exists = receiver_store
        .snapshot_with_entries(&envelope.snapshot.id)?
        .is_some();
    if preflight_should_block(
        base_snapshot_id.as_deref(),
        local_snapshot_id.as_deref(),
        &envelope.snapshot.id,
    ) {
        if !incoming_exists {
            persist_envelope(&mut receiver_store, &envelope)?;
        }
        let preflight = sync_preflight_in_store(
            &mut receiver_store,
            &envelope.project.id,
            base_snapshot_id.as_deref(),
            local_snapshot_id.as_deref(),
            &envelope.snapshot.id,
        )?;
        if preflight.is_blocked() {
            return Err(MaterializeError::PreflightBlocked(Box::new(preflight)));
        }
    }

    let cache = BlobCache::open(&request.cache_root)?;
    let mut downloaded_blob_count = 0;
    let mut plaintext_blob_bytes = 0;
    let mut remote_blob_bytes = 0;
    for blob in &envelope.included_blobs {
        let blob_id = BlobId::from_blake3_hex(blob.blob_id.clone())?;
        let object_key = ObjectKey::new(blob.remote_object_key.clone())?;
        let before = cache.exists(&blob_id);
        let downloaded =
            download_blob_to_cache(&cache, provider, &sync_key, &blob_id, &object_key)?;
        if !before {
            downloaded_blob_count += 1;
        }
        plaintext_blob_bytes += downloaded.plaintext_bytes;
        remote_blob_bytes += downloaded.remote_bytes;
    }

    let snapshot_inserted = if receiver_store
        .snapshot_with_entries(&envelope.snapshot.id)?
        .is_some()
    {
        false
    } else {
        persist_envelope(&mut receiver_store, &envelope)?;
        true
    };

    let cursor_updated_at = if advance_cursor {
        if let Some(client) = metadata.as_mut() {
            advance_import_cursor(
                &receiver_store,
                &receiver_identity.account_id,
                &receiver_identity.device_id,
                &envelope.project.id,
                &envelope.snapshot.id,
                metadata_account_id,
                Some(&mut **client),
            )?
        } else {
            advance_import_cursor(
                &receiver_store,
                &receiver_identity.account_id,
                &receiver_identity.device_id,
                &envelope.project.id,
                &envelope.snapshot.id,
                None,
                None,
            )?
        }
    } else {
        "-".to_string()
    };

    Ok(ImportedSnapshotBundle {
        source_account_id: envelope.account_id,
        receiver_account_id: receiver_identity.account_id,
        receiver_device_id: receiver_identity.device_id,
        project_id: envelope.project.id,
        snapshot_id: envelope.snapshot.id.clone(),
        manifest_object_key,
        snapshot_inserted,
        blob_count: envelope.included_blobs.len(),
        downloaded_blob_count,
        plaintext_blob_bytes,
        remote_blob_bytes,
        cursor_value: envelope.snapshot.id,
        cursor_updated_at,
    })
}

pub fn materialize_snapshot(
    request: &MaterializationRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
) -> MaterializeResult<MaterializationOutcome> {
    materialize_snapshot_inner(request, provider, None, None, None)
}

pub fn materialize_snapshot_with_metadata(
    request: &MaterializationRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
    metadata: &mut impl HostedMetadataClient,
    options: &HostedMetadataImportOptions,
) -> MaterializeResult<MaterializationOutcome> {
    materialize_snapshot_inner(
        request,
        provider,
        Some(metadata),
        Some(options.account_id.as_str()),
        Some(options.project_id.as_str()),
    )
}

fn materialize_snapshot_inner(
    request: &MaterializationRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
    mut metadata: Option<&mut dyn HostedMetadataClient>,
    metadata_account_id: Option<&str>,
    metadata_project_id: Option<&str>,
) -> MaterializeResult<MaterializationOutcome> {
    let import_request = ImportSnapshotRequest {
        db_path: request.db_path.clone(),
        cache_root: request.cache_root.clone(),
        key_source_db_path: request.key_source_db_path.clone(),
        snapshot_id: request.snapshot_id.clone(),
    };
    let mut import = if let Some(client) = metadata.as_mut() {
        import_snapshot_inner(
            &import_request,
            provider,
            Some(&mut **client),
            metadata_account_id,
            metadata_project_id,
            false,
        )?
    } else {
        import_snapshot_inner(
            &import_request,
            provider,
            None,
            metadata_account_id,
            metadata_project_id,
            false,
        )?
    };

    let store = open_store(&request.db_path)?;
    let persisted = store
        .snapshot_with_entries(&request.snapshot_id)?
        .ok_or_else(|| MaterializeError::SnapshotNotFound(request.snapshot_id.clone()))?;
    let cache = BlobCache::open(&request.cache_root)?;
    let plan = RestorePlan::from_persisted_snapshot(&persisted, &cache, &request.target)?;
    let target_status = plan.target_status().as_str().to_string();
    let apply_allowed = plan.apply_allowed();
    let summary = plan.summary();
    let mut applied = false;

    if request.apply {
        RestoreMaterializer::new(cache).apply(&plan)?;
        applied = true;
    }
    import.cursor_updated_at = if let Some(client) = metadata.as_mut() {
        advance_import_cursor(
            &store,
            &import.receiver_account_id,
            &import.receiver_device_id,
            &import.project_id,
            &import.snapshot_id,
            metadata_account_id,
            Some(&mut **client),
        )?
    } else {
        advance_import_cursor(
            &store,
            &import.receiver_account_id,
            &import.receiver_device_id,
            &import.project_id,
            &import.snapshot_id,
            None,
            None,
        )?
    };

    Ok(MaterializationOutcome {
        import,
        target: request.target.clone(),
        target_status,
        apply: request.apply,
        apply_allowed,
        plan: summary,
        applied,
    })
}

fn open_store(path: &Path) -> MaterializeResult<Store> {
    let store = Store::open_file(path)?;
    store.apply_migrations()?;
    Ok(store)
}

fn sync_key_for_import(
    request: &ImportSnapshotRequest,
    receiver_key_hex: &str,
) -> MaterializeResult<SyncKey> {
    let key_hex = if let Some(path) = &request.key_source_db_path {
        let key_store = open_store(path)?;
        key_store
            .completed_local_identity()?
            .ok_or(MaterializeError::LocalIdentityMissing)?
            .sync_key_hex
    } else {
        receiver_key_hex.to_string()
    };

    Ok(SyncKey::from_hex(&key_hex)?)
}

fn register_published_snapshot_metadata(
    metadata: &mut dyn HostedMetadataClient,
    identity: &bindhub_store::LocalIdentityRecord,
    persisted: &PersistedSnapshot,
    manifest_object_key: &ObjectKey,
    manifest_plaintext: &[u8],
    published_at: &str,
) -> MaterializeResult<PublishedSnapshotRecord> {
    metadata.upsert_device(UpsertDeviceRequest {
        account_id: identity.account_id.clone(),
        device_id: identity.device_id.clone(),
        display_name: identity.device_name.clone(),
        last_seen_at: published_at.to_string(),
    })?;
    metadata.upsert_project(UpsertProjectRequest {
        account_id: identity.account_id.clone(),
        project_id: persisted.project.id.clone(),
        display_name: persisted.project.display_name.clone(),
        root_hint: persisted.project.root_path.clone(),
        project_kind: persisted.project.kind.clone(),
        updated_at: published_at.to_string(),
    })?;
    metadata.publish_snapshot(MetadataPublishSnapshotRequest {
        account_id: identity.account_id.clone(),
        project_id: persisted.project.id.clone(),
        snapshot_id: persisted.snapshot.id.clone(),
        parent_snapshot_id: persisted.snapshot.parent_snapshot_id.clone(),
        manifest_object_key: manifest_object_key.to_string(),
        manifest_hash: format!("blake3:{}", blake3::hash(manifest_plaintext).to_hex()),
        manifest_entry_count: persisted.snapshot.manifest_entry_count,
        total_size_bytes: persisted.snapshot.total_size_bytes,
        published_by_device_id: identity.device_id.clone(),
        published_at: published_at.to_string(),
    })
}

fn resolve_manifest_object_key(
    metadata: Option<&mut dyn HostedMetadataClient>,
    store: &Store,
    identity: &bindhub_store::LocalIdentityRecord,
    metadata_account_id: Option<&str>,
    metadata_project_id: Option<&str>,
    snapshot_id: &str,
) -> MaterializeResult<ObjectKey> {
    let Some(metadata) = metadata else {
        return published_snapshot_object_key(snapshot_id);
    };
    let project_id = metadata_project_id.ok_or_else(|| {
        MaterializeError::InvalidBundle(
            "hosted metadata import requires a metadata project id".to_string(),
        )
    })?;
    let account_id = metadata_account_id.ok_or_else(|| {
        MaterializeError::InvalidBundle(
            "hosted metadata import requires a metadata account id".to_string(),
        )
    })?;
    let now = store.current_timestamp()?;
    metadata.upsert_device(UpsertDeviceRequest {
        account_id: account_id.to_string(),
        device_id: identity.device_id.clone(),
        display_name: identity.device_name.clone(),
        last_seen_at: now,
    })?;
    let snapshot = metadata
        .snapshot(account_id, project_id, snapshot_id)?
        .ok_or_else(|| MetadataError::NotFound {
            entity: "snapshot",
            id: snapshot_id.to_string(),
        })?;

    Ok(ObjectKey::new(snapshot.manifest_object_key)?)
}

fn advance_import_cursor(
    store: &Store,
    account_id: &str,
    device_id: &str,
    project_id: &str,
    snapshot_id: &str,
    metadata_account_id: Option<&str>,
    metadata: Option<&mut dyn HostedMetadataClient>,
) -> MaterializeResult<String> {
    let updated_at = store.current_timestamp()?;
    let expected_cursor = store
        .device_project_cursor(account_id, device_id, project_id)?
        .map(|cursor| cursor.cursor_value);
    if let Some(metadata) = metadata {
        let hosted_account_id = metadata_account_id.ok_or_else(|| {
            MaterializeError::InvalidBundle(
                "hosted metadata cursor update requires a metadata account id".to_string(),
            )
        })?;
        metadata.compare_and_set_cursor(UpdateCursorRequest {
            account_id: hosted_account_id.to_string(),
            device_id: device_id.to_string(),
            project_id: project_id.to_string(),
            expected_cursor,
            next_cursor: Some(snapshot_id.to_string()),
            updated_at: updated_at.clone(),
        })?;
    }

    let cursor = DeviceProjectCursor {
        account_id: account_id.to_string(),
        device_id: device_id.to_string(),
        project_id: project_id.to_string(),
        cursor_value: snapshot_id.to_string(),
        updated_at: updated_at.clone(),
    };
    store.upsert_device_project_cursor(&cursor)?;

    Ok(updated_at)
}

fn sync_preflight_in_store(
    store: &mut Store,
    project_id: &str,
    base_snapshot_id: Option<&str>,
    local_snapshot_id: Option<&str>,
    incoming_snapshot_id: &str,
) -> MaterializeResult<SyncPreflightOutcome> {
    let incoming = store
        .snapshot_with_entries(incoming_snapshot_id)?
        .ok_or_else(|| MaterializeError::SnapshotNotFound(incoming_snapshot_id.to_string()))?;
    if incoming.project.id != project_id {
        return Err(MaterializeError::InvalidBundle(format!(
            "incoming snapshot {} belongs to project {}, not {project_id}",
            incoming.snapshot.id, incoming.project.id
        )));
    }

    let local = local_snapshot_id
        .map(|id| {
            store
                .snapshot_with_entries(id)?
                .ok_or_else(|| MaterializeError::SnapshotNotFound(id.to_string()))
        })
        .transpose()?;
    if let Some(local) = &local {
        if local.project.id != project_id {
            return Err(MaterializeError::InvalidBundle(format!(
                "local snapshot {} belongs to project {}, not {project_id}",
                local.snapshot.id, local.project.id
            )));
        }
    }

    if !preflight_should_block(base_snapshot_id, local_snapshot_id, incoming_snapshot_id) {
        return Ok(SyncPreflightOutcome::ok(
            project_id.to_string(),
            base_snapshot_id.map(str::to_string),
            local_snapshot_id.map(str::to_string),
            incoming_snapshot_id.to_string(),
        ));
    }

    let base = base_snapshot_id
        .map(|id| {
            store
                .snapshot_with_entries(id)?
                .ok_or_else(|| MaterializeError::SnapshotNotFound(id.to_string()))
        })
        .transpose()?;
    if let Some(base) = &base {
        if base.project.id != project_id {
            return Err(MaterializeError::InvalidBundle(format!(
                "base snapshot {} belongs to project {}, not {project_id}",
                base.snapshot.id, base.project.id
            )));
        }
    }

    let local = local.expect("local_snapshot_id was present above");
    let base_comparable = base.as_ref().map(snapshot_to_comparable);
    let local_comparable = snapshot_to_comparable(&local);
    let incoming_comparable = snapshot_to_comparable(&incoming);
    let comparison = compare_snapshots(
        base_comparable.as_ref(),
        &local_comparable,
        &incoming_comparable,
    )?;
    let created_at = store.current_timestamp()?;
    let rows = comparison
        .rows()
        .iter()
        .map(new_conflict_row)
        .collect::<Vec<_>>();
    let conflict = store.persist_conflict(
        &NewConflict {
            id: comparison.conflict_id(),
            project_id: comparison.project_id(),
            base_snapshot_id: comparison.base_snapshot_id(),
            local_snapshot_id: comparison.local_snapshot_id(),
            incoming_snapshot_id: comparison.incoming_snapshot_id(),
            summary: comparison.summary(),
            created_at: &created_at,
        },
        &rows,
    )?;

    Ok(SyncPreflightOutcome::blocked(
        project_id.to_string(),
        base_snapshot_id.map(str::to_string),
        local_snapshot_id.map(str::to_string),
        incoming_snapshot_id.to_string(),
        conflict,
    ))
}

fn preflight_should_block(
    base_snapshot_id: Option<&str>,
    local_snapshot_id: Option<&str>,
    incoming_snapshot_id: &str,
) -> bool {
    !(local_snapshot_id.is_none()
        || local_snapshot_id == Some(incoming_snapshot_id)
        || base_snapshot_id == local_snapshot_id
        || base_snapshot_id == Some(incoming_snapshot_id))
}

fn snapshot_to_comparable(snapshot: &PersistedSnapshot) -> ComparableSnapshot {
    ComparableSnapshot::new(
        snapshot.project.id.clone(),
        snapshot.snapshot.id.clone(),
        snapshot
            .entries
            .iter()
            .map(|entry| {
                ComparableEntry::new(
                    entry.relative_path.clone(),
                    entry.kind.clone(),
                    entry.size_bytes,
                    entry.blob_id.clone(),
                    entry.object_ref.clone(),
                    entry.policy_decision.clone(),
                )
            })
            .collect(),
    )
}

fn new_conflict_row(row: &PathComparisonRow) -> NewConflictRow<'_> {
    NewConflictRow {
        path: row.path(),
        state: row.state(),
        entry_kind: row.entry_kind(),
        base_blob_id: row.base_blob_id(),
        local_blob_id: row.local_blob_id(),
        incoming_blob_id: row.incoming_blob_id(),
        base_size_bytes: row.base_size_bytes(),
        local_size_bytes: row.local_size_bytes(),
        incoming_size_bytes: row.incoming_size_bytes(),
        local_policy_decision: row.local_policy_decision(),
        incoming_policy_decision: row.incoming_policy_decision(),
    }
}

#[derive(Debug, Clone)]
struct IncludedBlob {
    blob_id: BlobId,
    object_key: ObjectKey,
    size_bytes: u64,
}

fn included_file_blobs(
    persisted: &PersistedSnapshot,
) -> MaterializeResult<BTreeMap<String, IncludedBlob>> {
    let mut blobs = BTreeMap::new();
    for entry in &persisted.entries {
        if entry.kind != ManifestEntryKind::File || entry.policy_decision != PolicyDecision::Include
        {
            continue;
        }

        let blob_id = entry.blob_id.clone().ok_or_else(|| {
            MaterializeError::InvalidBundle(format!(
                "included file {} is missing a blob id",
                path_to_store_string(&entry.relative_path)
            ))
        })?;
        blobs
            .entry(blob_id.to_string())
            .or_insert_with(|| IncludedBlob {
                object_key: encrypted_blob_object_key(&blob_id),
                blob_id,
                size_bytes: entry.size_bytes,
            });
    }

    Ok(blobs)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestPut {
    uploaded: bool,
    remote_bytes: u64,
}

fn put_encrypted_manifest(
    provider: &(impl RemoteBlobProvider + ?Sized),
    sync_key: &SyncKey,
    object_key: &ObjectKey,
    plaintext: &[u8],
) -> MaterializeResult<ManifestPut> {
    if let Some(existing) = provider.get(object_key)? {
        let existing_plaintext = decrypt_payload(sync_key, object_key, &existing)?;
        if existing_plaintext == plaintext {
            return Ok(ManifestPut {
                uploaded: false,
                remote_bytes: existing.len() as u64,
            });
        }

        return Err(MaterializeError::RemoteObjectAlreadyExists(
            object_key.clone(),
        ));
    }

    let encrypted = encrypt_payload(sync_key, object_key, plaintext)?;
    let put = match provider.put(object_key, &encrypted) {
        Ok(put) => put,
        Err(SyncError::RemoteObjectAlreadyExists { key }) if key == *object_key => {
            if let Some(existing) = provider.get(object_key)? {
                let existing_plaintext = decrypt_payload(sync_key, object_key, &existing)?;
                if existing_plaintext == plaintext {
                    return Ok(ManifestPut {
                        uploaded: false,
                        remote_bytes: existing.len() as u64,
                    });
                }
            }

            return Err(MaterializeError::RemoteObjectAlreadyExists(
                object_key.clone(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    Ok(ManifestPut {
        uploaded: put.uploaded,
        remote_bytes: put.size_bytes,
    })
}

fn persist_envelope(
    store: &mut Store,
    envelope: &SnapshotBundleEnvelope,
) -> MaterializeResult<PersistedSnapshot> {
    let entries = envelope
        .entries
        .iter()
        .map(WireManifestEntry::to_record)
        .collect::<MaterializeResult<Vec<_>>>()?;
    let draft_entries = entries
        .iter()
        .map(|entry| NewSnapshotManifestEntry {
            relative_path: &entry.relative_path,
            kind: entry.kind.clone(),
            size_bytes: entry.size_bytes,
            blob_id: entry.blob_id.as_ref(),
            object_ref: entry.object_ref.as_deref(),
            policy_decision: &entry.policy_decision,
        })
        .collect::<Vec<_>>();
    let draft = NewSnapshotDraft {
        project: NewProject {
            id: &envelope.project.id,
            root_path: &envelope.project.root_path,
            kind: &envelope.project.kind,
            display_name: &envelope.project.display_name,
            discovered_at: &envelope.project.discovered_at,
        },
        snapshot: NewSnapshot {
            id: &envelope.snapshot.id,
            project_id: &envelope.snapshot.project_id,
            parent_snapshot_id: envelope.snapshot.parent_snapshot_id.as_deref(),
            created_at: &envelope.snapshot.created_at,
            reason: &envelope.snapshot.reason,
            manifest_entry_count: envelope.snapshot.manifest_entry_count,
            total_size_bytes: envelope.snapshot.total_size_bytes,
        },
        entries: draft_entries,
    };

    Ok(store.persist_draft_snapshot(&draft)?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SnapshotBundleEnvelope {
    version: u32,
    account_id: String,
    device_id: String,
    project: WireProject,
    snapshot: WireSnapshot,
    entries: Vec<WireManifestEntry>,
    included_blobs: Vec<WireIncludedBlob>,
}

impl SnapshotBundleEnvelope {
    fn from_persisted(
        version: u32,
        account_id: &str,
        device_id: &str,
        persisted: &PersistedSnapshot,
        included_blobs: &BTreeMap<String, IncludedBlob>,
    ) -> Self {
        Self {
            version,
            account_id: account_id.to_string(),
            device_id: device_id.to_string(),
            project: WireProject::from_record(&persisted.project),
            snapshot: WireSnapshot::from_record(&persisted.snapshot),
            entries: persisted
                .entries
                .iter()
                .map(WireManifestEntry::from_record)
                .collect(),
            included_blobs: included_blobs
                .values()
                .map(WireIncludedBlob::from_included)
                .collect(),
        }
    }

    fn validate(&self, requested_snapshot_id: &str) -> MaterializeResult<()> {
        if self.version != BUNDLE_VERSION {
            return Err(MaterializeError::InvalidBundle(format!(
                "unsupported bundle version {}; expected {BUNDLE_VERSION}",
                self.version
            )));
        }
        if self.snapshot.id != requested_snapshot_id {
            return Err(MaterializeError::InvalidBundle(format!(
                "snapshot id mismatch: requested {requested_snapshot_id}, bundle contains {}",
                self.snapshot.id
            )));
        }
        if self.snapshot.project_id != self.project.id {
            return Err(MaterializeError::InvalidBundle(format!(
                "snapshot project {} does not match project {}",
                self.snapshot.project_id, self.project.id
            )));
        }
        if self.snapshot.manifest_entry_count != self.entries.len() as u64 {
            return Err(MaterializeError::InvalidBundle(format!(
                "manifest count mismatch: snapshot says {}, bundle has {}",
                self.snapshot.manifest_entry_count,
                self.entries.len()
            )));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WireProject {
    id: String,
    root_path: String,
    kind: String,
    display_name: String,
    discovered_at: String,
}

impl WireProject {
    fn from_record(record: &bindhub_store::ProjectRecord) -> Self {
        Self {
            id: record.id.clone(),
            root_path: record.root_path.clone(),
            kind: record.kind.clone(),
            display_name: record.display_name.clone(),
            discovered_at: record.discovered_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WireSnapshot {
    id: String,
    project_id: String,
    parent_snapshot_id: Option<String>,
    created_at: String,
    reason: String,
    manifest_entry_count: u64,
    total_size_bytes: u64,
}

impl WireSnapshot {
    fn from_record(record: &bindhub_store::SnapshotRecord) -> Self {
        Self {
            id: record.id.clone(),
            project_id: record.project_id.clone(),
            parent_snapshot_id: record.parent_snapshot_id.clone(),
            created_at: record.created_at.clone(),
            reason: record.reason.clone(),
            manifest_entry_count: record.manifest_entry_count,
            total_size_bytes: record.total_size_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WireManifestEntry {
    path: String,
    kind: String,
    size_bytes: u64,
    blob_id: Option<String>,
    object_ref: Option<String>,
    policy_decision: String,
    policy_reason: Option<String>,
}

impl WireManifestEntry {
    fn from_record(record: &ManifestEntryRecord) -> Self {
        let (policy_decision, policy_reason) = match &record.policy_decision {
            PolicyDecision::Include => ("include".to_string(), None),
            PolicyDecision::Exclude { reason } => ("exclude".to_string(), Some(reason.clone())),
            PolicyDecision::RequiresUserDecision { reason } => {
                ("requires_user_decision".to_string(), Some(reason.clone()))
            }
        };

        Self {
            path: path_to_store_string(&record.relative_path),
            kind: kind_to_wire(&record.kind).to_string(),
            size_bytes: record.size_bytes,
            blob_id: record.blob_id.as_ref().map(ToString::to_string),
            object_ref: record.object_ref.clone(),
            policy_decision,
            policy_reason,
        }
    }

    fn to_record(&self) -> MaterializeResult<ManifestEntryRecord> {
        Ok(ManifestEntryRecord {
            relative_path: PathBuf::from(&self.path),
            kind: kind_from_wire(&self.kind)?,
            size_bytes: self.size_bytes,
            blob_id: self
                .blob_id
                .clone()
                .map(BlobId::from_blake3_hex)
                .transpose()?,
            object_ref: self.object_ref.clone(),
            policy_decision: policy_from_wire(&self.policy_decision, self.policy_reason.clone())?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WireIncludedBlob {
    blob_id: String,
    remote_object_key: String,
    size_bytes: u64,
}

impl WireIncludedBlob {
    fn from_included(included: &IncludedBlob) -> Self {
        Self {
            blob_id: included.blob_id.to_string(),
            remote_object_key: included.object_key.to_string(),
            size_bytes: included.size_bytes,
        }
    }
}

fn kind_to_wire(kind: &ManifestEntryKind) -> &'static str {
    match kind {
        ManifestEntryKind::File => "file",
        ManifestEntryKind::Directory => "directory",
        ManifestEntryKind::Symlink => "symlink",
        ManifestEntryKind::Unsupported => "unsupported",
    }
}

fn kind_from_wire(value: &str) -> MaterializeResult<ManifestEntryKind> {
    match value {
        "file" => Ok(ManifestEntryKind::File),
        "directory" => Ok(ManifestEntryKind::Directory),
        "symlink" => Ok(ManifestEntryKind::Symlink),
        "unsupported" => Ok(ManifestEntryKind::Unsupported),
        _ => Err(MaterializeError::InvalidBundle(format!(
            "unknown manifest entry kind {value}"
        ))),
    }
}

fn policy_from_wire(decision: &str, reason: Option<String>) -> MaterializeResult<PolicyDecision> {
    match decision {
        "include" => Ok(PolicyDecision::Include),
        "exclude" => Ok(PolicyDecision::Exclude {
            reason: reason.unwrap_or_default(),
        }),
        "requires_user_decision" => Ok(PolicyDecision::RequiresUserDecision {
            reason: reason.unwrap_or_default(),
        }),
        _ => Err(MaterializeError::InvalidBundle(format!(
            "unknown policy decision {decision}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bindhub_metadata::{app_with_config, HostedApiConfig, InMemoryMetadataStore, MetadataStore};
    use bindhub_snapshot::{RestoreTargetStatus, SnapshotManifestBuilder};
    use bindhub_store::{
        local_project_id, BlobCache, EnsureLocalIdentityOptions, NewProject, NewSnapshot,
        NewSnapshotDraft, NewSnapshotManifestEntry,
    };
    use bindhub_sync::{LocalFilesystemBlobProvider, ObjectMetadata, PutOutcome, SyncResult};
    use std::cell::Cell;
    use std::fs;

    #[test]
    fn put_encrypted_manifest_treats_plaintext_match_after_put_race_as_idempotent() {
        let sync_key = SyncKey::from_bytes([31; 32]);
        let object_key =
            ObjectKey::new("snapshots/project/snapshot/manifest.json").expect("object key parses");
        let plaintext = br#"{"version":1,"entries":[]}"#;
        let existing_encrypted =
            encrypt_payload(&sync_key, &object_key, plaintext).expect("encryption works");
        let provider = RacingManifestProvider::new(object_key.clone(), existing_encrypted);

        let put = put_encrypted_manifest(&provider, &sync_key, &object_key, plaintext)
            .expect("same plaintext manifest race is idempotent");

        assert!(!put.uploaded);
        assert_eq!(provider.put_calls.get(), 1);
        assert_eq!(provider.get_calls.get(), 2);
    }

    #[test]
    fn hosted_metadata_api_client_publishes_discovers_latest_and_cas_cursor() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "hosted api metadata\n");
        let snapshot_id = fixture.persist_source_snapshot();
        let identity = local_identity_for_fixture(&fixture.source_db);
        let raw_token = "raw-hosted-metadata-session-token";
        let server = start_hosted_metadata_server(&identity.account_id, raw_token);
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        let config =
            HostedMetadataApiConfig::new(&server.url, "BINDHUB_TEST_METADATA_TOKEN", raw_token)
                .expect("hosted metadata config validates");
        let formatted = format!("{config:?}");
        assert!(!formatted.contains(raw_token));
        assert!(formatted.contains("<redacted>"));
        let mut client = HostedMetadataApiClient::new(config);

        let published = publish_snapshot_with_metadata(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
            &mut client,
        )
        .expect("hosted metadata publish succeeds");
        assert_eq!(published.account_id, identity.account_id);

        let latest = HostedMetadataClient::latest_snapshot(
            &mut client,
            "caller-supplied-account-is-ignored-by-http-client",
            &published.project_id,
        )
        .expect("latest lookup succeeds")
        .expect("latest snapshot exists");
        assert_eq!(latest.account_id, identity.account_id);
        assert_eq!(latest.snapshot_id, snapshot_id);
        assert_eq!(
            latest.manifest_object_key,
            published.manifest_object_key.to_string()
        );

        let cursor = client
            .compare_and_set_cursor(UpdateCursorRequest {
                account_id: "forged-account".to_string(),
                device_id: identity.device_id.clone(),
                project_id: published.project_id.clone(),
                expected_cursor: None,
                next_cursor: Some(snapshot_id.clone()),
                updated_at: "2026-06-19T10:00:00Z".to_string(),
            })
            .expect("cursor CAS succeeds");
        assert_eq!(cursor.account_id, identity.account_id);
        assert_eq!(cursor.cursor_value.as_deref(), Some(snapshot_id.as_str()));

        let stale = client
            .compare_and_set_cursor(UpdateCursorRequest {
                account_id: "forged-account".to_string(),
                device_id: identity.device_id.clone(),
                project_id: published.project_id.clone(),
                expected_cursor: None,
                next_cursor: Some("snapshot-stale".to_string()),
                updated_at: "2026-06-19T10:01:00Z".to_string(),
            })
            .expect_err("stale cursor is rejected");
        assert!(matches!(
            stale,
            MaterializeError::Metadata(MetadataError::CursorPreconditionFailed { .. })
        ));
    }

    #[test]
    fn publish_import_and_materialize_round_trip_through_local_remote() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "hello from device one\n");
        fixture.write("src/main.rs", "fn main() {}\n");
        fixture.write("node_modules/left-pad/index.js", "ignored\n");
        fixture.write(".git/config", "[core]\nrepositoryformatversion = 0\n");
        let snapshot_id = fixture.persist_source_snapshot();
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");

        let published = publish_snapshot(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
        )
        .expect("snapshot publishes");
        assert_eq!(published.blob_count, 2);
        assert_eq!(published.uploaded_blob_count, 2);

        let remote_manifest =
            fs::read(provider.path_for(&published.manifest_object_key)).expect("manifest reads");
        assert!(!remote_manifest
            .windows(b"src/main.rs".len())
            .any(|window| window == b"src/main.rs"));
        assert!(!remote_manifest
            .windows(b"hello from device one".len())
            .any(|window| window == b"hello from device one"));

        let outcome = materialize_snapshot(
            &MaterializationRequest {
                db_path: fixture.receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: Some(fixture.source_db.clone()),
                snapshot_id: snapshot_id.clone(),
                target: fixture.target.clone(),
                apply: true,
            },
            &provider,
        )
        .expect("snapshot materializes");

        assert!(outcome.applied);
        assert_eq!(outcome.plan.files_to_write, 2);
        assert_eq!(
            fs::read_to_string(fixture.target.join("README.md")).expect("readme restored"),
            "hello from device one\n"
        );
        assert_eq!(
            fs::read_to_string(fixture.target.join("src/main.rs")).expect("main restored"),
            "fn main() {}\n"
        );
        assert!(!fixture.target.join("node_modules").exists());
        assert!(!fixture.target.join(".git").exists());

        let receiver = Store::open_file(&fixture.receiver_db).expect("receiver opens");
        receiver.apply_migrations().expect("migrations apply");
        let receiver_identity = receiver
            .local_identity()
            .expect("identity reads")
            .expect("identity exists");
        let cursor = receiver
            .device_project_cursor(
                &receiver_identity.account_id,
                &receiver_identity.device_id,
                &outcome.import.project_id,
            )
            .expect("cursor reads")
            .expect("cursor exists");
        assert_eq!(cursor.cursor_value, snapshot_id);
    }

    #[test]
    fn paired_receiver_materializes_without_source_key_database() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "hello from paired source\n");
        let snapshot_id = fixture.persist_source_snapshot();
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");

        publish_snapshot(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
        )
        .expect("snapshot publishes");

        let source_identity = local_identity_for_fixture(&fixture.source_db);
        let source_view = local_identity_view_for_fixture(&source_identity);
        let invitation =
            bindhub_auth::create_pairing_invitation(&source_view, "2026-06-18T10:00:00Z", 100, 600)
                .expect("pairing invitation creates");
        let mut source_store = Store::open_file(&fixture.source_db).expect("source opens");
        source_store
            .apply_migrations()
            .expect("source migrations apply");
        source_store
            .insert_pairing_invitation(&invitation.invitation)
            .expect("invitation persists");

        let paired_receiver_db = fixture._dir.path().join("paired-receiver.sqlite3");
        let paired_target = fixture._dir.path().join("paired-target");
        let mut receiver_store = Store::open_file(&paired_receiver_db).expect("receiver opens");
        receiver_store
            .apply_migrations()
            .expect("receiver migrations apply");
        let receiver_identity = receiver_store
            .prepare_pairing_receiver_identity(&invitation.token, "Receiver laptop")
            .expect("receiver identity prepares");
        let join = bindhub_auth::create_pairing_join_request(
            &invitation.token,
            &receiver_identity.device_id,
        )
        .expect("join request creates");
        let pending_import = import_snapshot(
            &ImportSnapshotRequest {
                db_path: paired_receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: None,
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
        )
        .expect_err("pending receiver cannot import with all-zero key");
        assert!(pending_import
            .to_string()
            .contains("local identity is pending pairing completion"));
        let approval = bindhub_auth::approve_pairing_join_request(
            &source_view,
            &invitation.invitation,
            &invitation.token,
            &join,
            "Receiver laptop",
            "2026-06-18T10:01:00Z",
            101,
        )
        .expect("join request approves");
        source_store
            .persist_pairing_approval(&approval)
            .expect("pairing approval persists");
        let completion = bindhub_auth::pairing_completion_from_approval(&approval);
        let completed_receiver = receiver_store
            .complete_pairing_for_local_device(&completion)
            .expect("receiver completes pairing");
        assert_eq!(
            completed_receiver.sync_key_hex,
            source_identity.sync_key_hex
        );

        let outcome = materialize_snapshot(
            &MaterializationRequest {
                db_path: paired_receiver_db,
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: None,
                snapshot_id: snapshot_id.clone(),
                target: paired_target.clone(),
                apply: true,
            },
            &provider,
        )
        .expect("paired receiver materializes");

        assert_eq!(outcome.import.source_account_id, source_identity.account_id);
        assert_eq!(
            outcome.import.receiver_account_id,
            source_identity.account_id
        );
        assert_eq!(
            outcome.import.receiver_device_id,
            receiver_identity.device_id
        );
        assert_eq!(outcome.import.downloaded_blob_count, 1);
        assert!(outcome.applied);
        assert_eq!(
            fs::read_to_string(paired_target.join("README.md")).expect("readme restored"),
            "hello from paired source\n"
        );
    }

    #[test]
    fn hosted_publish_registers_device_project_and_snapshot_metadata() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "hosted metadata registration\n");
        let snapshot_id = fixture.persist_source_snapshot();
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        let mut metadata = InMemoryMetadataStore::default();

        let published = publish_snapshot_with_metadata(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
            &mut metadata,
        )
        .expect("snapshot publishes with metadata");

        let hosted = bindhub_metadata::MetadataStore::snapshot(
            &metadata,
            &published.account_id,
            &published.project_id,
            &snapshot_id,
        )
        .expect("hosted lookup succeeds")
        .expect("hosted snapshot exists");
        assert_eq!(hosted.snapshot_id, snapshot_id);
        assert_eq!(
            hosted.manifest_object_key,
            published.manifest_object_key.to_string()
        );
        assert_eq!(hosted.manifest_entry_count, 1);
        assert_eq!(
            hosted.total_size_bytes,
            "hosted metadata registration\n".len() as u64
        );
    }

    #[test]
    fn materialize_discovers_manifest_object_key_through_hosted_metadata() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "metadata-selected manifest\n");
        let snapshot_id = fixture.persist_source_snapshot();
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        let mut metadata = InMemoryMetadataStore::default();
        let published = publish_snapshot_with_metadata(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
            &mut metadata,
        )
        .expect("snapshot publishes with metadata");
        let sync_key = sync_key_for_fixture(&fixture.source_db);
        let original_key = published.manifest_object_key.clone();
        let original_encrypted = provider
            .get(&original_key)
            .expect("manifest get succeeds")
            .expect("manifest exists");
        let plaintext =
            decrypt_payload(&sync_key, &original_key, &original_encrypted).expect("decrypts");
        let alias_key = ObjectKey::new("encrypted/snapshots/metadata-alias/bundle-v1")
            .expect("alias key parses");
        let alias_encrypted =
            encrypt_payload(&sync_key, &alias_key, &plaintext).expect("alias encrypts");
        provider
            .put(&alias_key, &alias_encrypted)
            .expect("alias manifest writes");
        fs::remove_file(provider.path_for(&original_key)).expect("original manifest removes");
        republish_manifest_metadata_with_key(&mut metadata, &published, &alias_key);

        let outcome = materialize_snapshot_with_metadata(
            &MaterializationRequest {
                db_path: fixture.receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: Some(fixture.source_db.clone()),
                snapshot_id: snapshot_id.clone(),
                target: fixture.target.clone(),
                apply: true,
            },
            &provider,
            &mut metadata,
            &HostedMetadataImportOptions {
                account_id: published.account_id.clone(),
                project_id: published.project_id.clone(),
            },
        )
        .expect("metadata-backed materialization succeeds");

        assert_ne!(
            outcome.import.source_account_id,
            outcome.import.receiver_account_id
        );
        assert_eq!(outcome.import.manifest_object_key, alias_key);
        assert!(outcome.applied);
        assert_eq!(
            fs::read_to_string(fixture.target.join("README.md")).expect("materialized file reads"),
            "metadata-selected manifest\n"
        );
        assert_receiver_cursor(&fixture, &published.project_id, &snapshot_id);
        let hosted_cursor = bindhub_metadata::MetadataStore::cursor(
            &metadata,
            &published.account_id,
            &outcome.import.receiver_device_id,
            &published.project_id,
        )
        .expect("hosted cursor reads")
        .expect("hosted cursor exists");
        assert_eq!(
            hosted_cursor.cursor_value.as_deref(),
            Some(snapshot_id.as_str())
        );
    }

    #[test]
    fn hosted_cursor_conflict_prevents_local_cursor_advance() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "server cursor wins\n");
        let snapshot_id = fixture.persist_source_snapshot();
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        let mut metadata = InMemoryMetadataStore::default();
        let published = publish_snapshot_with_metadata(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
            &mut metadata,
        )
        .expect("snapshot publishes with metadata");
        let receiver_identity = local_identity_for_fixture(&fixture.receiver_db);
        assert_ne!(published.account_id, receiver_identity.account_id);
        bindhub_metadata::MetadataStore::upsert_device(
            &mut metadata,
            UpsertDeviceRequest {
                account_id: published.account_id.clone(),
                device_id: receiver_identity.device_id.clone(),
                display_name: receiver_identity.device_name.clone(),
                last_seen_at: "2026-06-18T11:59:00Z".to_string(),
            },
        )
        .expect("receiver device registers under hosted account");
        bindhub_metadata::MetadataStore::compare_and_set_cursor(
            &mut metadata,
            UpdateCursorRequest {
                account_id: published.account_id.clone(),
                device_id: receiver_identity.device_id.clone(),
                project_id: published.project_id.clone(),
                expected_cursor: None,
                next_cursor: Some("server-current".to_string()),
                updated_at: "2026-06-18T12:00:00Z".to_string(),
            },
        )
        .expect("server cursor advances first");

        let error = import_snapshot_with_metadata(
            &ImportSnapshotRequest {
                db_path: fixture.receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: Some(fixture.source_db.clone()),
                snapshot_id,
            },
            &provider,
            &mut metadata,
            &HostedMetadataImportOptions {
                account_id: published.account_id.clone(),
                project_id: published.project_id.clone(),
            },
        )
        .expect_err("stale local cursor is rejected");

        match error {
            MaterializeError::Metadata(MetadataError::CursorPreconditionFailed {
                current_cursor,
            }) => assert_eq!(current_cursor.as_deref(), Some("server-current")),
            error => panic!("unexpected error: {error}"),
        }
        let store = Store::open_file(&fixture.receiver_db).expect("receiver opens");
        store.apply_migrations().expect("migrations apply");
        assert!(store
            .device_project_cursor(
                &receiver_identity.account_id,
                &receiver_identity.device_id,
                &published.project_id
            )
            .expect("local cursor reads")
            .is_none());
        let hosted_cursor = bindhub_metadata::MetadataStore::cursor(
            &metadata,
            &published.account_id,
            &receiver_identity.device_id,
            &published.project_id,
        )
        .expect("hosted cursor reads")
        .expect("hosted cursor exists");
        assert_eq!(
            hosted_cursor.cursor_value.as_deref(),
            Some("server-current")
        );
    }

    #[test]
    fn blocked_secret_entries_are_not_published_or_materialized() {
        let fixture = FoundationFixture::new();
        let raw_secret = synthetic_token("github_pat_", "11AAabcdefghijklmnopqrstuvwxyz1234567890");
        fixture.write("README.md", "safe content\n");
        fixture.write("secrets.env", &format!("GITHUB_TOKEN={raw_secret}\n"));
        let snapshot_id = fixture.persist_source_snapshot();
        let source = Store::open_file(&fixture.source_db)
            .expect("source opens")
            .snapshot_with_entries(&snapshot_id)
            .expect("snapshot reads")
            .expect("snapshot exists");
        let blocked = source
            .entries
            .iter()
            .find(|entry| entry.relative_path == Path::new("secrets.env"))
            .expect("blocked entry exists");
        assert_eq!(blocked.blob_id, None);
        assert_eq!(blocked.object_ref, None);

        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        let published = publish_snapshot(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
        )
        .expect("snapshot publishes");
        assert_eq!(published.blob_count, 1);

        let outcome = materialize_snapshot(
            &MaterializationRequest {
                db_path: fixture.receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: Some(fixture.source_db.clone()),
                snapshot_id,
                target: fixture.target.clone(),
                apply: true,
            },
            &provider,
        )
        .expect("snapshot materializes");

        assert_eq!(outcome.plan.files_to_write, 1);
        assert!(fixture.target.join("README.md").is_file());
        assert!(!fixture.target.join("secrets.env").exists());
    }

    #[test]
    fn import_is_idempotent_and_can_refill_missing_receiver_cache() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "cached twice\n");
        let snapshot_id = fixture.persist_source_snapshot();
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        publish_snapshot(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
        )
        .expect("snapshot publishes");
        let request = ImportSnapshotRequest {
            db_path: fixture.receiver_db.clone(),
            cache_root: fixture.receiver_cache.clone(),
            key_source_db_path: Some(fixture.source_db.clone()),
            snapshot_id,
        };

        let first = import_snapshot(&request, &provider).expect("first import");
        assert_receiver_cursor(&fixture, &first.project_id, &first.snapshot_id);
        let blob_path = single_cache_blob(&fixture.receiver_cache);
        fs::remove_file(&blob_path).expect("receiver cache blob deletes");
        let second = import_snapshot(&request, &provider).expect("second import");

        assert!(first.snapshot_inserted);
        assert!(!second.snapshot_inserted);
        assert_eq!(second.downloaded_blob_count, 1);
        assert!(blob_path.is_file());
    }

    #[test]
    fn preflight_allows_when_local_has_not_diverged_from_base() {
        let fixture = FoundationFixture::new();
        let project_id = "project-preflight-ok";
        persist_manual_snapshot(
            &fixture.receiver_db,
            project_id,
            "snapshot-base",
            &[manual_file("README.md", b"base\n")],
        );
        persist_manual_snapshot(
            &fixture.receiver_db,
            project_id,
            "snapshot-incoming",
            &[manual_file("README.md", b"incoming\n")],
        );

        let outcome = sync_preflight(&SyncPreflightRequest {
            db_path: fixture.receiver_db.clone(),
            project_id: project_id.to_string(),
            base_snapshot_id: Some("snapshot-base".to_string()),
            local_snapshot_id: Some("snapshot-base".to_string()),
            incoming_snapshot_id: "snapshot-incoming".to_string(),
        })
        .expect("safe preflight succeeds");

        assert_eq!(outcome.status, SyncPreflightStatus::Ok);
        assert!(outcome.conflict.is_none());
        let store = Store::open_file(&fixture.receiver_db).expect("receiver opens");
        store.apply_migrations().expect("migrations apply");
        assert!(store
            .list_conflicts(Some(project_id))
            .expect("conflicts list")
            .is_empty());
    }

    #[test]
    fn preflight_blocks_unknown_base_when_local_state_exists() {
        let fixture = FoundationFixture::new();
        let project_id = "project-unknown-base";
        persist_manual_snapshot(
            &fixture.receiver_db,
            project_id,
            "snapshot-local",
            &[manual_file("README.md", b"local\n")],
        );
        persist_manual_snapshot(
            &fixture.receiver_db,
            project_id,
            "snapshot-incoming",
            &[manual_file("README.md", b"incoming\n")],
        );

        let outcome = sync_preflight(&SyncPreflightRequest {
            db_path: fixture.receiver_db.clone(),
            project_id: project_id.to_string(),
            base_snapshot_id: None,
            local_snapshot_id: Some("snapshot-local".to_string()),
            incoming_snapshot_id: "snapshot-incoming".to_string(),
        })
        .expect("unknown-base preflight persists a conflict");

        assert_eq!(outcome.status, SyncPreflightStatus::Blocked);
        let conflict = outcome.conflict.expect("conflict exists");
        assert_eq!(conflict.conflict.base_snapshot_id, None);
        assert_eq!(conflict.conflict.both_modified_different_count, 1);
    }

    #[test]
    fn import_preflight_blocks_divergence_without_cursor_advance_or_blob_download() {
        let fixture = FoundationFixture::new();
        let raw_secret = synthetic_token("sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456");
        fixture.write("README.md", "incoming\n");
        fixture.write("secret.env", &format!("OPENAI_API_KEY={raw_secret}\n"));
        let incoming_snapshot_id = fixture.persist_source_snapshot();
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        publish_snapshot(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: incoming_snapshot_id.clone(),
            },
            &provider,
        )
        .expect("snapshot publishes");
        let source = Store::open_file(&fixture.source_db)
            .expect("source opens")
            .snapshot_with_entries(&incoming_snapshot_id)
            .expect("source snapshot loads")
            .expect("source snapshot exists");
        let project_id = source.project.id.clone();

        persist_manual_snapshot(
            &fixture.receiver_db,
            &project_id,
            "snapshot-base",
            &[
                manual_file("README.md", b"base\n"),
                manual_secret(
                    "secret.env",
                    "secret blocked by policy rule openai_api_key at line 1: sk-<redacted>",
                ),
            ],
        );
        persist_manual_snapshot(
            &fixture.receiver_db,
            &project_id,
            "snapshot-local",
            &[
                manual_file("README.md", b"local\n"),
                manual_secret(
                    "secret.env",
                    "secret blocked by policy rule openai_api_key at line 1: sk-<redacted>",
                ),
            ],
        );
        set_receiver_cursor(&fixture, &project_id, "snapshot-base");

        let first = import_snapshot(
            &ImportSnapshotRequest {
                db_path: fixture.receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: Some(fixture.source_db.clone()),
                snapshot_id: incoming_snapshot_id.clone(),
            },
            &provider,
        )
        .expect_err("divergent import is blocked");
        let first_conflict_id = match first {
            MaterializeError::PreflightBlocked(outcome) => {
                assert_eq!(outcome.status, SyncPreflightStatus::Blocked);
                let conflict = outcome.conflict.expect("conflict exists");
                assert_eq!(conflict.conflict.local_snapshot_id, "snapshot-local");
                assert_eq!(conflict.conflict.incoming_snapshot_id, incoming_snapshot_id);
                assert_eq!(conflict.conflict.policy_blocked_count, 1);
                assert!(conflict.rows.iter().any(|row| {
                    matches!(
                        &row.local_policy_decision,
                        Some(PolicyDecision::RequiresUserDecision { reason })
                            if reason.contains("sk-<redacted>") && !reason.contains(&raw_secret)
                    )
                }));
                conflict.conflict.id
            }
            error => panic!("unexpected error: {error}"),
        };

        assert_receiver_cursor(&fixture, &project_id, "snapshot-base");
        assert!(!fixture.receiver_cache.join("blobs").exists());

        let second = import_snapshot(
            &ImportSnapshotRequest {
                db_path: fixture.receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: Some(fixture.source_db.clone()),
                snapshot_id: incoming_snapshot_id,
            },
            &provider,
        )
        .expect_err("repeat divergent import is blocked");
        match second {
            MaterializeError::PreflightBlocked(outcome) => {
                assert_eq!(outcome.conflict_id(), Some(first_conflict_id.as_str()));
            }
            error => panic!("unexpected error: {error}"),
        }
        let store = Store::open_file(&fixture.receiver_db).expect("receiver opens");
        store.apply_migrations().expect("migrations apply");
        let conflicts = store
            .list_conflicts(Some(&project_id))
            .expect("conflicts list");
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn missing_remote_blob_fails_before_metadata_import() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "missing remote blob\n");
        let snapshot_id = fixture.persist_source_snapshot();
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        let published = publish_snapshot(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
        )
        .expect("snapshot publishes");
        let snapshot = Store::open_file(&fixture.source_db)
            .expect("source opens")
            .snapshot_with_entries(&snapshot_id)
            .expect("snapshot reads")
            .expect("snapshot exists");
        let blob_id = snapshot.entries[0].blob_id.clone().expect("blob id exists");
        fs::remove_file(provider.path_for(&encrypted_blob_object_key(&blob_id)))
            .expect("remote blob deletes");

        let error = import_snapshot(
            &ImportSnapshotRequest {
                db_path: fixture.receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: Some(fixture.source_db.clone()),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
        )
        .expect_err("missing blob fails");

        assert!(matches!(
            error,
            MaterializeError::Sync(SyncError::MissingRemoteObject(_))
        ));
        assert!(published.manifest_uploaded);
        let receiver = Store::open_file(&fixture.receiver_db).expect("receiver opens");
        receiver.apply_migrations().expect("migrations apply");
        assert!(receiver
            .snapshot_with_entries(&snapshot_id)
            .expect("snapshot query")
            .is_none());
    }

    #[test]
    fn safe_materialization_refuses_non_empty_target() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "restore me\n");
        fs::create_dir_all(&fixture.target).expect("target creates");
        fs::write(fixture.target.join("keep.txt"), "keep").expect("existing file writes");
        let snapshot_id = fixture.persist_source_snapshot();
        let project_id = project_id_for_snapshot(&fixture.source_db, &snapshot_id);
        set_receiver_cursor(&fixture, &project_id, "snapshot-before-materialize");
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        publish_snapshot(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
        )
        .expect("snapshot publishes");

        let error = materialize_snapshot(
            &MaterializationRequest {
                db_path: fixture.receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: Some(fixture.source_db.clone()),
                snapshot_id,
                target: fixture.target.clone(),
                apply: true,
            },
            &provider,
        )
        .expect_err("non-empty target fails");

        assert!(matches!(
            error,
            MaterializeError::Restore(RestorePlanError::ApplyNotAllowed { .. })
        ));
        assert_eq!(
            fs::read_to_string(fixture.target.join("keep.txt")).expect("existing file reads"),
            "keep"
        );
        assert_receiver_cursor(&fixture, &project_id, "snapshot-before-materialize");
    }

    #[test]
    fn dry_run_materialization_reports_non_empty_target_without_applying() {
        let fixture = FoundationFixture::new();
        fixture.write("README.md", "restore me\n");
        fs::create_dir_all(&fixture.target).expect("target creates");
        fs::write(fixture.target.join("keep.txt"), "keep").expect("existing file writes");
        let snapshot_id = fixture.persist_source_snapshot();
        let provider = LocalFilesystemBlobProvider::open(&fixture.remote).expect("remote opens");
        publish_snapshot(
            &PublishSnapshotRequest {
                db_path: fixture.source_db.clone(),
                cache_root: fixture.source_cache.clone(),
                snapshot_id: snapshot_id.clone(),
            },
            &provider,
        )
        .expect("snapshot publishes");

        let outcome = materialize_snapshot(
            &MaterializationRequest {
                db_path: fixture.receiver_db.clone(),
                cache_root: fixture.receiver_cache.clone(),
                key_source_db_path: Some(fixture.source_db.clone()),
                snapshot_id,
                target: fixture.target.clone(),
                apply: false,
            },
            &provider,
        )
        .expect("dry run succeeds");

        assert_eq!(
            outcome.target_status,
            RestoreTargetStatus::NonEmptyDirectory.as_str()
        );
        assert!(!outcome.apply_allowed);
        assert!(!outcome.applied);
    }

    struct FoundationFixture {
        _dir: tempfile::TempDir,
        project: PathBuf,
        source_db: PathBuf,
        source_cache: PathBuf,
        receiver_db: PathBuf,
        receiver_cache: PathBuf,
        remote: PathBuf,
        target: PathBuf,
    }

    impl FoundationFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let fixture = Self {
                project: dir.path().join("project"),
                source_db: dir.path().join("source.sqlite3"),
                source_cache: dir.path().join("source-cache"),
                receiver_db: dir.path().join("receiver.sqlite3"),
                receiver_cache: dir.path().join("receiver-cache"),
                remote: dir.path().join("remote"),
                target: dir.path().join("target"),
                _dir: dir,
            };
            fs::create_dir_all(&fixture.project).expect("project creates");
            fixture.init_identity(&fixture.source_db, "Desk");
            fixture.init_identity(&fixture.receiver_db, "Laptop");
            fixture
        }

        fn init_identity(&self, db: &Path, name: &str) {
            let mut store = Store::open_file(db).expect("store opens");
            store.apply_migrations().expect("migrations apply");
            store
                .ensure_local_identity(&EnsureLocalIdentityOptions {
                    device_name: Some(name),
                })
                .expect("identity initializes");
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.project.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent creates");
            }
            fs::write(path, content).expect("fixture file writes");
        }

        fn persist_source_snapshot(&self) -> String {
            let cache = BlobCache::open(&self.source_cache).expect("source cache opens");
            let snapshot = SnapshotManifestBuilder::new(cache)
                .build_draft(&self.project)
                .expect("snapshot builds");
            let mut store = Store::open_file(&self.source_db).expect("source opens");
            store.apply_migrations().expect("migrations apply");
            let created_at = store.current_timestamp().expect("timestamp reads");
            let project_id = local_project_id(snapshot.root()).to_string();
            let root_path = snapshot.root().display().to_string();
            let entries = snapshot
                .entries()
                .iter()
                .map(|entry| NewSnapshotManifestEntry {
                    relative_path: entry.relative_path(),
                    kind: entry.kind().clone(),
                    size_bytes: entry.size_bytes().unwrap_or_default(),
                    blob_id: entry.blob_id(),
                    object_ref: entry.object_ref(),
                    policy_decision: entry.policy_decision(),
                })
                .collect::<Vec<_>>();
            let snapshot_id = snapshot.id().to_string();
            let draft = NewSnapshotDraft {
                project: NewProject {
                    id: &project_id,
                    root_path: &root_path,
                    kind: "local",
                    display_name: "project",
                    discovered_at: &created_at,
                },
                snapshot: NewSnapshot {
                    id: &snapshot_id,
                    project_id: &project_id,
                    parent_snapshot_id: None,
                    created_at: &created_at,
                    reason: "test",
                    manifest_entry_count: snapshot.summary().total_entries() as u64,
                    total_size_bytes: snapshot.summary().total_file_bytes(),
                },
                entries,
            };
            store
                .persist_draft_snapshot(&draft)
                .expect("snapshot persists");
            snapshot_id
        }
    }

    fn single_cache_blob(root: &Path) -> PathBuf {
        let mut files = Vec::new();
        collect_files(&root.join("blobs"), &mut files);
        assert_eq!(files.len(), 1);
        files.pop().expect("one blob exists")
    }

    fn collect_files(path: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(path).expect("directory reads") {
            let entry = entry.expect("entry reads");
            let path = entry.path();
            if path.is_dir() {
                collect_files(&path, files);
            } else {
                files.push(path);
            }
        }
    }

    fn synthetic_token(prefix: &str, tail: &str) -> String {
        [prefix, tail].concat()
    }

    #[derive(Debug, Clone)]
    struct ManualEntry {
        path: PathBuf,
        kind: ManifestEntryKind,
        size_bytes: u64,
        blob_id: Option<BlobId>,
        object_ref: Option<String>,
        policy_decision: PolicyDecision,
    }

    fn manual_file(path: &str, content: &[u8]) -> ManualEntry {
        let blob_id = blob_id_for(content);
        ManualEntry {
            path: PathBuf::from(path),
            kind: ManifestEntryKind::File,
            size_bytes: content.len() as u64,
            object_ref: Some(format!("blobs/b3/{}", blob_id.as_str())),
            blob_id: Some(blob_id),
            policy_decision: PolicyDecision::Include,
        }
    }

    fn manual_secret(path: &str, reason: &str) -> ManualEntry {
        ManualEntry {
            path: PathBuf::from(path),
            kind: ManifestEntryKind::File,
            size_bytes: 0,
            blob_id: None,
            object_ref: None,
            policy_decision: PolicyDecision::RequiresUserDecision {
                reason: reason.to_string(),
            },
        }
    }

    fn persist_manual_snapshot(
        db_path: &Path,
        project_id: &str,
        snapshot_id: &str,
        entries: &[ManualEntry],
    ) {
        let mut store = Store::open_file(db_path).expect("store opens");
        store.apply_migrations().expect("migrations apply");
        let created_at = store.current_timestamp().expect("timestamp reads");
        let draft_entries = entries
            .iter()
            .map(|entry| NewSnapshotManifestEntry {
                relative_path: &entry.path,
                kind: entry.kind.clone(),
                size_bytes: entry.size_bytes,
                blob_id: entry.blob_id.as_ref(),
                object_ref: entry.object_ref.as_deref(),
                policy_decision: &entry.policy_decision,
            })
            .collect::<Vec<_>>();
        let total_size_bytes = entries.iter().map(|entry| entry.size_bytes).sum();
        let draft = NewSnapshotDraft {
            project: NewProject {
                id: project_id,
                root_path: "/workspace/bindhub",
                kind: "local",
                display_name: "bindhub",
                discovered_at: &created_at,
            },
            snapshot: NewSnapshot {
                id: snapshot_id,
                project_id,
                parent_snapshot_id: None,
                created_at: &created_at,
                reason: "test",
                manifest_entry_count: entries.len() as u64,
                total_size_bytes,
            },
            entries: draft_entries,
        };
        store
            .persist_draft_snapshot(&draft)
            .expect("manual snapshot persists");
    }

    fn blob_id_for(content: &[u8]) -> BlobId {
        BlobId::from_blake3_hex(blake3::hash(content).to_hex().to_string())
            .expect("BLAKE3 produces valid blob ids")
    }

    fn set_receiver_cursor(fixture: &FoundationFixture, project_id: &str, cursor_value: &str) {
        let store = Store::open_file(&fixture.receiver_db).expect("receiver opens");
        store.apply_migrations().expect("migrations apply");
        let identity = store
            .local_identity()
            .expect("identity reads")
            .expect("identity exists");
        store
            .upsert_device_project_cursor(&DeviceProjectCursor {
                account_id: identity.account_id,
                device_id: identity.device_id,
                project_id: project_id.to_string(),
                cursor_value: cursor_value.to_string(),
                updated_at: store.current_timestamp().expect("timestamp reads"),
            })
            .expect("cursor persists");
    }

    fn project_id_for_snapshot(db_path: &Path, snapshot_id: &str) -> String {
        Store::open_file(db_path)
            .expect("store opens")
            .snapshot_with_entries(snapshot_id)
            .expect("snapshot reads")
            .expect("snapshot exists")
            .project
            .id
    }

    fn assert_receiver_cursor(fixture: &FoundationFixture, project_id: &str, expected: &str) {
        let store = Store::open_file(&fixture.receiver_db).expect("receiver opens");
        store.apply_migrations().expect("migrations apply");
        let identity = store
            .local_identity()
            .expect("identity reads")
            .expect("identity exists");
        let cursor = store
            .device_project_cursor(&identity.account_id, &identity.device_id, project_id)
            .expect("cursor reads")
            .expect("cursor exists");
        assert_eq!(cursor.cursor_value, expected);
    }

    fn sync_key_for_fixture(db_path: &Path) -> SyncKey {
        let identity = local_identity_for_fixture(db_path);
        SyncKey::from_hex(&identity.sync_key_hex).expect("fixture sync key parses")
    }

    fn local_identity_for_fixture(db_path: &Path) -> bindhub_store::LocalIdentityRecord {
        let store = Store::open_file(db_path).expect("store opens");
        store.apply_migrations().expect("migrations apply");
        store
            .local_identity()
            .expect("identity reads")
            .expect("identity exists")
    }

    fn local_identity_view_for_fixture(
        identity: &bindhub_store::LocalIdentityRecord,
    ) -> bindhub_auth::LocalIdentityView {
        bindhub_auth::LocalIdentityView {
            account_id: identity.account_id.clone(),
            device_id: identity.device_id.clone(),
            device_name: identity.device_name.clone(),
            sync_key_hex: identity.sync_key_hex.clone(),
        }
    }

    fn republish_manifest_metadata_with_key(
        metadata: &mut InMemoryMetadataStore,
        published: &PublishedSnapshotBundle,
        manifest_object_key: &ObjectKey,
    ) {
        bindhub_metadata::MetadataStore::publish_snapshot(
            metadata,
            MetadataPublishSnapshotRequest {
                account_id: published.account_id.clone(),
                project_id: published.project_id.clone(),
                snapshot_id: published.snapshot_id.clone(),
                parent_snapshot_id: None,
                manifest_object_key: manifest_object_key.to_string(),
                manifest_hash: "blake3:metadata-alias".to_string(),
                manifest_entry_count: published.blob_count as u64,
                total_size_bytes: published.plaintext_blob_bytes,
                published_by_device_id: published.device_id.clone(),
                published_at: "2026-06-18T12:01:00Z".to_string(),
            },
        )
        .expect("metadata alias publishes");
    }

    struct HostedMetadataServer {
        url: String,
    }

    fn start_hosted_metadata_server(account_id: &str, raw_token: &str) -> HostedMetadataServer {
        let mut store = InMemoryMetadataStore::default();
        seed_account_session(&mut store, account_id, raw_token);
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("hosted metadata listener binds");
        listener
            .set_nonblocking(true)
            .expect("listener set nonblocking");
        let addr = listener.local_addr().expect("listener addr reads");
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime creates");
            runtime.block_on(async move {
                let listener =
                    tokio::net::TcpListener::from_std(listener).expect("tokio listener wraps");
                axum::serve(
                    listener,
                    app_with_config(store, HostedApiConfig::hosted_alpha()),
                )
                .await
                .expect("hosted metadata server runs");
            });
        });
        let server = HostedMetadataServer {
            url: format!("http://{addr}"),
        };
        for _ in 0..50 {
            if ureq::get(&format!("{}/health", server.url)).call().is_ok() {
                return server;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("hosted metadata server did not become ready");
    }

    fn seed_account_session(store: &mut InMemoryMetadataStore, account_id: &str, raw_token: &str) {
        let proof =
            bindhub_auth::create_account_ownership_proof(bindhub_auth::AccountOwnershipProofInput {
                account_id,
                provider_kind: "alpha-invite",
                provider_issuer: "bindhub-alpha",
                provider_subject: "user@example.com",
                verified_email: Some("user@example.com"),
                verified_domain: None,
                proof_issued_at: "2026-06-19T09:00:00Z",
                proof_expires_at_unix: 4_000_000_000,
            })
            .expect("proof creates");
        store
            .upsert_account_ownership_proof(proof.clone())
            .expect("proof upserts");
        let session = bindhub_auth::create_account_session(
            &proof,
            raw_token,
            "2026-06-19T09:01:00Z",
            101,
            4_000_000_000,
        )
        .expect("session creates");
        store
            .upsert_account_session(session)
            .expect("session upserts");
    }

    struct RacingManifestProvider {
        key: ObjectKey,
        existing: Vec<u8>,
        get_calls: Cell<usize>,
        put_calls: Cell<usize>,
    }

    impl RacingManifestProvider {
        fn new(key: ObjectKey, existing: Vec<u8>) -> Self {
            Self {
                key,
                existing,
                get_calls: Cell::new(0),
                put_calls: Cell::new(0),
            }
        }
    }

    impl RemoteBlobProvider for RacingManifestProvider {
        fn put(&self, key: &ObjectKey, _bytes: &[u8]) -> SyncResult<PutOutcome> {
            self.put_calls.set(self.put_calls.get() + 1);
            Err(SyncError::RemoteObjectAlreadyExists { key: key.clone() })
        }

        fn get(&self, key: &ObjectKey) -> SyncResult<Option<Vec<u8>>> {
            self.get_calls.set(self.get_calls.get() + 1);
            if self.get_calls.get() == 1 {
                return Ok(None);
            }
            if key == &self.key {
                Ok(Some(self.existing.clone()))
            } else {
                Ok(None)
            }
        }

        fn head(&self, key: &ObjectKey) -> SyncResult<Option<ObjectMetadata>> {
            if key == &self.key {
                Ok(Some(ObjectMetadata {
                    key: key.clone(),
                    size_bytes: self.existing.len() as u64,
                }))
            } else {
                Ok(None)
            }
        }
    }
}
