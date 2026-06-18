//! Local/mock second-device snapshot publish, import, and materialization.

use devbox_auth::DeviceProjectCursor;
use devbox_core::{BlobId, DomainIdError, ManifestEntryKind, PolicyDecision};
use devbox_snapshot::{RestoreMaterializer, RestorePlan, RestorePlanError, RestorePlanSummary};
use devbox_store::{
    path_to_store_string, BlobCache, BlobCacheError, ManifestEntryRecord, NewProject, NewSnapshot,
    NewSnapshotDraft, NewSnapshotManifestEntry, PersistedSnapshot, Store, StoreError,
};
use devbox_sync::{
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
            Self::SnapshotNotFound(id) => write!(f, "snapshot not found: {id}"),
            Self::LocalIdentityMissing => {
                f.write_str("local identity is not initialized; run devbox init --db <DB_PATH>")
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
            Self::SnapshotNotFound(_)
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

pub type MaterializeResult<T> = Result<T, MaterializeError>;

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
    provider: &impl RemoteBlobProvider,
) -> MaterializeResult<PublishedSnapshotBundle> {
    let store = open_store(&request.db_path)?;
    let identity = store
        .local_identity()?
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

    Ok(PublishedSnapshotBundle {
        account_id: identity.account_id,
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

pub fn import_snapshot(
    request: &ImportSnapshotRequest,
    provider: &impl RemoteBlobProvider,
) -> MaterializeResult<ImportedSnapshotBundle> {
    let mut receiver_store = open_store(&request.db_path)?;
    let receiver_identity = receiver_store
        .local_identity()?
        .ok_or(MaterializeError::LocalIdentityMissing)?;
    let sync_key = sync_key_for_import(request, &receiver_identity.sync_key_hex)?;
    let manifest_object_key = published_snapshot_object_key(&request.snapshot_id)?;
    let encrypted = provider
        .get(&manifest_object_key)?
        .ok_or_else(|| SyncError::MissingRemoteObject(manifest_object_key.clone()))?;
    let plaintext = decrypt_payload(&sync_key, &manifest_object_key, &encrypted)?;
    let envelope: SnapshotBundleEnvelope = serde_json::from_slice(&plaintext)?;
    envelope.validate(&request.snapshot_id)?;

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

    let updated_at = receiver_store.current_timestamp()?;
    let cursor = DeviceProjectCursor {
        account_id: receiver_identity.account_id.clone(),
        device_id: receiver_identity.device_id.clone(),
        project_id: envelope.project.id.clone(),
        cursor_value: envelope.snapshot.id.clone(),
        updated_at: updated_at.clone(),
    };
    receiver_store.upsert_device_project_cursor(&cursor)?;

    Ok(ImportedSnapshotBundle {
        source_account_id: envelope.account_id,
        receiver_account_id: receiver_identity.account_id,
        receiver_device_id: receiver_identity.device_id,
        project_id: envelope.project.id,
        snapshot_id: envelope.snapshot.id,
        manifest_object_key,
        snapshot_inserted,
        blob_count: envelope.included_blobs.len(),
        downloaded_blob_count,
        plaintext_blob_bytes,
        remote_blob_bytes,
        cursor_value: cursor.cursor_value,
        cursor_updated_at: updated_at,
    })
}

pub fn materialize_snapshot(
    request: &MaterializationRequest,
    provider: &impl RemoteBlobProvider,
) -> MaterializeResult<MaterializationOutcome> {
    let import = import_snapshot(
        &ImportSnapshotRequest {
            db_path: request.db_path.clone(),
            cache_root: request.cache_root.clone(),
            key_source_db_path: request.key_source_db_path.clone(),
            snapshot_id: request.snapshot_id.clone(),
        },
        provider,
    )?;

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
            .local_identity()?
            .ok_or(MaterializeError::LocalIdentityMissing)?
            .sync_key_hex
    } else {
        receiver_key_hex.to_string()
    };

    Ok(SyncKey::from_hex(&key_hex)?)
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
    provider: &impl RemoteBlobProvider,
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
    let put = provider.put(object_key, &encrypted)?;
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
    fn from_record(record: &devbox_store::ProjectRecord) -> Self {
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
    fn from_record(record: &devbox_store::SnapshotRecord) -> Self {
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
    use devbox_snapshot::{RestoreTargetStatus, SnapshotManifestBuilder};
    use devbox_store::{
        local_project_id, BlobCache, EnsureLocalIdentityOptions, NewProject, NewSnapshot,
        NewSnapshotDraft, NewSnapshotManifestEntry,
    };
    use devbox_sync::LocalFilesystemBlobProvider;
    use std::fs;

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
        let blob_path = single_cache_blob(&fixture.receiver_cache);
        fs::remove_file(&blob_path).expect("receiver cache blob deletes");
        let second = import_snapshot(&request, &provider).expect("second import");

        assert!(first.snapshot_inserted);
        assert!(!second.snapshot_inserted);
        assert_eq!(second.downloaded_blob_count, 1);
        assert!(blob_path.is_file());
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
}
