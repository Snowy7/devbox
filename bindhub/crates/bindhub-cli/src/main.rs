use bindhub_auth::{
    approve_pairing_invitation, approve_pairing_join_request, create_account_ownership_proof,
    create_account_session, create_device_rotation_intent, create_pairing_invitation,
    create_pairing_join_request, create_recovery_grant, generate_alpha_invite_code, mock_login,
    now_unix_seconds, pairing_completion_from_approval, revoke_account_session,
    validate_account_session, AccountOwnershipProofInput, DeviceProjectCursor,
    DeviceRotationIntent, DeviceRotationIntentInput, DeviceTrustRecord, LocalIdentityView,
    PairingCompletion, PairingInvitationToken, PairingJoinRequest, RecoveryGrant,
};
use bindhub_conflict::{
    compare_snapshots, path_to_conflict_string, ComparableEntry, ComparableSnapshot,
    PathComparisonRow,
};
use bindhub_core::scanner::ProjectScanner;
use bindhub_core::{BlobId, ManifestEntryKind, PolicyDecision};
use bindhub_materialize::{
    import_snapshot, import_snapshot_with_metadata, materialize_snapshot,
    materialize_snapshot_with_metadata, publish_snapshot, publish_snapshot_with_metadata,
    sync_preflight, HostedMetadataApiClient, HostedMetadataApiConfig, HostedMetadataImportOptions,
    ImportSnapshotRequest, MaterializationRequest, MaterializeError, PublishSnapshotRequest,
    SyncPreflightOutcome, SyncPreflightRequest,
};
use bindhub_metadata::{
    active_managed_object_credential_lease_for_session, authenticate_account_session,
    create_alpha_invite_request, redacted_managed_object_remote_config, AlphaLoginResponse,
    AuthSessionResponse, ManagedObjectAccessGrant, ManagedObjectAccessRequest,
    ManagedObjectCapability, ManagedObjectCredentialLeaseRequest, ManagedObjectProviderKind,
    MetadataAuthMode, MetadataServiceConfig, MetadataStore, PostgresMetadataStore,
    SqliteMetadataStore,
};
use bindhub_snapshot::{
    is_secret_block_reason, preflight_cache_root, preflight_db_path, scan_local_change_feed,
    LocalChangeFeedScanOptions, RestoreMaterializer, RestorePlan, RestoreSkippedEntry,
    RestoreTargetStatus, RestoreWrite, SnapshotManifestBuilder, SnapshotManifestEntry,
};
use bindhub_store::{
    local_project_id, path_to_store_string, validate_secret_envelope_reference, BlobCache,
    ConflictRowRecord, ConflictStatus, EnsureLocalIdentityOptions, LocalChangeKind,
    LocalIdentityRecord, ManifestEntryRecord, NewConflict, NewConflictRow, NewProject,
    NewSecretPolicyRule, NewSnapshot, NewSnapshotDraft, NewSnapshotManifestEntry,
    PendingLocalChangeRecord, PersistedSnapshot, SecretPolicyAction, SecretPolicyRuleRecord, Store,
};
use bindhub_sync::{
    download_blob_to_cache, encrypted_blob_object_key, upload_blob_from_cache,
    HostedObjectTransferConfig, HostedObjectTransferProvider, HostedRedactedConfig,
    LocalFilesystemBlobProvider, ObjectKey, RemoteBlobProvider, S3CompatibleBlobProvider,
    S3CompatibleConfig, S3CredentialsSource, S3RedactedConfig, SyncKey,
};
mod product;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") | Some("version") => {
            println!("bindhub {VERSION}");
            ExitCode::SUCCESS
        }
        Some("login") => product::run_command("login", &args[1..]),
        Some("share") => product::run_command("share", &args[1..]),
        Some("clone") => product::run_command("clone", &args[1..]),
        Some("manage") => product::run_command("manage", &args[1..]),
        Some("doctor") => product::run_command("doctor", &args[1..]),
        Some("pause") => product::run_command("pause", &args[1..]),
        Some("resume") => product::run_command("resume", &args[1..]),
        Some("unlink") => product::run_command("unlink", &args[1..]),
        Some("warm") => product::run_command("warm", &args[1..]),
        Some("hydrate") => product::run_command("hydrate", &args[1..]),
        Some("keep") => product::run_command("keep", &args[1..]),
        Some("free-space") => product::run_command("free-space", &args[1..]),
        Some("scan") => run_scan(&args[1..]),
        Some("init") => run_init(&args[1..]),
        Some("auth") => run_auth(&args[1..]),
        Some("devices") => run_devices(&args[1..]),
        Some("metadata") => run_metadata(&args[1..]),
        Some("sync") => run_sync(&args[1..]),
        Some("conflicts") => run_conflicts(&args[1..]),
        Some("secrets") => run_secrets(&args[1..]),
        Some("status") => run_status(&args[1..]),
        Some("snapshot") => run_snapshot(&args[1..]),
        Some("changes") => run_changes(&args[1..]),
        Some("restore" | "explain") => {
            println!("bindhub: command placeholder; daemon integration is not implemented yet");
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("bindhub: unknown command '{command}'");
            eprintln!("Run 'bindhub --help' for usage.");
            ExitCode::from(2)
        }
    }
}

fn run_changes(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("scan") => match parse_changes_scan_args(&args[1..])
            .and_then(|args| changes_scan(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                ExitCode::from(1)
            }
        },
        Some("list") => match parse_changes_list_args(&args[1..])
            .and_then(|args| changes_list(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                ExitCode::from(1)
            }
        },
        Some("clear") => match parse_changes_list_args(&args[1..])
            .and_then(|args| changes_clear(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("bindhub: changes requires scan, list, or clear");
            print_changes_usage();
            ExitCode::from(2)
        }
    }
}

fn run_conflicts(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("compare") => match parse_conflict_compare_args(&args[1..])
            .and_then(|args| conflicts_compare(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_conflicts_usage();
                ExitCode::from(1)
            }
        },
        Some("list") => match parse_conflict_list_args(&args[1..])
            .and_then(|args| conflicts_list(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_conflicts_usage();
                ExitCode::from(1)
            }
        },
        Some("show") => match parse_conflict_show_args(&args[1..])
            .and_then(|args| conflicts_show(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_conflicts_usage();
                ExitCode::from(1)
            }
        },
        Some("resolve") => match parse_conflict_resolve_args(&args[1..])
            .and_then(|args| conflicts_resolve(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_conflicts_usage();
                ExitCode::from(1)
            }
        },
        Some("dismiss") => match parse_conflict_show_args(&args[1..]).and_then(|args| {
            conflicts_update_status(&args, ConflictStatus::Dismissed)
                .map_err(|error| error.to_string())
        }) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_conflicts_usage();
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("bindhub: conflicts requires compare, list, show, resolve, or dismiss");
            print_conflicts_usage();
            ExitCode::from(2)
        }
    }
}

fn run_secrets(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("policy") => match args.get(1).map(String::as_str) {
            Some("add") => match parse_secret_policy_add_args(&args[2..])
                .and_then(|args| secret_policy_add(&args).map_err(|error| error.to_string()))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    print_secrets_usage();
                    ExitCode::from(1)
                }
            },
            Some("list") => match parse_secret_policy_list_args(&args[2..])
                .and_then(|args| secret_policy_list(&args).map_err(|error| error.to_string()))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    print_secrets_usage();
                    ExitCode::from(1)
                }
            },
            _ => {
                eprintln!("bindhub: secrets policy requires add or list");
                print_secrets_usage();
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("bindhub: secrets requires policy");
            print_secrets_usage();
            ExitCode::from(2)
        }
    }
}

fn run_snapshot(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("restore") => match snapshot_restore(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                ExitCode::from(1)
            }
        },
        Some("list") => match snapshot_list(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                ExitCode::from(1)
            }
        },
        Some("show") => match snapshot_show(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                ExitCode::from(1)
            }
        },
        _ => match parse_snapshot_create_args(args) {
            Ok(create_args) if create_args.dry_run => {
                match snapshot_dry_run(&create_args.cache_root, &create_args.path) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("bindhub: {error}");
                        ExitCode::from(1)
                    }
                }
            }
            Ok(create_args) => match snapshot_create(&create_args) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    ExitCode::from(1)
                }
            },
            Err(message) => {
                eprintln!("bindhub: {message}");
                print_snapshot_usage();
                ExitCode::from(2)
            }
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotCreateArgs {
    db_path: Option<String>,
    cache_root: String,
    dry_run: bool,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotRestoreArgs {
    db_path: String,
    cache_root: String,
    target: String,
    snapshot_id: String,
    apply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangesScanArgs {
    db_path: String,
    cache_root: String,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChangesListArgs {
    db_path: String,
    project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConflictCompareArgs {
    db_path: String,
    base_snapshot_id: Option<String>,
    local_snapshot_id: String,
    incoming_snapshot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConflictListArgs {
    db_path: String,
    project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConflictShowArgs {
    db_path: String,
    conflict_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConflictResolveArgs {
    db_path: String,
    conflict_id: String,
    manual_resolution: ManualConflictResolution,
    confirm_no_auto_apply: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManualConflictResolution {
    KeepLocal,
    KeepIncoming,
    KeepBoth,
    Exported,
}

impl ManualConflictResolution {
    fn as_str(self) -> &'static str {
        match self {
            Self::KeepLocal => "keep-local",
            Self::KeepIncoming => "keep-incoming",
            Self::KeepBoth => "keep-both",
            Self::Exported => "exported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SecretPolicyAddArgs {
    db_path: String,
    project_id: String,
    path: String,
    action: SecretPolicyAction,
    envelope_ref: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SecretPolicyListArgs {
    db_path: String,
    project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InitArgs {
    db_path: String,
    device_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DbOnlyArgs {
    db_path: String,
}

#[derive(Clone, PartialEq, Eq)]
struct AuthMockVerifiedBootstrapArgs {
    db_path: String,
    provider_kind: String,
    provider_issuer: String,
    provider_subject: Option<String>,
    verified_email: Option<String>,
    verified_domain: Option<String>,
    session_token: String,
    ttl_seconds: i64,
    proof_ttl_seconds: i64,
}

#[derive(Clone, PartialEq, Eq)]
struct AuthProofCheckArgs {
    db_path: String,
    session_token: String,
}

#[derive(Clone, PartialEq, Eq)]
struct AuthHostedLoginArgs {
    api: String,
    email: String,
    invite_code: Option<String>,
    invite_code_env: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct AuthHostedSessionArgs {
    api: String,
    session_token_env: String,
}

impl std::fmt::Debug for AuthMockVerifiedBootstrapArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthMockVerifiedBootstrapArgs")
            .field("db_path", &self.db_path)
            .field("provider_kind", &self.provider_kind)
            .field("provider_issuer", &self.provider_issuer)
            .field("provider_subject", &self.provider_subject)
            .field("verified_email", &self.verified_email)
            .field("verified_domain", &self.verified_domain)
            .field("session_token", &"<redacted>")
            .field("ttl_seconds", &self.ttl_seconds)
            .field("proof_ttl_seconds", &self.proof_ttl_seconds)
            .finish()
    }
}

impl std::fmt::Debug for AuthProofCheckArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthProofCheckArgs")
            .field("db_path", &self.db_path)
            .field("session_token", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Debug for AuthHostedLoginArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthHostedLoginArgs")
            .field("api", &self.api)
            .field("email", &self.email)
            .field(
                "invite_code",
                &self.invite_code.as_ref().map(|_| "<redacted>"),
            )
            .field("invite_code_env", &self.invite_code_env)
            .finish()
    }
}

impl std::fmt::Debug for AuthHostedSessionArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthHostedSessionArgs")
            .field("api", &self.api)
            .field("session_token_env", &self.session_token_env)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthRevokeSessionArgs {
    db_path: String,
    session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceInviteArgs {
    db_path: String,
    ttl_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceApproveArgs {
    db_path: String,
    token: String,
    device_name: String,
}

#[derive(Clone, PartialEq, Eq)]
struct DeviceJoinArgs {
    db_path: String,
    token: Option<String>,
    token_env: Option<String>,
    device_name: String,
}

#[derive(Clone, PartialEq, Eq)]
struct DeviceApproveJoinArgs {
    db_path: String,
    token: Option<String>,
    token_env: Option<String>,
    join_request: Option<String>,
    join_request_env: Option<String>,
    device_name: String,
}

#[derive(Clone, PartialEq, Eq)]
struct DeviceCompleteArgs {
    db_path: String,
    completion: Option<String>,
    completion_env: Option<String>,
}

impl std::fmt::Debug for DeviceJoinArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceJoinArgs")
            .field("db_path", &self.db_path)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("token_env", &self.token_env)
            .field("device_name", &self.device_name)
            .finish()
    }
}

impl std::fmt::Debug for DeviceApproveJoinArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceApproveJoinArgs")
            .field("db_path", &self.db_path)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("token_env", &self.token_env)
            .field(
                "join_request",
                &self.join_request.as_ref().map(|_| "<redacted>"),
            )
            .field("join_request_env", &self.join_request_env)
            .field("device_name", &self.device_name)
            .finish()
    }
}

impl std::fmt::Debug for DeviceCompleteArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceCompleteArgs")
            .field("db_path", &self.db_path)
            .field(
                "completion",
                &self.completion.as_ref().map(|_| "<redacted>"),
            )
            .field("completion_env", &self.completion_env)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceRevokeArgs {
    db_path: String,
    device_id: String,
    reason: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
struct DeviceRecoveryGrantArgs {
    db_path: String,
    device_id: String,
    recovery_ref: String,
    audit_label: String,
    ttl_seconds: i64,
}

impl std::fmt::Debug for DeviceRecoveryGrantArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceRecoveryGrantArgs")
            .field("db_path", &self.db_path)
            .field("device_id", &self.device_id)
            .field("recovery_ref", &"<redacted>")
            .field("audit_label", &self.audit_label)
            .field("ttl_seconds", &self.ttl_seconds)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceRecoveryRevokeArgs {
    db_path: String,
    grant_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceRotateEnvelopeArgs {
    db_path: String,
    device_id: String,
    reason: String,
    session_id: Option<String>,
    ttl_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncBlobArgs {
    db_path: String,
    cache_root: String,
    remote: SyncRemoteArgs,
    blob_id: String,
    object_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncSnapshotArgs {
    db_path: String,
    cache_root: String,
    remote: SyncRemoteArgs,
    metadata: SyncMetadataArgs,
    snapshot_id: String,
    mock_key_source_db: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncMaterializeArgs {
    db_path: String,
    cache_root: String,
    remote: SyncRemoteArgs,
    metadata: SyncMetadataArgs,
    target: String,
    snapshot_id: String,
    mock_key_source_db: Option<String>,
    apply: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncRemoteKindArg {
    Local,
    S3,
    Hosted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncMetadataModeArg {
    LocalMock,
    MockDevSqlite,
    HostedApi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncMetadataArgs {
    mode: SyncMetadataModeArg,
    db_path: Option<String>,
    account_id: Option<String>,
    project_id: Option<String>,
    endpoint: Option<String>,
    api: Option<String>,
    session_token_env: Option<String>,
}

impl Default for SyncMetadataArgs {
    fn default() -> Self {
        Self {
            mode: SyncMetadataModeArg::LocalMock,
            db_path: None,
            account_id: None,
            project_id: None,
            endpoint: None,
            api: None,
            session_token_env: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncRemoteArgs {
    kind: SyncRemoteKindArg,
    local_root: Option<String>,
    s3_endpoint: Option<String>,
    s3_bucket: Option<String>,
    s3_region: String,
    s3_region_explicit: bool,
    s3_prefix: Option<String>,
    s3_access_key_env: Option<String>,
    s3_secret_key_env: Option<String>,
    s3_session_token_env: Option<String>,
    object_access_api: Option<String>,
    object_access_project: Option<String>,
    object_access_lease: Option<String>,
    object_access_session_token_env: Option<String>,
}

impl Default for SyncRemoteArgs {
    fn default() -> Self {
        Self {
            kind: SyncRemoteKindArg::Local,
            local_root: None,
            s3_endpoint: None,
            s3_bucket: None,
            s3_region: "auto".to_string(),
            s3_region_explicit: false,
            s3_prefix: None,
            s3_access_key_env: None,
            s3_secret_key_env: None,
            s3_session_token_env: None,
            object_access_api: None,
            object_access_project: None,
            object_access_lease: None,
            object_access_session_token_env: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncRemoteCheckArgs {
    remote: SyncRemoteArgs,
    validate_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncPreflightArgs {
    db_path: String,
    project_id: String,
    base_snapshot_id: Option<String>,
    local_snapshot_id: String,
    incoming_snapshot_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncCursorArgs {
    db_path: String,
    project_id: String,
    device_id: Option<String>,
    value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetadataCheckArgs {
    endpoint: String,
    auth_mode: MetadataAuthMode,
}

#[derive(Clone, PartialEq, Eq)]
enum MetadataAdminStoreSelector {
    Sqlite { db_path: String },
    PostgresUrlEnv { env_name: String },
}

impl std::fmt::Debug for MetadataAdminStoreSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite { db_path } => f.debug_struct("Sqlite").field("db_path", db_path).finish(),
            Self::PostgresUrlEnv { env_name } => f
                .debug_struct("PostgresUrlEnv")
                .field("env_name", env_name)
                .field("database_url", &"<redacted>")
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MetadataAlphaInviteCreateArgs {
    store: MetadataAdminStoreSelector,
    email: Option<String>,
    domain: Option<String>,
    invite_code: Option<String>,
    ttl_seconds: i64,
}

impl std::fmt::Debug for MetadataAlphaInviteCreateArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataAlphaInviteCreateArgs")
            .field("store", &self.store)
            .field("email", &self.email)
            .field("domain", &self.domain)
            .field(
                "invite_code",
                &self.invite_code.as_ref().map(|_| "<redacted>"),
            )
            .field("ttl_seconds", &self.ttl_seconds)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MetadataCredentialLeaseArgs {
    store: MetadataAdminStoreSelector,
    session_token: String,
    account_id: Option<String>,
    verified_email: Option<String>,
    verified_domain: Option<String>,
    project_id: Option<String>,
    lease_id: String,
    provider_kind: ManagedObjectProviderKind,
    endpoint: String,
    bucket: String,
    region: String,
    prefix: Option<String>,
    capabilities: Vec<ManagedObjectCapability>,
    ttl_seconds: i64,
}

impl std::fmt::Debug for MetadataCredentialLeaseArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataCredentialLeaseArgs")
            .field("store", &self.store)
            .field("session_token", &"<redacted>")
            .field("account_id", &self.account_id)
            .field("verified_email", &self.verified_email)
            .field("verified_domain", &self.verified_domain)
            .field("project_id", &self.project_id)
            .field("lease_id", &self.lease_id)
            .field("provider_kind", &self.provider_kind)
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("prefix", &self.prefix)
            .field("capabilities", &self.capabilities)
            .field("ttl_seconds", &self.ttl_seconds)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MetadataCredentialLeaseLookupArgs {
    store: MetadataAdminStoreSelector,
    session_token: String,
    project_id: Option<String>,
    lease_id: String,
    required_capabilities: Vec<ManagedObjectCapability>,
}

impl std::fmt::Debug for MetadataCredentialLeaseLookupArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataCredentialLeaseLookupArgs")
            .field("store", &self.store)
            .field("session_token", &"<redacted>")
            .field("project_id", &self.project_id)
            .field("lease_id", &self.lease_id)
            .field("required_capabilities", &self.required_capabilities)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MetadataCredentialLeaseMutateArgs {
    store: MetadataAdminStoreSelector,
    session_token: String,
    project_id: Option<String>,
    lease_id: String,
}

impl std::fmt::Debug for MetadataCredentialLeaseMutateArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataCredentialLeaseMutateArgs")
            .field("store", &self.store)
            .field("session_token", &"<redacted>")
            .field("project_id", &self.project_id)
            .field("lease_id", &self.lease_id)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct MetadataObjectAccessResolveArgs {
    api: String,
    session_token_env: String,
    project_id: String,
    lease_id: String,
    required_capabilities: Vec<ManagedObjectCapability>,
}

impl std::fmt::Debug for MetadataObjectAccessResolveArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataObjectAccessResolveArgs")
            .field("api", &self.api)
            .field("session_token_env", &self.session_token_env)
            .field("project_id", &self.project_id)
            .field("lease_id", &self.lease_id)
            .field("required_capabilities", &self.required_capabilities)
            .finish()
    }
}

fn run_init(args: &[String]) -> ExitCode {
    match parse_init_args(args)
        .and_then(|args| init_identity(&args).map_err(|error| error.to_string()))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bindhub: {error}");
            eprintln!("Usage: bindhub init --db <DB_PATH> [--device-name <NAME>]");
            ExitCode::from(1)
        }
    }
}

fn run_auth(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("mock-login") | Some("login-dev") => {
            match parse_db_only_args(&args[1..], "auth mock-login")
                .and_then(|args| auth_mock_login(&args).map_err(|error| error.to_string()))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    eprintln!("Usage: bindhub auth mock-login --db <DB_PATH>");
                    ExitCode::from(1)
                }
            }
        }
        Some("status") => match parse_db_only_args(&args[1..], "auth status")
            .and_then(|args| auth_status(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!("Usage: bindhub auth status --db <DB_PATH>");
                ExitCode::from(1)
            }
        },
        Some("mock-verified-bootstrap") => {
            match parse_auth_mock_verified_bootstrap_args(&args[1..]).and_then(|args| {
                auth_mock_verified_bootstrap(&args).map_err(|error| error.to_string())
            }) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    eprintln!(
                        "Usage: bindhub auth mock-verified-bootstrap --db <DB_PATH> --verified-email <EMAIL>|--verified-domain <DOMAIN> --session-token <TOKEN> [--provider-kind <KIND>] [--provider-issuer <ISSUER>] [--provider-subject <SUBJECT>] [--ttl-seconds <SECONDS>] [--proof-ttl-seconds <SECONDS>]"
                    );
                    ExitCode::from(1)
                }
            }
        }
        Some("hosted-login") => match parse_auth_hosted_login_args(&args[1..])
            .and_then(|args| auth_hosted_login(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!(
                    "Usage: bindhub auth hosted-login --api <URL> --email <EMAIL> --invite-code-env <ENV>|--invite-code <CODE>"
                );
                ExitCode::from(1)
            }
        },
        Some("hosted-status") => match parse_auth_hosted_session_args(&args[1..], "hosted-status")
            .and_then(|args| auth_hosted_status(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!(
                    "Usage: bindhub auth hosted-status --api <URL> [--session-token-env <ENV>]"
                );
                ExitCode::from(1)
            }
        },
        Some("hosted-logout") => match parse_auth_hosted_session_args(&args[1..], "hosted-logout")
            .and_then(|args| auth_hosted_logout(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!(
                    "Usage: bindhub auth hosted-logout --api <URL> [--session-token-env <ENV>]"
                );
                ExitCode::from(1)
            }
        },
        Some("proof-check") => match parse_auth_proof_check_args(&args[1..])
            .and_then(|args| auth_proof_check(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!("Usage: bindhub auth proof-check --db <DB_PATH> --session-token <TOKEN>");
                ExitCode::from(1)
            }
        },
        Some("revoke-session") => match parse_auth_revoke_session_args(&args[1..])
            .and_then(|args| auth_revoke_session(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!("Usage: bindhub auth revoke-session --db <DB_PATH> <SESSION_ID>");
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!(
                "bindhub: auth requires mock-login, status, mock-verified-bootstrap, hosted-login, hosted-status, hosted-logout, proof-check, or revoke-session"
            );
            eprintln!("Usage:");
            eprintln!("  bindhub auth mock-login --db <DB_PATH>");
            eprintln!("  bindhub auth status --db <DB_PATH>");
            eprintln!(
                "  bindhub auth mock-verified-bootstrap --db <DB_PATH> --verified-email <EMAIL>|--verified-domain <DOMAIN> --session-token <TOKEN> [--provider-kind <KIND>] [--provider-issuer <ISSUER>] [--provider-subject <SUBJECT>] [--ttl-seconds <SECONDS>] [--proof-ttl-seconds <SECONDS>]"
            );
            eprintln!(
                "  bindhub auth hosted-login --api <URL> --email <EMAIL> --invite-code-env <ENV>|--invite-code <CODE>"
            );
            eprintln!("  bindhub auth hosted-status --api <URL> [--session-token-env <ENV>]");
            eprintln!("  bindhub auth hosted-logout --api <URL> [--session-token-env <ENV>]");
            eprintln!("  bindhub auth proof-check --db <DB_PATH> --session-token <TOKEN>");
            eprintln!("  bindhub auth revoke-session --db <DB_PATH> <SESSION_ID>");
            ExitCode::from(2)
        }
    }
}

fn run_devices(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("list") => match parse_devices_list_args(&args[1..])
            .and_then(|db_path| devices_list(&db_path).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!("Usage: bindhub devices list --db <DB_PATH>");
                ExitCode::from(1)
            }
        },
        Some("invite") => match parse_device_invite_args(&args[1..])
            .and_then(|args| devices_invite(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!("Usage: bindhub devices invite --db <DB_PATH> [--ttl-seconds <SECONDS>]");
                ExitCode::from(1)
            }
        },
        Some("approve") => match parse_device_approve_args(&args[1..])
            .and_then(|args| devices_approve(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!(
                    "Usage: bindhub devices approve --db <DB_PATH> --token <TOKEN> --device-name <NAME>"
                );
                ExitCode::from(1)
            }
        },
        Some("join") => match parse_device_join_args(&args[1..])
            .and_then(|args| devices_join(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!("Usage: bindhub devices join --db <RECEIVER_DB> --token-env <ENV>|--token <TOKEN> --device-name <NAME>");
                ExitCode::from(1)
            }
        },
        Some("approve-join") => match parse_device_approve_join_args(&args[1..])
            .and_then(|args| devices_approve_join(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!("Usage: bindhub devices approve-join --db <SOURCE_DB> --token-env <ENV>|--token <TOKEN> --join-request-env <ENV>|--join-request <REQUEST> --device-name <NAME>");
                ExitCode::from(1)
            }
        },
        Some("complete") => match parse_device_complete_args(&args[1..])
            .and_then(|args| devices_complete(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!("Usage: bindhub devices complete --db <RECEIVER_DB> --completion-env <ENV>|--completion <COMPLETION>");
                ExitCode::from(1)
            }
        },
        Some("revoke") => match parse_device_revoke_args(&args[1..])
            .and_then(|args| devices_revoke(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                eprintln!(
                    "Usage: bindhub devices revoke --db <DB_PATH> <DEVICE_ID> [--reason <TEXT>]"
                );
                ExitCode::from(1)
            }
        },
        Some("recovery") => match args.get(1).map(String::as_str) {
            Some("create") => match parse_device_recovery_grant_args(&args[2..]).and_then(|args| {
                devices_recovery_grant_create(&args).map_err(|error| error.to_string())
            }) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    print_devices_usage();
                    ExitCode::from(1)
                }
            },
            Some("revoke") => match parse_device_recovery_revoke_args(&args[2..]).and_then(|args| {
                devices_recovery_grant_revoke(&args).map_err(|error| error.to_string())
            }) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    print_devices_usage();
                    ExitCode::from(1)
                }
            },
            _ => {
                eprintln!("bindhub: devices recovery requires create or revoke");
                print_devices_usage();
                ExitCode::from(2)
            }
        },
        Some("rotate-key-envelope") => match parse_device_rotate_envelope_args(&args[1..])
            .and_then(|args| devices_rotate_key_envelope(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_devices_usage();
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("bindhub: devices requires list, invite, approve, join, approve-join, complete, revoke, recovery, or rotate-key-envelope");
            print_devices_usage();
            ExitCode::from(2)
        }
    }
}

fn run_metadata(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("check") => match parse_metadata_check_args(&args[1..])
            .and_then(|args| metadata_check(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_metadata_usage();
                ExitCode::from(1)
            }
        },
        Some("alpha-invite") => match args.get(1).map(String::as_str) {
            Some("create") => {
                match parse_metadata_alpha_invite_create_args(&args[2..]).and_then(|args| {
                    metadata_alpha_invite_create(&args).map_err(|error| error.to_string())
                }) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("bindhub: {error}");
                        print_metadata_usage();
                        ExitCode::from(1)
                    }
                }
            }
            _ => {
                eprintln!("bindhub: metadata alpha-invite requires create");
                print_metadata_usage();
                ExitCode::from(2)
            }
        },
        Some("credential-lease") => match args.get(1).map(String::as_str) {
            Some("mock-create") => {
                match parse_metadata_credential_lease_create_args(&args[2..]).and_then(|args| {
                    metadata_credential_lease_mock_create(&args).map_err(|error| error.to_string())
                }) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("bindhub: {error}");
                        print_metadata_usage();
                        ExitCode::from(1)
                    }
                }
            }
            Some("check") => {
                match parse_metadata_credential_lease_lookup_args(&args[2..]).and_then(|args| {
                    metadata_credential_lease_check(&args).map_err(|error| error.to_string())
                }) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("bindhub: {error}");
                        print_metadata_usage();
                        ExitCode::from(1)
                    }
                }
            }
            Some("revoke") => match parse_metadata_credential_lease_mutate_args(&args[2..])
                .and_then(|args| {
                    metadata_credential_lease_revoke(&args).map_err(|error| error.to_string())
                }) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    print_metadata_usage();
                    ExitCode::from(1)
                }
            },
            Some("rotate") => match parse_metadata_credential_lease_mutate_args(&args[2..])
                .and_then(|args| {
                    metadata_credential_lease_rotate(&args).map_err(|error| error.to_string())
                }) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    print_metadata_usage();
                    ExitCode::from(1)
                }
            },
            _ => {
                eprintln!(
                    "bindhub: metadata credential-lease requires mock-create, check, revoke, or rotate"
                );
                print_metadata_usage();
                ExitCode::from(2)
            }
        },
        Some("object-access") => match args.get(1).map(String::as_str) {
            Some("resolve") => {
                match parse_metadata_object_access_resolve_args(&args[2..]).and_then(|args| {
                    metadata_object_access_resolve(&args).map_err(|error| error.to_string())
                }) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("bindhub: {error}");
                        print_metadata_usage();
                        ExitCode::from(1)
                    }
                }
            }
            _ => {
                eprintln!("bindhub: metadata object-access requires resolve");
                print_metadata_usage();
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("bindhub: metadata requires check, credential-lease, or object-access");
            print_metadata_usage();
            ExitCode::from(2)
        }
    }
}

fn run_sync(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("run-loop") => product::run_loom_daemon_entrypoint(&args[1..]),
        Some("publish-snapshot") => match parse_sync_snapshot_args(&args[1..], false)
            .and_then(|args| sync_publish_snapshot(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("import-snapshot") => match parse_sync_snapshot_args(&args[1..], true)
            .and_then(|args| sync_import_snapshot(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("materialize") => match parse_sync_materialize_args(&args[1..])
            .and_then(|args| sync_materialize(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("preflight") => match parse_sync_preflight_args(&args[1..])
            .and_then(|args| sync_preflight_command(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("upload") => match parse_sync_blob_args(&args[1..])
            .and_then(|args| sync_upload(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("download") => match parse_sync_blob_args(&args[1..])
            .and_then(|args| sync_download(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("remote") => match args.get(1).map(String::as_str) {
            Some("check") => match parse_sync_remote_check_args(&args[2..])
                .and_then(|args| sync_remote_check(&args).map_err(|error| error.to_string()))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    print_sync_usage();
                    ExitCode::from(1)
                }
            },
            _ => {
                eprintln!("bindhub: sync remote requires check");
                print_sync_usage();
                ExitCode::from(2)
            }
        },
        Some("cursor") => match args.get(1).map(String::as_str) {
            Some("get") => match parse_sync_cursor_args(&args[2..], false)
                .and_then(|args| sync_cursor_get(&args).map_err(|error| error.to_string()))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    print_sync_usage();
                    ExitCode::from(1)
                }
            },
            Some("set") => match parse_sync_cursor_args(&args[2..], true)
                .and_then(|args| sync_cursor_set(&args).map_err(|error| error.to_string()))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("bindhub: {error}");
                    print_sync_usage();
                    ExitCode::from(1)
                }
            },
            _ => {
                eprintln!("bindhub: sync cursor requires get or set");
                print_sync_usage();
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!(
                "bindhub: sync requires publish-snapshot, import-snapshot, materialize, preflight, upload, download, remote, or cursor"
            );
            print_sync_usage();
            ExitCode::from(2)
        }
    }
}

fn parse_init_args(args: &[String]) -> Result<InitArgs, String> {
    let mut db_path = None;
    let mut device_name = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--device-name" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--device-name requires a name".to_string())?;
                device_name = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown init option '{value}'"));
            }
            _ => return Err("init accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(InitArgs {
        db_path: db_path.ok_or_else(|| "init requires --db <DB_PATH>".to_string())?,
        device_name,
    })
}

fn parse_devices_list_args(args: &[String]) -> Result<String, String> {
    let [flag, db_path] = args else {
        return Err("devices list requires --db <DB_PATH>".to_string());
    };
    if flag != "--db" {
        return Err("devices list requires --db <DB_PATH>".to_string());
    }
    Ok(db_path.clone())
}

fn parse_db_only_args(args: &[String], command: &str) -> Result<DbOnlyArgs, String> {
    let [flag, db_path] = args else {
        return Err(format!("{command} requires --db <DB_PATH>"));
    };
    if flag != "--db" {
        return Err(format!("{command} requires --db <DB_PATH>"));
    }
    Ok(DbOnlyArgs {
        db_path: db_path.clone(),
    })
}

fn parse_auth_mock_verified_bootstrap_args(
    args: &[String],
) -> Result<AuthMockVerifiedBootstrapArgs, String> {
    let mut db_path = None;
    let mut provider_kind = "oidc-dev".to_string();
    let mut provider_issuer = "https://bindhub.local/mock-oidc".to_string();
    let mut provider_subject = None;
    let mut verified_email = None;
    let mut verified_domain = None;
    let mut session_token = None;
    let mut ttl_seconds = 3_600_i64;
    let mut proof_ttl_seconds = 86_400_i64;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--provider-kind" => {
                index += 1;
                provider_kind = args
                    .get(index)
                    .ok_or_else(|| "--provider-kind requires a value".to_string())?
                    .clone();
            }
            "--provider-issuer" => {
                index += 1;
                provider_issuer = args
                    .get(index)
                    .ok_or_else(|| "--provider-issuer requires a value".to_string())?
                    .clone();
            }
            "--provider-subject" => {
                index += 1;
                provider_subject = args.get(index).cloned();
            }
            "--verified-email" => {
                index += 1;
                verified_email = args.get(index).cloned();
            }
            "--verified-domain" => {
                index += 1;
                verified_domain = args.get(index).cloned();
            }
            "--session-token" => {
                index += 1;
                session_token = args.get(index).cloned();
            }
            "--ttl-seconds" => {
                index += 1;
                ttl_seconds = parse_positive_i64(args.get(index), "--ttl-seconds")?;
            }
            "--proof-ttl-seconds" => {
                index += 1;
                proof_ttl_seconds = parse_positive_i64(args.get(index), "--proof-ttl-seconds")?;
            }
            value if value.starts_with('-') => {
                return Err(format!(
                    "unknown auth mock-verified-bootstrap option '{value}'"
                ));
            }
            _ => return Err("auth mock-verified-bootstrap accepts only flags".to_string()),
        }

        index += 1;
    }

    if verified_email.is_none() && verified_domain.is_none() {
        return Err(
            "auth mock-verified-bootstrap requires --verified-email or --verified-domain"
                .to_string(),
        );
    }

    Ok(AuthMockVerifiedBootstrapArgs {
        db_path: db_path
            .ok_or_else(|| "auth mock-verified-bootstrap requires --db <DB_PATH>".to_string())?,
        provider_kind,
        provider_issuer,
        provider_subject,
        verified_email,
        verified_domain,
        session_token: session_token.ok_or_else(|| {
            "auth mock-verified-bootstrap requires --session-token <TOKEN>".to_string()
        })?,
        ttl_seconds,
        proof_ttl_seconds,
    })
}

fn parse_auth_proof_check_args(args: &[String]) -> Result<AuthProofCheckArgs, String> {
    let mut db_path = None;
    let mut session_token = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--session-token" => {
                index += 1;
                session_token = args.get(index).cloned();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown auth proof-check option '{value}'"));
            }
            _ => return Err("auth proof-check accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(AuthProofCheckArgs {
        db_path: db_path.ok_or_else(|| "auth proof-check requires --db <DB_PATH>".to_string())?,
        session_token: session_token
            .ok_or_else(|| "auth proof-check requires --session-token <TOKEN>".to_string())?,
    })
}

fn parse_auth_hosted_login_args(args: &[String]) -> Result<AuthHostedLoginArgs, String> {
    let mut api = None;
    let mut email = None;
    let mut invite_code = None;
    let mut invite_code_env = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--api" => {
                index += 1;
                api = args.get(index).cloned();
            }
            "--email" => {
                index += 1;
                email = args.get(index).cloned();
            }
            "--invite-code" => {
                index += 1;
                invite_code = args.get(index).cloned();
            }
            "--invite-code-env" => {
                index += 1;
                invite_code_env = args.get(index).cloned();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown auth hosted-login option '{value}'"));
            }
            _ => return Err("auth hosted-login accepts only flags".to_string()),
        }

        index += 1;
    }

    if invite_code.is_some() == invite_code_env.is_some() {
        return Err(
            "auth hosted-login requires exactly one of --invite-code or --invite-code-env"
                .to_string(),
        );
    }

    Ok(AuthHostedLoginArgs {
        api: api.ok_or_else(|| "auth hosted-login requires --api <URL>".to_string())?,
        email: email.ok_or_else(|| "auth hosted-login requires --email <EMAIL>".to_string())?,
        invite_code,
        invite_code_env,
    })
}

fn parse_auth_hosted_session_args(
    args: &[String],
    command: &'static str,
) -> Result<AuthHostedSessionArgs, String> {
    let mut api = None;
    let mut session_token_env = "BINDHUB_SESSION_TOKEN".to_string();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--api" => {
                index += 1;
                api = args.get(index).cloned();
            }
            "--session-token-env" => {
                index += 1;
                session_token_env = args
                    .get(index)
                    .ok_or_else(|| "--session-token-env requires <ENV>".to_string())?
                    .clone();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown auth {command} option '{value}'"));
            }
            _ => return Err(format!("auth {command} accepts only flags")),
        }

        index += 1;
    }

    Ok(AuthHostedSessionArgs {
        api: api.ok_or_else(|| format!("auth {command} requires --api <URL>"))?,
        session_token_env,
    })
}

fn parse_auth_revoke_session_args(args: &[String]) -> Result<AuthRevokeSessionArgs, String> {
    let mut db_path = None;
    let mut session_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown auth revoke-session option '{value}'"));
            }
            value => {
                if session_id.replace(value.to_string()).is_some() {
                    return Err("auth revoke-session accepts exactly one session id".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(AuthRevokeSessionArgs {
        db_path: db_path
            .ok_or_else(|| "auth revoke-session requires --db <DB_PATH>".to_string())?,
        session_id: session_id
            .ok_or_else(|| "auth revoke-session requires a session id".to_string())?,
    })
}

fn parse_positive_i64(value: Option<&String>, flag: &str) -> Result<i64, String> {
    let value = value.ok_or_else(|| format!("{flag} requires a positive integer"))?;
    let parsed = value
        .parse::<i64>()
        .map_err(|_| format!("{flag} requires a positive integer"))?;
    if parsed <= 0 {
        return Err(format!("{flag} requires a positive integer"));
    }
    Ok(parsed)
}

fn parse_device_invite_args(args: &[String]) -> Result<DeviceInviteArgs, String> {
    let mut db_path = None;
    let mut ttl_seconds = 600_i64;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--ttl-seconds" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--ttl-seconds requires a positive integer".to_string())?;
                ttl_seconds = value
                    .parse::<i64>()
                    .map_err(|_| "--ttl-seconds requires a positive integer".to_string())?;
                if ttl_seconds <= 0 {
                    return Err("--ttl-seconds requires a positive integer".to_string());
                }
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown devices invite option '{value}'"));
            }
            _ => return Err("devices invite accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(DeviceInviteArgs {
        db_path: db_path.ok_or_else(|| "devices invite requires --db <DB_PATH>".to_string())?,
        ttl_seconds,
    })
}

fn parse_device_approve_args(args: &[String]) -> Result<DeviceApproveArgs, String> {
    let mut db_path = None;
    let mut token = None;
    let mut device_name = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--token" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--token requires an invitation token".to_string())?;
                token = Some(value.clone());
            }
            "--device-name" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--device-name requires a name".to_string())?;
                device_name = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown devices approve option '{value}'"));
            }
            _ => return Err("devices approve accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(DeviceApproveArgs {
        db_path: db_path.ok_or_else(|| "devices approve requires --db <DB_PATH>".to_string())?,
        token: token.ok_or_else(|| "devices approve requires --token <TOKEN>".to_string())?,
        device_name: device_name
            .ok_or_else(|| "devices approve requires --device-name <NAME>".to_string())?,
    })
}

fn parse_device_join_args(args: &[String]) -> Result<DeviceJoinArgs, String> {
    let mut db_path = None;
    let mut token = None;
    let mut token_env = None;
    let mut device_name = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--token" => {
                index += 1;
                token = args.get(index).cloned();
            }
            "--token-env" => {
                index += 1;
                token_env = args.get(index).cloned();
            }
            "--device-name" => {
                index += 1;
                device_name = args.get(index).cloned();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown devices join option '{value}'"));
            }
            _ => return Err("devices join accepts only flags".to_string()),
        }

        index += 1;
    }

    if token.is_some() == token_env.is_some() {
        return Err("devices join requires exactly one of --token or --token-env".to_string());
    }

    Ok(DeviceJoinArgs {
        db_path: db_path.ok_or_else(|| "devices join requires --db <DB_PATH>".to_string())?,
        token,
        token_env,
        device_name: device_name
            .ok_or_else(|| "devices join requires --device-name <NAME>".to_string())?,
    })
}

fn parse_device_approve_join_args(args: &[String]) -> Result<DeviceApproveJoinArgs, String> {
    let mut db_path = None;
    let mut token = None;
    let mut token_env = None;
    let mut join_request = None;
    let mut join_request_env = None;
    let mut device_name = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--token" => {
                index += 1;
                token = args.get(index).cloned();
            }
            "--token-env" => {
                index += 1;
                token_env = args.get(index).cloned();
            }
            "--join-request" => {
                index += 1;
                join_request = args.get(index).cloned();
            }
            "--join-request-env" => {
                index += 1;
                join_request_env = args.get(index).cloned();
            }
            "--device-name" => {
                index += 1;
                device_name = args.get(index).cloned();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown devices approve-join option '{value}'"));
            }
            _ => return Err("devices approve-join accepts only flags".to_string()),
        }

        index += 1;
    }

    if token.is_some() == token_env.is_some() {
        return Err(
            "devices approve-join requires exactly one of --token or --token-env".to_string(),
        );
    }

    if join_request.is_some() == join_request_env.is_some() {
        return Err(
            "devices approve-join requires exactly one of --join-request or --join-request-env"
                .to_string(),
        );
    }

    Ok(DeviceApproveJoinArgs {
        db_path: db_path
            .ok_or_else(|| "devices approve-join requires --db <DB_PATH>".to_string())?,
        token,
        token_env,
        join_request,
        join_request_env,
        device_name: device_name
            .ok_or_else(|| "devices approve-join requires --device-name <NAME>".to_string())?,
    })
}

fn parse_device_complete_args(args: &[String]) -> Result<DeviceCompleteArgs, String> {
    let mut db_path = None;
    let mut completion = None;
    let mut completion_env = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--completion" => {
                index += 1;
                completion = args.get(index).cloned();
            }
            "--completion-env" => {
                index += 1;
                completion_env = args.get(index).cloned();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown devices complete option '{value}'"));
            }
            _ => return Err("devices complete accepts only flags".to_string()),
        }

        index += 1;
    }

    if completion.is_some() == completion_env.is_some() {
        return Err(
            "devices complete requires exactly one of --completion or --completion-env".to_string(),
        );
    }

    Ok(DeviceCompleteArgs {
        db_path: db_path.ok_or_else(|| "devices complete requires --db <DB_PATH>".to_string())?,
        completion,
        completion_env,
    })
}

fn parse_device_revoke_args(args: &[String]) -> Result<DeviceRevokeArgs, String> {
    let mut db_path = None;
    let mut reason = None;
    let mut device_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--reason" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--reason requires text".to_string())?;
                reason = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown devices revoke option '{value}'"));
            }
            value => {
                if device_id.replace(value.to_string()).is_some() {
                    return Err("devices revoke accepts exactly one device id".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(DeviceRevokeArgs {
        db_path: db_path.ok_or_else(|| "devices revoke requires --db <DB_PATH>".to_string())?,
        device_id: device_id.ok_or_else(|| "devices revoke requires a device id".to_string())?,
        reason,
    })
}

fn parse_device_recovery_grant_args(args: &[String]) -> Result<DeviceRecoveryGrantArgs, String> {
    let mut db_path = None;
    let mut device_id = None;
    let mut recovery_ref = None;
    let mut audit_label = "recovery grant".to_string();
    let mut ttl_seconds = 600_i64;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--device" => {
                index += 1;
                device_id = args.get(index).cloned();
            }
            "--recovery-ref" => {
                index += 1;
                recovery_ref = args.get(index).cloned();
            }
            "--audit-label" => {
                index += 1;
                audit_label = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--audit-label requires text".to_string())?;
            }
            "--ttl-seconds" => {
                index += 1;
                ttl_seconds = parse_positive_i64(args.get(index), "--ttl-seconds")?;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown devices recovery create option '{value}'"));
            }
            _ => return Err("devices recovery create accepts only flags".to_string()),
        }
        index += 1;
    }

    Ok(DeviceRecoveryGrantArgs {
        db_path: db_path
            .ok_or_else(|| "devices recovery create requires --db <DB_PATH>".to_string())?,
        device_id: device_id
            .ok_or_else(|| "devices recovery create requires --device <DEVICE_ID>".to_string())?,
        recovery_ref: recovery_ref.ok_or_else(|| {
            "devices recovery create requires --recovery-ref <REDACTED_REF>".to_string()
        })?,
        audit_label,
        ttl_seconds,
    })
}

fn parse_device_recovery_revoke_args(args: &[String]) -> Result<DeviceRecoveryRevokeArgs, String> {
    let mut db_path = None;
    let mut grant_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown devices recovery revoke option '{value}'"));
            }
            value => {
                if grant_id.replace(value.to_string()).is_some() {
                    return Err("devices recovery revoke accepts exactly one grant id".to_string());
                }
            }
        }
        index += 1;
    }

    Ok(DeviceRecoveryRevokeArgs {
        db_path: db_path
            .ok_or_else(|| "devices recovery revoke requires --db <DB_PATH>".to_string())?,
        grant_id: grant_id
            .ok_or_else(|| "devices recovery revoke requires a grant id".to_string())?,
    })
}

fn parse_device_rotate_envelope_args(args: &[String]) -> Result<DeviceRotateEnvelopeArgs, String> {
    let mut db_path = None;
    let mut device_id = None;
    let mut reason = "recovery rotation".to_string();
    let mut session_id = None;
    let mut ttl_seconds = 600_i64;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--device" => {
                index += 1;
                device_id = args.get(index).cloned();
            }
            "--reason" => {
                index += 1;
                reason = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--reason requires text".to_string())?;
            }
            "--session-id" => {
                index += 1;
                session_id = args.get(index).cloned();
            }
            "--ttl-seconds" => {
                index += 1;
                ttl_seconds = parse_positive_i64(args.get(index), "--ttl-seconds")?;
            }
            value if value.starts_with('-') => {
                return Err(format!(
                    "unknown devices rotate-key-envelope option '{value}'"
                ));
            }
            _ => return Err("devices rotate-key-envelope accepts only flags".to_string()),
        }
        index += 1;
    }

    Ok(DeviceRotateEnvelopeArgs {
        db_path: db_path
            .ok_or_else(|| "devices rotate-key-envelope requires --db <DB_PATH>".to_string())?,
        device_id: device_id.ok_or_else(|| {
            "devices rotate-key-envelope requires --device <DEVICE_ID>".to_string()
        })?,
        reason,
        session_id,
        ttl_seconds,
    })
}

fn parse_sync_blob_args(args: &[String]) -> Result<SyncBlobArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut remote = SyncRemoteArgs::default();
    let mut object_key = None;
    let mut blob_id = None;
    let mut index = 0;

    while index < args.len() {
        let current = args[index].as_str();
        if parse_sync_remote_arg(current, args, &mut index, &mut remote)? {
            index += 1;
            continue;
        }
        match current {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--cache" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--cache requires a path".to_string())?;
                cache_root = Some(value.clone());
            }
            "--object-key" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--object-key requires a key".to_string())?;
                object_key = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown sync option '{value}'"));
            }
            value => {
                if blob_id.replace(value.to_string()).is_some() {
                    return Err("sync accepts exactly one blob id".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(SyncBlobArgs {
        db_path: db_path.ok_or_else(|| "sync requires --db <DB_PATH>".to_string())?,
        cache_root: cache_root.ok_or_else(|| "sync requires --cache <CACHE_ROOT>".to_string())?,
        remote: finalize_sync_remote(remote, "sync")?,
        blob_id: blob_id.ok_or_else(|| "sync requires a blob id".to_string())?,
        object_key,
    })
}

fn parse_sync_snapshot_args(
    args: &[String],
    allow_mock_key_source: bool,
) -> Result<SyncSnapshotArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut remote = SyncRemoteArgs::default();
    let mut metadata = SyncMetadataArgs::default();
    let mut snapshot_id = None;
    let mut mock_key_source_db = None;
    let mut index = 0;

    while index < args.len() {
        let current = args[index].as_str();
        if parse_sync_remote_arg(current, args, &mut index, &mut remote)? {
            index += 1;
            continue;
        }
        if parse_sync_metadata_arg(current, args, &mut index, &mut metadata)? {
            index += 1;
            continue;
        }
        match current {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--cache" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--cache requires a path".to_string())?;
                cache_root = Some(value.clone());
            }
            "--mock-key-source-db" if allow_mock_key_source => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--mock-key-source-db requires a path".to_string())?;
                mock_key_source_db = Some(value.clone());
            }
            "--mock-key-source-db" => {
                return Err(
                    "--mock-key-source-db is only accepted for import/materialize".to_string(),
                );
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown sync snapshot option '{value}'"));
            }
            value => {
                if snapshot_id.replace(value.to_string()).is_some() {
                    return Err("sync snapshot commands accept exactly one snapshot id".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(SyncSnapshotArgs {
        db_path: db_path.ok_or_else(|| "sync snapshot requires --db <DB_PATH>".to_string())?,
        cache_root: cache_root
            .ok_or_else(|| "sync snapshot requires --cache <CACHE_ROOT>".to_string())?,
        remote: finalize_sync_remote(remote, "sync snapshot")?,
        metadata: finalize_sync_metadata(metadata, "sync snapshot", allow_mock_key_source)?,
        snapshot_id: snapshot_id
            .ok_or_else(|| "sync snapshot requires a snapshot id".to_string())?,
        mock_key_source_db,
    })
}

fn parse_sync_materialize_args(args: &[String]) -> Result<SyncMaterializeArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut remote = SyncRemoteArgs::default();
    let mut metadata = SyncMetadataArgs::default();
    let mut target = None;
    let mut snapshot_id = None;
    let mut mock_key_source_db = None;
    let mut apply = false;
    let mut mode_seen = false;
    let mut index = 0;

    while index < args.len() {
        let current = args[index].as_str();
        if parse_sync_remote_arg(current, args, &mut index, &mut remote)? {
            index += 1;
            continue;
        }
        if parse_sync_metadata_arg(current, args, &mut index, &mut metadata)? {
            index += 1;
            continue;
        }
        match current {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--cache" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--cache requires a path".to_string())?;
                cache_root = Some(value.clone());
            }
            "--to" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--to requires a target directory".to_string())?;
                target = Some(value.clone());
            }
            "--mock-key-source-db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--mock-key-source-db requires a path".to_string())?;
                mock_key_source_db = Some(value.clone());
            }
            "--apply" => {
                if mode_seen {
                    return Err("choose only one of --dry-run or --apply".to_string());
                }
                mode_seen = true;
                apply = true;
            }
            "--dry-run" => {
                if mode_seen {
                    return Err("choose only one of --dry-run or --apply".to_string());
                }
                mode_seen = true;
                apply = false;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown sync materialize option '{value}'"));
            }
            value => {
                if snapshot_id.replace(value.to_string()).is_some() {
                    return Err("sync materialize accepts exactly one snapshot id".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(SyncMaterializeArgs {
        db_path: db_path.ok_or_else(|| "sync materialize requires --db <DB_PATH>".to_string())?,
        cache_root: cache_root
            .ok_or_else(|| "sync materialize requires --cache <CACHE_ROOT>".to_string())?,
        remote: finalize_sync_remote(remote, "sync materialize")?,
        metadata: finalize_sync_metadata(metadata, "sync materialize", true)?,
        target: target.ok_or_else(|| "sync materialize requires --to <TARGET_DIR>".to_string())?,
        snapshot_id: snapshot_id
            .ok_or_else(|| "sync materialize requires a snapshot id".to_string())?,
        mock_key_source_db,
        apply,
    })
}

fn parse_sync_remote_check_args(args: &[String]) -> Result<SyncRemoteCheckArgs, String> {
    let mut remote = SyncRemoteArgs::default();
    let mut validate_only = false;
    let mut index = 0;

    while index < args.len() {
        let current = args[index].as_str();
        if parse_sync_remote_arg(current, args, &mut index, &mut remote)? {
            index += 1;
            continue;
        }
        match current {
            "--validate-only" => {
                validate_only = true;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown sync remote check option '{value}'"));
            }
            _ => return Err("sync remote check accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(SyncRemoteCheckArgs {
        remote: finalize_sync_remote(remote, "sync remote check")?,
        validate_only,
    })
}

fn parse_metadata_check_args(args: &[String]) -> Result<MetadataCheckArgs, String> {
    let mut endpoint = None;
    let mut auth_mode = MetadataAuthMode::MockDevHeaders;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--endpoint" => {
                index += 1;
                endpoint = args.get(index).cloned();
            }
            "--auth-mode" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    "--auth-mode requires mock-dev-headers or account-session".to_string()
                })?;
                auth_mode = match value.as_str() {
                    "mock-dev-headers" => MetadataAuthMode::MockDevHeaders,
                    "account-session" => MetadataAuthMode::AccountSession,
                    _ => {
                        return Err(
                            "--auth-mode requires mock-dev-headers or account-session".to_string()
                        )
                    }
                };
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown metadata check option '{value}'"));
            }
            _ => return Err("metadata check accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(MetadataCheckArgs {
        endpoint: endpoint.ok_or_else(|| "metadata check requires --endpoint <URL>".to_string())?,
        auth_mode,
    })
}

fn parse_metadata_alpha_invite_create_args(
    args: &[String],
) -> Result<MetadataAlphaInviteCreateArgs, String> {
    let mut db_path = None;
    let mut postgres_url_env = None;
    let mut email = None;
    let mut domain = None;
    let mut invite_code = None;
    let mut ttl_seconds = 60 * 60 * 24 * 14;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--postgres-url-env" => {
                index += 1;
                postgres_url_env = args.get(index).cloned();
            }
            "--email" => {
                index += 1;
                email = args.get(index).cloned();
            }
            "--domain" => {
                index += 1;
                domain = args.get(index).cloned();
            }
            "--invite-code" => {
                index += 1;
                invite_code = args.get(index).cloned();
            }
            "--ttl-seconds" => {
                index += 1;
                ttl_seconds = args
                    .get(index)
                    .ok_or_else(|| "--ttl-seconds requires <SECONDS>".to_string())?
                    .parse()
                    .map_err(|_| "--ttl-seconds requires an integer".to_string())?;
            }
            value if value.starts_with('-') => {
                return Err(format!(
                    "unknown metadata alpha-invite create option '{value}'"
                ));
            }
            _ => return Err("metadata alpha-invite create accepts only flags".to_string()),
        }

        index += 1;
    }

    if email.is_none() && domain.is_none() {
        return Err("metadata alpha-invite create requires --email or --domain".to_string());
    }
    if ttl_seconds <= 0 {
        return Err("metadata alpha-invite create requires a positive --ttl-seconds".to_string());
    }

    Ok(MetadataAlphaInviteCreateArgs {
        store: parse_metadata_admin_store_selector(
            db_path,
            postgres_url_env,
            "metadata alpha-invite create",
        )?,
        email,
        domain,
        invite_code,
        ttl_seconds,
    })
}

fn parse_metadata_credential_lease_create_args(
    args: &[String],
) -> Result<MetadataCredentialLeaseArgs, String> {
    let mut db_path = None;
    let mut postgres_url_env = None;
    let mut session_token = None;
    let mut account_id = None;
    let mut verified_email = None;
    let mut verified_domain = None;
    let mut project_id = None;
    let mut lease_id = None;
    let mut provider_kind = ManagedObjectProviderKind::R2;
    let mut endpoint = None;
    let mut bucket = None;
    let mut region = "auto".to_string();
    let mut prefix = None;
    let mut capabilities = vec![
        ManagedObjectCapability::Read,
        ManagedObjectCapability::Write,
        ManagedObjectCapability::List,
        ManagedObjectCapability::Head,
    ];
    let mut ttl_seconds = 3600_i64;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--postgres-url-env" => {
                index += 1;
                postgres_url_env = args.get(index).cloned();
            }
            "--session-token" => {
                index += 1;
                session_token = args.get(index).cloned();
            }
            "--account" => {
                index += 1;
                account_id = args.get(index).cloned();
            }
            "--verified-email" => {
                index += 1;
                verified_email = args.get(index).cloned();
            }
            "--verified-domain" => {
                index += 1;
                verified_domain = args.get(index).cloned();
            }
            "--project" => {
                index += 1;
                project_id = Some(parse_managed_credential_project_arg(args.get(index))?);
            }
            "--lease" => {
                index += 1;
                lease_id = args.get(index).cloned();
            }
            "--provider-kind" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    "--provider-kind requires r2, s3, or minio-compatible".to_string()
                })?;
                provider_kind = value
                    .parse()
                    .map_err(|error: bindhub_metadata::MetadataError| error.to_string())?;
            }
            "--endpoint" => {
                index += 1;
                endpoint = args.get(index).cloned();
            }
            "--bucket" => {
                index += 1;
                bucket = args.get(index).cloned();
            }
            "--region" => {
                index += 1;
                region = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--region requires a value".to_string())?;
            }
            "--prefix" => {
                index += 1;
                prefix = args.get(index).cloned();
            }
            "--capabilities" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--capabilities requires a comma-separated list".to_string())?;
                capabilities = parse_managed_object_capabilities(value)?;
            }
            "--ttl-seconds" => {
                index += 1;
                ttl_seconds = parse_positive_i64(args.get(index), "--ttl-seconds")?;
            }
            value if value.starts_with('-') => {
                return Err(format!(
                    "unknown metadata credential-lease option '{value}'"
                ));
            }
            _ => return Err("metadata credential-lease accepts only flags".to_string()),
        }
        index += 1;
    }

    if verified_email.is_none() && verified_domain.is_none() {
        return Err(
            "metadata credential-lease mock-create requires --verified-email or --verified-domain"
                .to_string(),
        );
    }
    if ttl_seconds <= 0 {
        return Err("--ttl-seconds must be positive".to_string());
    }

    Ok(MetadataCredentialLeaseArgs {
        store: parse_metadata_admin_store_selector(
            db_path,
            postgres_url_env,
            "metadata credential-lease",
        )?,
        session_token: session_token.ok_or_else(|| {
            "metadata credential-lease requires --session-token <TOKEN>".to_string()
        })?,
        account_id,
        verified_email,
        verified_domain,
        project_id,
        lease_id: lease_id.unwrap_or_else(|| "lease-dev-managed-object".to_string()),
        provider_kind,
        endpoint: endpoint
            .ok_or_else(|| "metadata credential-lease requires --endpoint <URL>".to_string())?,
        bucket: bucket
            .ok_or_else(|| "metadata credential-lease requires --bucket <BUCKET>".to_string())?,
        region,
        prefix,
        capabilities,
        ttl_seconds,
    })
}

fn parse_metadata_credential_lease_lookup_args(
    args: &[String],
) -> Result<MetadataCredentialLeaseLookupArgs, String> {
    let mut base = parse_metadata_credential_lease_mutate_args(args)?;
    let mut required_capabilities = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--require-capabilities" {
            index += 1;
            let value = args.get(index).ok_or_else(|| {
                "--require-capabilities requires a comma-separated list".to_string()
            })?;
            required_capabilities = parse_managed_object_capabilities(value)?;
        }
        index += 1;
    }
    Ok(MetadataCredentialLeaseLookupArgs {
        store: base.store,
        session_token: base.session_token,
        project_id: base.project_id.take(),
        lease_id: base.lease_id,
        required_capabilities,
    })
}

fn parse_metadata_credential_lease_mutate_args(
    args: &[String],
) -> Result<MetadataCredentialLeaseMutateArgs, String> {
    let mut db_path = None;
    let mut postgres_url_env = None;
    let mut session_token = None;
    let mut project_id = None;
    let mut lease_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--postgres-url-env" => {
                index += 1;
                postgres_url_env = args.get(index).cloned();
            }
            "--session-token" => {
                index += 1;
                session_token = args.get(index).cloned();
            }
            "--project" => {
                index += 1;
                project_id = Some(parse_managed_credential_project_arg(args.get(index))?);
            }
            "--lease" => {
                index += 1;
                lease_id = args.get(index).cloned();
            }
            "--require-capabilities" => {
                index += 1;
                if args.get(index).is_none() {
                    return Err(
                        "--require-capabilities requires a comma-separated list".to_string()
                    );
                }
            }
            value if value.starts_with('-') => {
                return Err(format!(
                    "unknown metadata credential-lease option '{value}'"
                ));
            }
            _ => return Err("metadata credential-lease accepts only flags".to_string()),
        }
        index += 1;
    }

    Ok(MetadataCredentialLeaseMutateArgs {
        store: parse_metadata_admin_store_selector(
            db_path,
            postgres_url_env,
            "metadata credential-lease",
        )?,
        session_token: session_token.ok_or_else(|| {
            "metadata credential-lease requires --session-token <TOKEN>".to_string()
        })?,
        project_id,
        lease_id: lease_id
            .ok_or_else(|| "metadata credential-lease requires --lease <LEASE_ID>".to_string())?,
    })
}

fn parse_metadata_admin_store_selector(
    db_path: Option<String>,
    postgres_url_env: Option<String>,
    command: &'static str,
) -> Result<MetadataAdminStoreSelector, String> {
    match (db_path, postgres_url_env) {
        (Some(_), Some(_)) => Err(format!(
            "{command} accepts only one of --db <METADATA_DB> or --postgres-url-env <ENV>"
        )),
        (Some(db_path), None) => Ok(MetadataAdminStoreSelector::Sqlite { db_path }),
        (None, Some(env_name)) => {
            bindhub_metadata::validate_env_reference_name(env_name, "postgres url env")
                .map(|env_name| MetadataAdminStoreSelector::PostgresUrlEnv { env_name })
                .map_err(|error| error.to_string())
        }
        (None, None) => Err(format!(
            "{command} requires --db <METADATA_DB> or --postgres-url-env <ENV>"
        )),
    }
}

fn open_metadata_admin_store(
    selector: &MetadataAdminStoreSelector,
) -> Result<Box<dyn MetadataStore>, Box<dyn std::error::Error>> {
    match selector {
        MetadataAdminStoreSelector::Sqlite { db_path } => {
            Ok(Box::new(SqliteMetadataStore::open_file(db_path)?))
        }
        MetadataAdminStoreSelector::PostgresUrlEnv { env_name } => {
            let database_url = secret_from_env(env_name, "postgres database url")?;
            Ok(Box::new(PostgresMetadataStore::connect(&database_url)?))
        }
    }
}

fn parse_metadata_object_access_resolve_args(
    args: &[String],
) -> Result<MetadataObjectAccessResolveArgs, String> {
    let mut api = None;
    let mut session_token_env = "BINDHUB_SESSION_TOKEN".to_string();
    let mut project_id = None;
    let mut lease_id = None;
    let mut required_capabilities = vec![
        ManagedObjectCapability::Read,
        ManagedObjectCapability::Write,
        ManagedObjectCapability::List,
        ManagedObjectCapability::Head,
    ];
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--api" => {
                index += 1;
                api = args.get(index).cloned();
            }
            "--session-token-env" => {
                index += 1;
                session_token_env = args
                    .get(index)
                    .ok_or_else(|| "--session-token-env requires <ENV>".to_string())?
                    .clone();
            }
            "--project" => {
                index += 1;
                project_id = Some(parse_managed_credential_project_arg(args.get(index))?);
            }
            "--lease" => {
                index += 1;
                lease_id = args.get(index).cloned();
            }
            "--require-capabilities" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    "--require-capabilities requires a comma-separated list".to_string()
                })?;
                required_capabilities = parse_managed_object_capabilities(value)?;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown metadata object-access option '{value}'"));
            }
            _ => return Err("metadata object-access accepts only flags".to_string()),
        }
        index += 1;
    }

    Ok(MetadataObjectAccessResolveArgs {
        api: api
            .ok_or_else(|| "metadata object-access resolve requires --api <URL>".to_string())?,
        session_token_env,
        project_id: project_id.ok_or_else(|| {
            "metadata object-access resolve requires --project <PROJECT_ID>".to_string()
        })?,
        lease_id: lease_id.ok_or_else(|| {
            "metadata object-access resolve requires --lease <LEASE_ID>".to_string()
        })?,
        required_capabilities,
    })
}

fn parse_managed_object_capabilities(value: &str) -> Result<Vec<ManagedObjectCapability>, String> {
    let mut capabilities = value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse()
                .map_err(|error: bindhub_metadata::MetadataError| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if capabilities.is_empty() {
        return Err("--capabilities requires at least one capability".to_string());
    }
    capabilities.sort();
    capabilities.dedup();
    Ok(capabilities)
}

fn parse_managed_credential_project_arg(value: Option<&String>) -> Result<String, String> {
    let value = value
        .ok_or_else(|| "--project requires a project id".to_string())?
        .trim();
    if value == "*" {
        return Err(
            "project id '*' is reserved for account-wide managed object credential leases"
                .to_string(),
        );
    }
    Ok(value.to_string())
}

fn parse_sync_metadata_arg(
    flag: &str,
    args: &[String],
    index: &mut usize,
    metadata: &mut SyncMetadataArgs,
) -> Result<bool, String> {
    match flag {
        "--metadata-mode" => {
            *index += 1;
            let value = args.get(*index).ok_or_else(|| {
                "--metadata-mode requires local-mock, mock-dev-sqlite, or hosted-api".to_string()
            })?;
            metadata.mode = match value.as_str() {
                "local-mock" => SyncMetadataModeArg::LocalMock,
                "mock-dev-sqlite" => SyncMetadataModeArg::MockDevSqlite,
                "hosted-api" => SyncMetadataModeArg::HostedApi,
                _ => {
                    return Err(
                        "--metadata-mode requires local-mock, mock-dev-sqlite, or hosted-api"
                            .to_string(),
                    )
                }
            };
        }
        "--metadata-db" => {
            *index += 1;
            metadata.db_path = args.get(*index).cloned();
        }
        "--metadata-account" => {
            *index += 1;
            metadata.account_id = args.get(*index).cloned();
        }
        "--metadata-project" => {
            *index += 1;
            metadata.project_id = args.get(*index).cloned();
        }
        "--metadata-endpoint" => {
            *index += 1;
            metadata.endpoint = args.get(*index).cloned();
        }
        "--metadata-api" => {
            *index += 1;
            metadata.api = args.get(*index).cloned();
        }
        "--metadata-session-token-env" => {
            *index += 1;
            metadata.session_token_env = args.get(*index).cloned();
        }
        _ => return Ok(false),
    }

    Ok(true)
}

fn finalize_sync_metadata(
    metadata: SyncMetadataArgs,
    command: &str,
    require_project_for_metadata: bool,
) -> Result<SyncMetadataArgs, String> {
    if metadata.mode == SyncMetadataModeArg::LocalMock {
        if metadata.db_path.is_some()
            || metadata.account_id.is_some()
            || metadata.project_id.is_some()
            || metadata.endpoint.is_some()
            || metadata.api.is_some()
            || metadata.session_token_env.is_some()
        {
            return Err(format!(
                "{command} metadata flags require --metadata-mode mock-dev-sqlite or hosted-api"
            ));
        }
        return Ok(metadata);
    }

    match metadata.mode {
        SyncMetadataModeArg::LocalMock => unreachable!(),
        SyncMetadataModeArg::MockDevSqlite => {
            if metadata.api.is_some() || metadata.session_token_env.is_some() {
                return Err(format!(
                    "{command} hosted API metadata flags require --metadata-mode hosted-api"
                ));
            }
            if metadata.db_path.is_none() {
                return Err(format!(
                    "{command} requires --metadata-db <DB_PATH> with --metadata-mode mock-dev-sqlite"
                ));
            }
            if !require_project_for_metadata
                && (metadata.account_id.is_some() || metadata.project_id.is_some())
            {
                return Err(format!(
                    "{command} accepts --metadata-account/--metadata-project only for import-snapshot or materialize"
                ));
            }
            if require_project_for_metadata && metadata.project_id.is_none() {
                return Err(format!(
                    "{command} requires --metadata-project <PROJECT_ID> with --metadata-mode mock-dev-sqlite"
                ));
            }
            if let Some(endpoint) = &metadata.endpoint {
                MetadataServiceConfig {
                    endpoint: endpoint.clone(),
                    auth_mode: MetadataAuthMode::MockDevHeaders,
                }
                .validate()
                .map_err(|error| error.to_string())?;
            }
        }
        SyncMetadataModeArg::HostedApi => {
            if metadata.db_path.is_some() || metadata.endpoint.is_some() {
                return Err(format!(
                    "{command} hosted API metadata uses --metadata-api, not --metadata-db or --metadata-endpoint"
                ));
            }
            if metadata.account_id.is_some() {
                return Err(format!(
                    "{command} hosted API metadata derives account identity from the authenticated session; remove --metadata-account"
                ));
            }
            if metadata.api.is_none() {
                return Err(format!(
                    "{command} requires --metadata-api <URL> with --metadata-mode hosted-api"
                ));
            }
            if require_project_for_metadata && metadata.project_id.is_none() {
                return Err(format!(
                    "{command} requires --metadata-project <PROJECT_ID> with --metadata-mode hosted-api"
                ));
            }
            let token_env = metadata
                .session_token_env
                .as_deref()
                .unwrap_or("BINDHUB_SESSION_TOKEN");
            validate_env_name(token_env, "--metadata-session-token-env")?;
            MetadataServiceConfig {
                endpoint: metadata.api.clone().expect("metadata api checked"),
                auth_mode: MetadataAuthMode::AccountSession,
            }
            .validate()
            .map_err(|error| error.to_string())?;
        }
    }

    Ok(metadata)
}

fn parse_sync_remote_arg(
    flag: &str,
    args: &[String],
    index: &mut usize,
    remote: &mut SyncRemoteArgs,
) -> Result<bool, String> {
    match flag {
        "--remote-kind" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or_else(|| "--remote-kind requires local, s3, or hosted".to_string())?;
            remote.kind = match value.as_str() {
                "local" => SyncRemoteKindArg::Local,
                "s3" => SyncRemoteKindArg::S3,
                "hosted" => SyncRemoteKindArg::Hosted,
                _ => return Err("--remote-kind requires local, s3, or hosted".to_string()),
            };
            Ok(true)
        }
        "--remote" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or_else(|| "--remote requires a directory".to_string())?;
            remote.local_root = Some(value.clone());
            Ok(true)
        }
        "--s3-endpoint" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or_else(|| "--s3-endpoint requires a URL".to_string())?;
            remote.s3_endpoint = Some(value.clone());
            Ok(true)
        }
        "--s3-bucket" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or_else(|| "--s3-bucket requires a bucket".to_string())?;
            remote.s3_bucket = Some(value.clone());
            Ok(true)
        }
        "--s3-region" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or_else(|| "--s3-region requires a region".to_string())?;
            remote.s3_region = value.clone();
            remote.s3_region_explicit = true;
            Ok(true)
        }
        "--s3-prefix" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or_else(|| "--s3-prefix requires a prefix".to_string())?;
            remote.s3_prefix = Some(value.clone());
            Ok(true)
        }
        "--s3-access-key-env" => {
            *index += 1;
            let value = args.get(*index).ok_or_else(|| {
                "--s3-access-key-env requires an environment variable name".to_string()
            })?;
            remote.s3_access_key_env = Some(value.clone());
            Ok(true)
        }
        "--s3-secret-key-env" => {
            *index += 1;
            let value = args.get(*index).ok_or_else(|| {
                "--s3-secret-key-env requires an environment variable name".to_string()
            })?;
            remote.s3_secret_key_env = Some(value.clone());
            Ok(true)
        }
        "--s3-session-token-env" => {
            *index += 1;
            let value = args.get(*index).ok_or_else(|| {
                "--s3-session-token-env requires an environment variable name".to_string()
            })?;
            remote.s3_session_token_env = Some(value.clone());
            Ok(true)
        }
        "--object-access-api" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or_else(|| "--object-access-api requires a URL".to_string())?;
            remote.object_access_api = Some(value.clone());
            Ok(true)
        }
        "--object-access-project" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or_else(|| "--object-access-project requires a project id".to_string())?;
            remote.object_access_project = Some(value.clone());
            Ok(true)
        }
        "--object-access-lease" => {
            *index += 1;
            let value = args
                .get(*index)
                .ok_or_else(|| "--object-access-lease requires a lease id".to_string())?;
            remote.object_access_lease = Some(value.clone());
            Ok(true)
        }
        "--object-access-session-token-env" => {
            *index += 1;
            let value = args.get(*index).ok_or_else(|| {
                "--object-access-session-token-env requires an environment variable name"
                    .to_string()
            })?;
            remote.object_access_session_token_env = Some(value.clone());
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn finalize_sync_remote(
    remote: SyncRemoteArgs,
    command_name: &str,
) -> Result<SyncRemoteArgs, String> {
    match remote.kind {
        SyncRemoteKindArg::Local => {
            if remote.local_root.is_none() {
                return Err(format!("{command_name} requires --remote <REMOTE_DIR>"));
            }
            if remote.has_s3_options() {
                return Err(format!(
                    "{command_name} received --s3-* flags; add --remote-kind s3 to use an S3-compatible remote"
                ));
            }
            if remote.has_object_access_options() {
                return Err(format!(
                    "{command_name} received --object-access-* flags; add --remote-kind hosted to use server-mediated object transfer"
                ));
            }
            Ok(remote)
        }
        SyncRemoteKindArg::S3 => {
            if remote.local_root.is_some() {
                return Err(format!(
                    "{command_name} uses --s3-endpoint/--s3-bucket for --remote-kind s3, not --remote"
                ));
            }
            if remote.has_object_access_options() {
                return Err(format!(
                    "{command_name} received --object-access-* flags; add --remote-kind hosted for server-mediated transfer"
                ));
            }
            if remote.s3_endpoint.is_none() {
                return Err(format!(
                    "{command_name} requires --s3-endpoint <URL> for --remote-kind s3"
                ));
            }
            if remote.s3_bucket.is_none() {
                return Err(format!(
                    "{command_name} requires --s3-bucket <BUCKET> for --remote-kind s3"
                ));
            }
            match (
                remote.s3_access_key_env.as_ref(),
                remote.s3_secret_key_env.as_ref(),
            ) {
                (Some(_), Some(_)) => {}
                (None, None) => {
                    if remote.s3_session_token_env.is_some() {
                        return Err(
                            "--s3-session-token-env requires --s3-access-key-env and --s3-secret-key-env"
                                .to_string(),
                        );
                    }
                }
                _ => {
                    return Err(
                        "--s3-access-key-env and --s3-secret-key-env must be provided together"
                            .to_string(),
                    );
                }
            }
            let _ = s3_config_from_args(&remote).map_err(|error| error.to_string())?;
            Ok(remote)
        }
        SyncRemoteKindArg::Hosted => {
            if remote.local_root.is_some() || remote.has_s3_options() {
                return Err(format!(
                    "{command_name} uses --object-access-* flags for --remote-kind hosted, not --remote or --s3-*"
                ));
            }
            if remote.object_access_api.is_none() {
                return Err(format!(
                    "{command_name} requires --object-access-api <URL> for --remote-kind hosted"
                ));
            }
            if remote.object_access_project.is_none() {
                return Err(format!(
                    "{command_name} requires --object-access-project <PROJECT_ID> for --remote-kind hosted"
                ));
            }
            if remote.object_access_lease.is_none() {
                return Err(format!(
                    "{command_name} requires --object-access-lease <LEASE_ID> for --remote-kind hosted"
                ));
            }
            let _ = hosted_config_from_args(&remote).map_err(|error| error.to_string())?;
            Ok(remote)
        }
    }
}

impl SyncRemoteArgs {
    fn has_s3_options(&self) -> bool {
        self.s3_endpoint.is_some()
            || self.s3_bucket.is_some()
            || self.s3_region_explicit
            || self.s3_prefix.is_some()
            || self.s3_access_key_env.is_some()
            || self.s3_secret_key_env.is_some()
            || self.s3_session_token_env.is_some()
    }

    fn has_object_access_options(&self) -> bool {
        self.object_access_api.is_some()
            || self.object_access_project.is_some()
            || self.object_access_lease.is_some()
            || self.object_access_session_token_env.is_some()
    }
}

fn parse_sync_preflight_args(args: &[String]) -> Result<SyncPreflightArgs, String> {
    let mut db_path = None;
    let mut project_id = None;
    let mut base_snapshot_id = None;
    let mut local_snapshot_id = None;
    let mut incoming_snapshot_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--project" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--project requires an id".to_string())?;
                project_id = Some(value.clone());
            }
            "--base" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--base requires a snapshot id".to_string())?;
                base_snapshot_id = Some(value.clone());
            }
            "--local" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--local requires a snapshot id".to_string())?;
                local_snapshot_id = Some(value.clone());
            }
            "--incoming" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--incoming requires a snapshot id".to_string())?;
                incoming_snapshot_id = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown sync preflight option '{value}'"));
            }
            _ => return Err("sync preflight accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(SyncPreflightArgs {
        db_path: db_path.ok_or_else(|| "sync preflight requires --db <DB_PATH>".to_string())?,
        project_id: project_id
            .ok_or_else(|| "sync preflight requires --project <PROJECT_ID>".to_string())?,
        base_snapshot_id,
        local_snapshot_id: local_snapshot_id
            .ok_or_else(|| "sync preflight requires --local <LOCAL_SNAPSHOT_ID>".to_string())?,
        incoming_snapshot_id: incoming_snapshot_id.ok_or_else(|| {
            "sync preflight requires --incoming <INCOMING_SNAPSHOT_ID>".to_string()
        })?,
    })
}

fn parse_sync_cursor_args(args: &[String], requires_value: bool) -> Result<SyncCursorArgs, String> {
    let mut db_path = None;
    let mut project_id = None;
    let mut device_id = None;
    let mut value = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let arg = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(arg.clone());
            }
            "--project" => {
                index += 1;
                let arg = args
                    .get(index)
                    .ok_or_else(|| "--project requires an id".to_string())?;
                project_id = Some(arg.clone());
            }
            "--device" => {
                index += 1;
                let arg = args
                    .get(index)
                    .ok_or_else(|| "--device requires an id".to_string())?;
                device_id = Some(arg.clone());
            }
            "--value" => {
                index += 1;
                let arg = args
                    .get(index)
                    .ok_or_else(|| "--value requires a cursor value".to_string())?;
                value = Some(arg.clone());
            }
            arg if arg.starts_with('-') => {
                return Err(format!("unknown sync cursor option '{arg}'"));
            }
            _ => return Err("sync cursor accepts only flags".to_string()),
        }

        index += 1;
    }

    if requires_value && value.is_none() {
        return Err("sync cursor set requires --value <CURSOR>".to_string());
    }

    Ok(SyncCursorArgs {
        db_path: db_path.ok_or_else(|| "sync cursor requires --db <DB_PATH>".to_string())?,
        project_id: project_id
            .ok_or_else(|| "sync cursor requires --project <PROJECT_ID>".to_string())?,
        device_id,
        value,
    })
}

fn parse_snapshot_create_args(args: &[String]) -> Result<SnapshotCreateArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut dry_run = false;
    let mut path = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--cache" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--cache requires a path".to_string())?;
                cache_root = Some(value.clone());
            }
            "--dry-run" => dry_run = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown snapshot option '{value}'"));
            }
            value => {
                if path.replace(value.to_string()).is_some() {
                    return Err("snapshot accepts exactly one project path".to_string());
                }
            }
        }

        index += 1;
    }

    let cache_root =
        cache_root.ok_or_else(|| "snapshot requires --cache <CACHE_ROOT>".to_string())?;
    let path = path.ok_or_else(|| "snapshot requires a project path".to_string())?;

    if !dry_run && db_path.is_none() {
        return Err("snapshot persistence requires --db <DB_PATH>".to_string());
    }

    Ok(SnapshotCreateArgs {
        db_path,
        cache_root,
        dry_run,
        path,
    })
}

fn parse_changes_scan_args(args: &[String]) -> Result<ChangesScanArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut path = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--cache" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--cache requires a path".to_string())?;
                cache_root = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown changes scan option '{value}'"));
            }
            value => {
                if path.replace(value.to_string()).is_some() {
                    return Err("changes scan accepts exactly one project path".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(ChangesScanArgs {
        db_path: db_path.ok_or_else(|| "changes scan requires --db <DB_PATH>".to_string())?,
        cache_root: cache_root
            .ok_or_else(|| "changes scan requires --cache <CACHE_ROOT>".to_string())?,
        path: path.ok_or_else(|| "changes scan requires a project path".to_string())?,
    })
}

fn parse_changes_list_args(args: &[String]) -> Result<ChangesListArgs, String> {
    let mut db_path = None;
    let mut project_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--project" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--project requires a project id".to_string())?;
                project_id = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown changes list option '{value}'"));
            }
            _ => return Err("changes list accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(ChangesListArgs {
        db_path: db_path.ok_or_else(|| "changes list requires --db <DB_PATH>".to_string())?,
        project_id,
    })
}

fn parse_conflict_compare_args(args: &[String]) -> Result<ConflictCompareArgs, String> {
    let mut db_path = None;
    let mut base_snapshot_id = None;
    let mut local_snapshot_id = None;
    let mut incoming_snapshot_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--base" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--base requires a snapshot id".to_string())?;
                base_snapshot_id = Some(value.clone());
            }
            "--local" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--local requires a snapshot id".to_string())?;
                local_snapshot_id = Some(value.clone());
            }
            "--incoming" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--incoming requires a snapshot id".to_string())?;
                incoming_snapshot_id = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown conflicts compare option '{value}'"));
            }
            _ => return Err("conflicts compare accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(ConflictCompareArgs {
        db_path: db_path.ok_or_else(|| "conflicts compare requires --db <DB_PATH>".to_string())?,
        base_snapshot_id,
        local_snapshot_id: local_snapshot_id
            .ok_or_else(|| "conflicts compare requires --local <SNAPSHOT_ID>".to_string())?,
        incoming_snapshot_id: incoming_snapshot_id
            .ok_or_else(|| "conflicts compare requires --incoming <SNAPSHOT_ID>".to_string())?,
    })
}

fn parse_conflict_list_args(args: &[String]) -> Result<ConflictListArgs, String> {
    let mut db_path = None;
    let mut project_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--project" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--project requires a project id".to_string())?;
                project_id = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown conflicts list option '{value}'"));
            }
            _ => return Err("conflicts list accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(ConflictListArgs {
        db_path: db_path.ok_or_else(|| "conflicts list requires --db <DB_PATH>".to_string())?,
        project_id,
    })
}

fn parse_conflict_show_args(args: &[String]) -> Result<ConflictShowArgs, String> {
    let mut db_path = None;
    let mut conflict_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown conflicts command option '{value}'"));
            }
            value => {
                if conflict_id.replace(value.to_string()).is_some() {
                    return Err("conflicts command accepts one conflict id".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(ConflictShowArgs {
        db_path: db_path.ok_or_else(|| "conflicts command requires --db <DB_PATH>".to_string())?,
        conflict_id: conflict_id
            .ok_or_else(|| "conflicts command requires <CONFLICT_ID>".to_string())?,
    })
}

fn parse_conflict_resolve_args(args: &[String]) -> Result<ConflictResolveArgs, String> {
    let mut db_path = None;
    let mut conflict_id = None;
    let mut manual_resolution = None;
    let mut confirm_no_auto_apply = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--manual-resolution" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    "--manual-resolution requires keep-local, keep-incoming, keep-both, or exported"
                        .to_string()
                })?;
                manual_resolution = Some(parse_manual_conflict_resolution(value)?);
            }
            "--confirm-no-auto-apply" => {
                confirm_no_auto_apply = true;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown conflicts resolve option '{value}'"));
            }
            value => {
                if conflict_id.replace(value.to_string()).is_some() {
                    return Err("conflicts resolve accepts one conflict id".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(ConflictResolveArgs {
        db_path: db_path.ok_or_else(|| "conflicts resolve requires --db <DB_PATH>".to_string())?,
        conflict_id: conflict_id
            .ok_or_else(|| "conflicts resolve requires <CONFLICT_ID>".to_string())?,
        manual_resolution: manual_resolution.ok_or_else(|| {
            "conflicts resolve requires --manual-resolution keep-local|keep-incoming|keep-both|exported"
                .to_string()
        })?,
        confirm_no_auto_apply,
    })
}

fn parse_manual_conflict_resolution(value: &str) -> Result<ManualConflictResolution, String> {
    match value {
        "keep-local" => Ok(ManualConflictResolution::KeepLocal),
        "keep-incoming" => Ok(ManualConflictResolution::KeepIncoming),
        "keep-both" => Ok(ManualConflictResolution::KeepBoth),
        "exported" => Ok(ManualConflictResolution::Exported),
        _ => Err(
            "--manual-resolution requires keep-local, keep-incoming, keep-both, or exported"
                .to_string(),
        ),
    }
}

fn parse_secret_policy_add_args(args: &[String]) -> Result<SecretPolicyAddArgs, String> {
    let mut db_path = None;
    let mut project_id = None;
    let mut path = None;
    let mut action = None;
    let mut envelope_ref = None;
    let mut note = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--project" => {
                index += 1;
                project_id = args.get(index).cloned();
            }
            "--path" => {
                index += 1;
                path = args.get(index).cloned();
            }
            "--action" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--action requires block, template, or envelope".to_string())?;
                action = Some(parse_secret_policy_action(value)?);
            }
            "--envelope-ref" => {
                index += 1;
                envelope_ref = args.get(index).cloned();
            }
            "--note" => {
                index += 1;
                note = args.get(index).cloned();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown secrets policy add option '{value}'"));
            }
            _ => return Err("secrets policy add accepts only flags".to_string()),
        }

        index += 1;
    }

    let action = action.ok_or_else(|| {
        "secrets policy add requires --action block|template|envelope".to_string()
    })?;
    if action == SecretPolicyAction::Envelope && envelope_ref.is_none() {
        return Err("secrets policy add envelope action requires --envelope-ref <REF>".to_string());
    }
    if action != SecretPolicyAction::Envelope && envelope_ref.is_some() {
        return Err(
            "secrets policy add accepts --envelope-ref only for envelope action".to_string(),
        );
    }
    if let Some(reference) = &envelope_ref {
        validate_secret_envelope_reference(reference).map_err(|error| error.to_string())?;
    }
    if let Some(note) = &note {
        validate_non_secret_reference(note, "policy note")?;
    }

    Ok(SecretPolicyAddArgs {
        db_path: db_path.ok_or_else(|| "secrets policy add requires --db <DB_PATH>".to_string())?,
        project_id: project_id
            .ok_or_else(|| "secrets policy add requires --project <PROJECT_ID>".to_string())?,
        path: path.ok_or_else(|| "secrets policy add requires --path <REL_PATH>".to_string())?,
        action,
        envelope_ref,
        note,
    })
}

fn parse_secret_policy_action(value: &str) -> Result<SecretPolicyAction, String> {
    match value {
        "block" => Ok(SecretPolicyAction::Block),
        "template" => Ok(SecretPolicyAction::Template),
        "envelope" => Ok(SecretPolicyAction::Envelope),
        _ => Err("--action requires block, template, or envelope".to_string()),
    }
}

fn parse_secret_policy_list_args(args: &[String]) -> Result<SecretPolicyListArgs, String> {
    let mut db_path = None;
    let mut project_id = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = args.get(index).cloned();
            }
            "--project" => {
                index += 1;
                project_id = args.get(index).cloned();
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown secrets policy list option '{value}'"));
            }
            _ => return Err("secrets policy list accepts only flags".to_string()),
        }

        index += 1;
    }

    Ok(SecretPolicyListArgs {
        db_path: db_path
            .ok_or_else(|| "secrets policy list requires --db <DB_PATH>".to_string())?,
        project_id,
    })
}

fn parse_snapshot_restore_args(args: &[String]) -> Result<SnapshotRestoreArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut target = None;
    let mut snapshot_id = None;
    let mut dry_run = false;
    let mut apply = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--db requires a path".to_string())?;
                db_path = Some(value.clone());
            }
            "--cache" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--cache requires a path".to_string())?;
                cache_root = Some(value.clone());
            }
            "--to" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--to requires a target directory".to_string())?;
                target = Some(value.clone());
            }
            "--dry-run" => dry_run = true,
            "--apply" => apply = true,
            value if value.starts_with('-') => {
                return Err(format!("unknown snapshot restore option '{value}'"));
            }
            value => {
                if snapshot_id.replace(value.to_string()).is_some() {
                    return Err("snapshot restore accepts exactly one snapshot id".to_string());
                }
            }
        }

        index += 1;
    }

    if dry_run && apply {
        return Err("snapshot restore accepts only one of --dry-run or --apply".to_string());
    }

    Ok(SnapshotRestoreArgs {
        db_path: db_path.ok_or_else(|| "snapshot restore requires --db <DB_PATH>".to_string())?,
        cache_root: cache_root
            .ok_or_else(|| "snapshot restore requires --cache <CACHE_ROOT>".to_string())?,
        target: target.ok_or_else(|| "snapshot restore requires --to <TARGET_DIR>".to_string())?,
        snapshot_id: snapshot_id
            .ok_or_else(|| "snapshot restore requires a snapshot id".to_string())?,
        apply,
    })
}

fn init_identity(args: &InitArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = Store::open_file(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store.ensure_local_identity(&EnsureLocalIdentityOptions {
        device_name: args.device_name.as_deref(),
    })?;

    println!("Local identity initialized");
    println!("Account id: {}", identity.account_id);
    println!("Current device id: {}", identity.device_id);
    println!("Current device name: {}", identity.device_name);
    println!("Created at: {}", identity.device_created_at);
    println!("SQLite database: {}", args.db_path);
    println!("Cloud authentication: not configured");
    println!("Key material: stored locally; not printed");

    Ok(())
}

fn devices_list(db_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(db_path)?;
    store.apply_migrations()?;
    let devices = store.list_device_trust()?;

    println!("Device id\tAccount id\tCurrent local\tName\tTrust state\tApproved at\tRevoked at\tLast seen at");
    for device in &devices {
        print_device_trust(device);
    }

    Ok(())
}

fn auth_mock_login(args: &DbOnlyArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .completed_local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let now = store.current_timestamp()?;
    let session = mock_login(&identity_view(&identity), &now);
    store.upsert_auth_session(&session)?;

    println!("Mock auth session active");
    println!("Account id: {}", session.account_id);
    println!("Provider: {}", session.provider_kind);
    println!("Subject: {}", session.subject);
    println!("Session state: {}", session.session_state);
    println!("Last refreshed at: {}", session.last_refreshed_at);
    println!("Production authentication: not configured");
    println!("Raw secrets: not printed");

    Ok(())
}

fn auth_status(args: &DbOnlyArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .completed_local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let session = store.auth_session(&identity.account_id)?;

    println!("Auth boundary: local/mock");
    println!("Account id: {}", identity.account_id);
    println!("Current device id: {}", identity.device_id);
    match session {
        Some(session) => {
            println!("Session state: {}", session.session_state);
            println!("Provider: {}", session.provider_kind);
            println!("Subject: {}", session.subject);
            println!("Last refreshed at: {}", session.last_refreshed_at);
        }
        None => {
            println!("Session state: missing");
            println!("Provider: none");
        }
    }
    let production_proof = store.account_ownership_proof(&identity.account_id)?;
    println!(
        "Production-shaped account proof: {}",
        if production_proof.is_some() {
            "configured"
        } else {
            "missing"
        }
    );
    println!("Production authentication: live OAuth/login UI not configured");

    Ok(())
}

fn auth_mock_verified_bootstrap(
    args: &AuthMockVerifiedBootstrapArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let now = store.current_timestamp()?;
    let now_unix = now_unix_seconds();
    let provider_subject = args
        .provider_subject
        .clone()
        .unwrap_or_else(|| format!("local-dev:{}", identity.account_id));
    let proof = create_account_ownership_proof(AccountOwnershipProofInput {
        account_id: &identity.account_id,
        provider_kind: &args.provider_kind,
        provider_issuer: &args.provider_issuer,
        provider_subject: &provider_subject,
        verified_email: args.verified_email.as_deref(),
        verified_domain: args.verified_domain.as_deref(),
        proof_issued_at: &now,
        proof_expires_at_unix: now_unix + args.proof_ttl_seconds,
    })?;
    store.upsert_account_ownership_proof(&proof)?;
    let session = create_account_session(
        &proof,
        &args.session_token,
        &now,
        now_unix,
        args.ttl_seconds,
    )?;
    store.upsert_account_session(&session)?;

    println!("Mock verified account boundary bootstrapped");
    println!("Account id: {}", proof.account_id);
    println!("Provider kind: {}", proof.provider_kind);
    println!("Provider issuer: {}", proof.provider_issuer);
    println!("Provider subject: {}", proof.provider_subject);
    println!(
        "Verified email: {}",
        proof.verified_email.as_deref().unwrap_or("not configured")
    );
    println!(
        "Verified domain: {}",
        proof.verified_domain.as_deref().unwrap_or("not configured")
    );
    println!("Session id: {}", session.session_id);
    println!("Session expires at unix: {}", session.expires_at_unix);
    println!("Session token: not printed");
    println!("Stored credential material: session token hash only");
    println!("Production authentication: live OAuth/login UI remains deferred");

    Ok(())
}

fn auth_proof_check(args: &AuthProofCheckArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let session = store
        .account_session_for_token(&args.session_token)?
        .ok_or("account session not found or token hash mismatch")?;
    let context = validate_account_session(&session, &args.session_token, now_unix_seconds())?;
    let proof = store.account_ownership_proof(&context.account_id)?;

    println!("Auth proof check: active");
    println!("Account id: {}", context.account_id);
    println!("Session id: {}", context.session_id);
    println!("Provider kind: {}", context.provider_kind);
    println!("Provider issuer: {}", context.provider_issuer);
    println!("Provider subject: {}", context.provider_subject);
    if let Some(proof) = proof {
        println!(
            "Verified email: {}",
            proof.verified_email.as_deref().unwrap_or("not configured")
        );
        println!(
            "Verified domain: {}",
            proof.verified_domain.as_deref().unwrap_or("not configured")
        );
    }
    println!("Session expires at unix: {}", session.expires_at_unix);
    println!("Session token: not printed");
    println!("Production authentication: live OAuth/login UI remains deferred");

    Ok(())
}

fn auth_revoke_session(args: &AuthRevokeSessionArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let session = store
        .account_session(&args.session_id)?
        .ok_or("account session not found")?;
    let now = store.current_timestamp()?;
    let revoked = revoke_account_session(&session, &now)?;
    store.upsert_account_session(&revoked)?;

    println!("Account session revoked");
    println!("Account id: {}", revoked.account_id);
    println!("Session id: {}", revoked.session_id);
    println!("Session state: {}", revoked.session_state);
    println!(
        "Revoked at: {}",
        revoked.revoked_at.as_deref().unwrap_or("not recorded")
    );
    println!("Session token: not printed");

    Ok(())
}

#[derive(Serialize)]
struct HostedLoginRequest<'a> {
    email: &'a str,
    invite_code: &'a str,
}

fn auth_hosted_login(args: &AuthHostedLoginArgs) -> Result<(), Box<dyn std::error::Error>> {
    let invite_code = invite_code_for_hosted_login(args)?;
    let response: AlphaLoginResponse = ureq::post(&api_url(&args.api, "/v1/auth/alpha/login")?)
        .send_json(serde_json::to_value(HostedLoginRequest {
            email: &args.email,
            invite_code: &invite_code,
        })?)?
        .into_json()?;

    println!("Hosted auth session active");
    println!("Account id: {}", response.account_id);
    println!("Session id: {}", response.session_id);
    println!("Provider kind: {}", response.provider_kind);
    println!("Provider issuer: {}", response.provider_issuer);
    println!("Provider subject: {}", response.provider_subject);
    println!("Session expires at unix: {}", response.expires_at_unix);
    println!("Session token env: BINDHUB_SESSION_TOKEN");
    println!("Session token export:");
    println!("export BINDHUB_SESSION_TOKEN='{}'", response.session_token);
    println!("Stored credential material: none");

    Ok(())
}

fn auth_hosted_status(args: &AuthHostedSessionArgs) -> Result<(), Box<dyn std::error::Error>> {
    let token = session_token_from_env(&args.session_token_env)?;
    let response: AuthSessionResponse = ureq::get(&api_url(&args.api, "/v1/auth/session")?)
        .set("authorization", &format!("Bearer {token}"))
        .call()?
        .into_json()?;

    println!("Hosted auth session active");
    println!("Account id: {}", response.account_id);
    println!("Session id: {}", response.session_id);
    println!("Provider kind: {}", response.provider_kind);
    println!("Provider issuer: {}", response.provider_issuer);
    println!("Provider subject: {}", response.provider_subject);
    println!("Session expires at unix: {}", response.expires_at_unix);
    println!("Session token: loaded from {}", args.session_token_env);

    Ok(())
}

fn auth_hosted_logout(args: &AuthHostedSessionArgs) -> Result<(), Box<dyn std::error::Error>> {
    let token = session_token_from_env(&args.session_token_env)?;
    let response: AuthSessionResponse = ureq::delete(&api_url(&args.api, "/v1/auth/session")?)
        .set("authorization", &format!("Bearer {token}"))
        .call()?
        .into_json()?;

    println!("Hosted auth session revoked");
    println!("Account id: {}", response.account_id);
    println!("Session id: {}", response.session_id);
    println!("Session token: loaded from {}", args.session_token_env);

    Ok(())
}

fn metadata_object_access_resolve(
    args: &MetadataObjectAccessResolveArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let token = session_token_from_env(&args.session_token_env)?;
    let path = format!(
        "/v1/projects/{}/object-access/{}",
        api_path_segment(&args.project_id, "project id")?,
        api_path_segment(&args.lease_id, "lease id")?
    );
    let grant: ManagedObjectAccessGrant = ureq::post(&api_url(&args.api, &path)?)
        .set("authorization", &format!("Bearer {token}"))
        .send_json(serde_json::to_value(ManagedObjectAccessRequest {
            required_capabilities: args.required_capabilities.clone(),
        })?)?
        .into_json()?;

    print_managed_object_access_grant(&grant);
    Ok(())
}

fn session_token_from_env(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    secret_from_env(name, "session token")
}

fn invite_code_for_hosted_login(
    args: &AuthHostedLoginArgs,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(name) = &args.invite_code_env {
        return secret_from_env(name, "invite code");
    }
    args.invite_code
        .clone()
        .ok_or_else(|| "hosted login requires an invite code".into())
}

fn secret_from_env(name: &str, label: &'static str) -> Result<String, Box<dyn std::error::Error>> {
    validate_non_secret_reference(name, &format!("{label} env name"))?;
    let value = std::env::var(name).map_err(|_| format!("{label} env var is not set: {name}"))?;
    if value.trim().is_empty() {
        return Err(format!("{label} env var is empty: {name}").into());
    }
    Ok(value)
}

fn validate_env_name(name: &str, flag: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(format!("{flag} requires an environment variable name"));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(format!("{flag} requires an environment variable name"));
    }
    Ok(())
}

fn api_url(api: &str, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let check = MetadataServiceConfig {
        endpoint: api.to_string(),
        auth_mode: MetadataAuthMode::AccountSession,
    }
    .validate()?;
    Ok(format!("{}{}", check.endpoint.trim_end_matches('/'), path))
}

fn api_path_segment(
    value: &str,
    field: &'static str,
) -> Result<String, Box<dyn std::error::Error>> {
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
        return Err(format!("{field} must be a safe hosted API path segment").into());
    }
    Ok(trimmed.to_string())
}

fn devices_invite(args: &DeviceInviteArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let now = store.current_timestamp()?;
    let draft = create_pairing_invitation(
        &identity_view(&identity),
        &now,
        now_unix_seconds(),
        args.ttl_seconds,
    )?;
    store.insert_pairing_invitation(&draft.invitation)?;

    println!("Pairing invitation created");
    println!("Invitation id: {}", draft.invitation.id);
    println!("Account id: {}", draft.invitation.account_id);
    println!("Inviter device id: {}", draft.invitation.inviter_device_id);
    println!("Status: {}", draft.invitation.status);
    println!("Expires at unix: {}", draft.invitation.expires_at_unix);
    println!("Pairing token: {}", draft.token.expose_for_cli());
    println!("Pairing token env: BINDHUB_PAIRING_TOKEN");
    println!(
        "export BINDHUB_PAIRING_TOKEN='{}'",
        draft.token.expose_for_cli()
    );
    println!("Provider: local/mock metadata");
    println!("Raw account/device keys: not printed");

    Ok(())
}

fn devices_approve(args: &DeviceApproveArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let token = PairingInvitationToken::parse(&args.token)?;
    let invitation = store
        .pairing_invitation(&token.id)?
        .ok_or_else(|| format!("pairing invitation not found: {}", token.id))?;
    let now = store.current_timestamp()?;
    let approval = approve_pairing_invitation(
        &identity_view(&identity),
        &invitation,
        &token,
        &args.device_name,
        &now,
        now_unix_seconds(),
    )?;
    store.persist_pairing_approval(&approval)?;

    println!("Device approved");
    println!("Device id: {}", approval.device.device_id);
    println!("Device name: {}", approval.device.display_name);
    println!("Account id: {}", approval.device.account_id);
    println!("Invitation id: {}", approval.device.invitation_id);
    println!("Trust state: approved");
    println!("Key envelope stored: true");
    println!("Raw account/device keys: not printed");

    Ok(())
}

fn devices_join(args: &DeviceJoinArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = Store::open_file(&args.db_path)?;
    store.apply_migrations()?;
    let token = PairingInvitationToken::parse(&pairing_token_for_receiver(args)?)?;
    let identity = store.prepare_pairing_receiver_identity(&token, &args.device_name)?;
    let join = create_pairing_join_request(&token, &identity.device_id)?;

    println!("Pairing join request created");
    println!("Account id: {}", join.account_id);
    println!("Receiver device id: {}", join.receiver_device_id);
    println!("Receiver device name: {}", identity.device_name);
    println!("Invitation id: {}", join.invitation_id);
    println!("Join request env: BINDHUB_PAIRING_JOIN_REQUEST");
    println!("Join request excludes the receiver device key; send it with the pairing token to the source approver");
    println!(
        "export BINDHUB_PAIRING_JOIN_REQUEST='{}'",
        join.expose_for_cli()
    );

    Ok(())
}

fn pairing_token_for_receiver(args: &DeviceJoinArgs) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(name) = &args.token_env {
        return secret_from_env(name, "pairing invitation token");
    }
    args.token
        .clone()
        .ok_or_else(|| "devices join requires a pairing invitation token".into())
}

fn devices_approve_join(args: &DeviceApproveJoinArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let join_request = pairing_join_request_for_approval(args)?;
    let join = PairingJoinRequest::parse(&join_request)?;
    let invitation = store
        .pairing_invitation(&join.invitation_id)?
        .ok_or_else(|| format!("pairing invitation not found: {}", join.invitation_id))?;
    let token = PairingInvitationToken::parse(&pairing_token_for_approval(args)?)?;
    let now = store.current_timestamp()?;
    let approval = approve_pairing_join_request(
        &identity_view(&identity),
        &invitation,
        &token,
        &join,
        &args.device_name,
        &now,
        now_unix_seconds(),
    )?;
    store.persist_pairing_approval(&approval)?;
    let completion = pairing_completion_from_approval(&approval);

    println!("Device join approved");
    println!("Device id: {}", approval.device.device_id);
    println!("Device name: {}", approval.device.display_name);
    println!("Account id: {}", approval.device.account_id);
    println!("Invitation id: {}", approval.device.invitation_id);
    println!("Completion env: BINDHUB_PAIRING_COMPLETION");
    println!("Completion is secret-bearing alpha handoff; share only with the receiver");
    println!(
        "export BINDHUB_PAIRING_COMPLETION='{}'",
        completion.expose_for_cli()
    );

    Ok(())
}

fn devices_complete(args: &DeviceCompleteArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let completion = pairing_completion_for_receiver(args)?;
    let completion = PairingCompletion::parse(&completion)?;
    let identity = store.complete_pairing_for_local_device(&completion)?;

    println!("Pairing completed");
    println!("Account id: {}", identity.account_id);
    println!("Device id: {}", identity.device_id);
    println!("Device name: {}", identity.device_name);
    println!("Key envelope stored: true");
    println!("Receiver can import/materialize without --mock-key-source-db");
    println!("Pairing completion consumed from secret-bearing alpha handoff");

    Ok(())
}

fn pairing_join_request_for_approval(
    args: &DeviceApproveJoinArgs,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(name) = &args.join_request_env {
        return secret_from_env(name, "pairing join request");
    }
    args.join_request
        .clone()
        .ok_or_else(|| "devices approve-join requires a join request".into())
}

fn pairing_token_for_approval(
    args: &DeviceApproveJoinArgs,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(name) = &args.token_env {
        return secret_from_env(name, "pairing invitation token");
    }
    args.token
        .clone()
        .ok_or_else(|| "devices approve-join requires a pairing invitation token".into())
}

fn pairing_completion_for_receiver(
    args: &DeviceCompleteArgs,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(name) = &args.completion_env {
        return secret_from_env(name, "pairing completion");
    }
    args.completion
        .clone()
        .ok_or_else(|| "devices complete requires a pairing completion".into())
}

fn devices_revoke(args: &DeviceRevokeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let revoked_at = store.current_timestamp()?;
    let revoked = store.revoke_device(&args.device_id, args.reason.as_deref(), &revoked_at)?;

    println!("Device revoked");
    println!("Device id: {}", revoked.device_id);
    println!("Account id: {}", revoked.account_id);
    println!("Trust state: {}", revoked.trust_state);
    println!(
        "Revoked at: {}",
        revoked.revoked_at.as_deref().unwrap_or("-")
    );
    println!("Provider: local/mock metadata");

    Ok(())
}

fn devices_recovery_grant_create(
    args: &DeviceRecoveryGrantArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let device = store
        .device_trust(&args.device_id)?
        .ok_or_else(|| format!("device not found: {}", args.device_id))?;
    let now = store.current_timestamp()?;
    let now_unix = now_unix_seconds();
    let grant = create_recovery_grant(
        &device.account_id,
        &device.device_id,
        &args.recovery_ref,
        &args.audit_label,
        &now,
        now_unix,
        args.ttl_seconds,
    )?;
    store.upsert_recovery_grant(&grant)?;

    println!("Recovery grant: created");
    print_recovery_grant(&grant);
    println!("Recovery secret/code plaintext: not printed");
    println!("Boundary: production-shaped no-network recovery primitive; UI remains deferred");

    Ok(())
}

fn devices_recovery_grant_revoke(
    args: &DeviceRecoveryRevokeArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let revoked_at = store.current_timestamp()?;
    let grant = store.revoke_recovery_grant(&args.grant_id, &revoked_at)?;

    println!("Recovery grant: revoked");
    print_recovery_grant(&grant);
    println!("Revocation idempotent: true");

    Ok(())
}

fn devices_rotate_key_envelope(
    args: &DeviceRotateEnvelopeArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let current = store
        .key_envelope_for_device(&args.device_id)?
        .ok_or_else(|| format!("key envelope not found for device: {}", args.device_id))?;
    let now = store.current_timestamp()?;
    let intent = create_device_rotation_intent(DeviceRotationIntentInput {
        account_id: &identity.account_id,
        device_id: &args.device_id,
        requested_by_session_id: args.session_id.as_deref(),
        reason: &args.reason,
        created_at: &now,
        now_unix: now_unix_seconds(),
        ttl_seconds: args.ttl_seconds,
        current_generation: current.rotation_generation,
    })?;
    store.upsert_device_rotation_intent(&intent)?;
    let (completed, envelope) = store.rotate_key_envelope_for_device(
        &intent,
        &identity.sync_key_hex,
        &now,
        now_unix_seconds(),
    )?;

    println!("Device key-envelope rotation: completed");
    print_device_rotation_intent(&completed);
    println!("Key envelope id: {}", envelope.id);
    println!("Key envelope generation: {}", envelope.rotation_generation);
    println!("Key envelope plaintext: not printed");
    println!("Device key material: not printed");
    println!("Boundary: production-shaped no-network rotation primitive; UI remains deferred");

    Ok(())
}

fn print_recovery_grant(grant: &RecoveryGrant) {
    println!("Grant id: {}", grant.id);
    println!("Account id: {}", grant.account_id);
    println!("Device id: {}", grant.device_id);
    println!("Grant reference: {}", grant.grant_ref);
    println!("Status: {}", grant.status);
    println!("Expires at unix: {}", grant.expires_at_unix);
    println!(
        "Consumed at: {}",
        grant.consumed_at.as_deref().unwrap_or("-")
    );
    println!("Revoked at: {}", grant.revoked_at.as_deref().unwrap_or("-"));
    println!("Audit label: {}", grant.audit_label);
}

fn print_device_rotation_intent(intent: &DeviceRotationIntent) {
    println!("Rotation intent id: {}", intent.id);
    println!("Account id: {}", intent.account_id);
    println!("Device id: {}", intent.device_id);
    println!(
        "Requested by session id: {}",
        intent.requested_by_session_id.as_deref().unwrap_or("-")
    );
    println!("Status: {}", intent.status);
    println!("Reason: {}", intent.reason);
    println!("Expires at unix: {}", intent.expires_at_unix);
    println!(
        "Completed at: {}",
        intent.completed_at.as_deref().unwrap_or("-")
    );
    println!(
        "Revoked at: {}",
        intent.revoked_at.as_deref().unwrap_or("-")
    );
    println!(
        "Key envelope generation: {}",
        intent.key_envelope_generation
    );
}

#[derive(Debug, Clone)]
enum RemoteProviderDescription {
    Local { root: PathBuf },
    S3 { redacted: S3RedactedConfig },
    Hosted { redacted: HostedRedactedConfig },
}

fn open_remote_provider(
    remote: &SyncRemoteArgs,
) -> Result<(Box<dyn RemoteBlobProvider>, RemoteProviderDescription), Box<dyn std::error::Error>> {
    match remote.kind {
        SyncRemoteKindArg::Local => {
            let root = remote
                .local_root
                .as_ref()
                .ok_or("local remote requires --remote <REMOTE_DIR>")?;
            let provider = LocalFilesystemBlobProvider::open(root)?;
            let description = RemoteProviderDescription::Local {
                root: provider.root().to_path_buf(),
            };
            Ok((Box::new(provider), description))
        }
        SyncRemoteKindArg::S3 => {
            let config = s3_config_from_args(remote)?;
            let redacted = config.redacted();
            let provider = S3CompatibleBlobProvider::from_env(config)?;
            Ok((
                Box::new(provider),
                RemoteProviderDescription::S3 { redacted },
            ))
        }
        SyncRemoteKindArg::Hosted => {
            let config = hosted_config_from_args(remote)?;
            let redacted = config.redacted();
            let provider = HostedObjectTransferProvider::from_env(config)?;
            Ok((
                Box::new(provider),
                RemoteProviderDescription::Hosted { redacted },
            ))
        }
    }
}

fn s3_config_from_args(
    remote: &SyncRemoteArgs,
) -> Result<S3CompatibleConfig, Box<dyn std::error::Error>> {
    let credentials = match (
        remote.s3_access_key_env.as_ref(),
        remote.s3_secret_key_env.as_ref(),
    ) {
        (Some(access), Some(secret)) => S3CredentialsSource::env(
            access.clone(),
            secret.clone(),
            remote.s3_session_token_env.clone(),
        )?,
        (None, None) => {
            if remote.s3_session_token_env.is_some() {
                return Err(
                    "--s3-session-token-env requires --s3-access-key-env and --s3-secret-key-env"
                        .into(),
                );
            }
            S3CredentialsSource::default()
        }
        _ => {
            return Err(
                "--s3-access-key-env and --s3-secret-key-env must be provided together".into(),
            );
        }
    };

    Ok(S3CompatibleConfig::new(
        remote
            .s3_endpoint
            .as_deref()
            .ok_or("--s3-endpoint is required")?,
        remote
            .s3_bucket
            .as_deref()
            .ok_or("--s3-bucket is required")?,
        remote.s3_region.as_str(),
        remote.s3_prefix.as_deref(),
        credentials,
    )?)
}

fn hosted_config_from_args(
    remote: &SyncRemoteArgs,
) -> Result<HostedObjectTransferConfig, Box<dyn std::error::Error>> {
    Ok(HostedObjectTransferConfig::new(
        remote
            .object_access_api
            .as_deref()
            .ok_or("--object-access-api is required")?,
        remote
            .object_access_project
            .as_deref()
            .ok_or("--object-access-project is required")?,
        remote
            .object_access_lease
            .as_deref()
            .ok_or("--object-access-lease is required")?,
        remote
            .object_access_session_token_env
            .as_deref()
            .unwrap_or("BINDHUB_SESSION_TOKEN"),
    )?)
}

fn print_remote_description(description: &RemoteProviderDescription) {
    match description {
        RemoteProviderDescription::Local { root } => {
            println!("Remote provider: local filesystem");
            println!("Remote root: {}", root.display());
            println!("Remote credentials: not used");
        }
        RemoteProviderDescription::S3 { redacted } => {
            println!("Remote provider: s3-compatible");
            println!("Remote endpoint host: {}", redacted.endpoint_host);
            println!("Remote bucket: {}", redacted.bucket);
            println!("Remote region: {}", redacted.region);
            println!(
                "Remote prefix: {}",
                redacted.prefix.as_deref().unwrap_or("-")
            );
            println!("Credential access key env: {}", redacted.access_key_env);
            println!("Credential secret key env: {}", redacted.secret_key_env);
            println!(
                "Credential session token env: {}",
                redacted.session_token_env.as_deref().unwrap_or("-")
            );
        }
        RemoteProviderDescription::Hosted { redacted } => {
            println!("Remote provider: hosted-object-transfer");
            println!("Remote API host: {}", redacted.api_host);
            println!("Remote project id: {}", redacted.project_id);
            println!("Object access lease: {}", redacted.lease_id);
            println!("Session token env: {}", redacted.session_token_env);
            println!("Client bucket credentials: not used");
        }
    }
}

fn print_cloud_auth_boundary(description: &RemoteProviderDescription) {
    match description {
        RemoteProviderDescription::Local { .. } => {
            println!("Cloud authentication: not configured");
        }
        RemoteProviderDescription::S3 { .. } => {
            println!("Cloud authentication: credentials loaded from environment");
        }
        RemoteProviderDescription::Hosted { .. } => {
            println!("Cloud authentication: account session bearer token");
            println!("Object credentials: held server-side");
        }
    }
}

fn sync_remote_check(args: &SyncRemoteCheckArgs) -> Result<(), Box<dyn std::error::Error>> {
    println!("Sync remote check");
    match args.remote.kind {
        SyncRemoteKindArg::Local => {
            let root = args
                .remote
                .local_root
                .as_ref()
                .ok_or("local remote requires --remote <REMOTE_DIR>")?;
            let description = RemoteProviderDescription::Local {
                root: PathBuf::from(root),
            };
            print_remote_description(&description);
            if args.validate_only {
                println!("Network check: skipped");
            } else {
                let (provider, _) = open_remote_provider(&args.remote)?;
                let probe = ObjectKey::new("bindhub/remote-check/probe")?;
                let status = if provider.head(&probe)?.is_some() {
                    "present"
                } else {
                    "missing"
                };
                println!("Probe object: {status}");
            }
        }
        SyncRemoteKindArg::S3 => {
            let config = s3_config_from_args(&args.remote)?;
            let redacted = config.redacted();
            print_remote_description(&RemoteProviderDescription::S3 { redacted });
            if args.validate_only {
                println!("Network check: skipped");
            } else {
                let provider = S3CompatibleBlobProvider::from_env(config)?;
                let probe = ObjectKey::new("bindhub/remote-check/probe")?;
                let status = if provider.head(&probe)?.is_some() {
                    "present"
                } else {
                    "missing"
                };
                println!("Probe object: {status}");
            }
        }
        SyncRemoteKindArg::Hosted => {
            let config = hosted_config_from_args(&args.remote)?;
            let redacted = config.redacted();
            print_remote_description(&RemoteProviderDescription::Hosted { redacted });
            if args.validate_only {
                println!("Network check: skipped");
            } else {
                let provider = HostedObjectTransferProvider::from_env(config)?;
                let probe = ObjectKey::new("bindhub/remote-check/probe")?;
                let status = if provider.head(&probe)?.is_some() {
                    "present"
                } else {
                    "missing"
                };
                println!("Probe object: {status}");
            }
        }
    }
    println!("Credentials redacted: true");
    println!("Status: ok");

    Ok(())
}

fn sync_upload(args: &SyncBlobArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .completed_local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let sync_key = SyncKey::from_hex(&identity.sync_key_hex)?;
    let blob_id = BlobId::from_blake3_hex(&args.blob_id)?;
    let object_key = sync_object_key(&blob_id, args.object_key.as_deref())?;
    let cache = BlobCache::open(&args.cache_root)?;
    let (provider, remote_description) = open_remote_provider(&args.remote)?;
    let uploaded =
        upload_blob_from_cache(&cache, provider.as_ref(), &sync_key, &blob_id, &object_key)?;

    println!("Sync upload: encrypted local-remote object");
    println!("Blob id: {blob_id}");
    println!("Object key: {}", uploaded.object_key);
    println!("Plaintext bytes: {}", uploaded.plaintext_bytes);
    println!("Remote bytes: {}", uploaded.remote_bytes);
    println!("Uploaded: {}", uploaded.uploaded);
    print_remote_description(&remote_description);
    print_cloud_auth_boundary(&remote_description);

    Ok(())
}

fn sync_download(args: &SyncBlobArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .completed_local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let sync_key = SyncKey::from_hex(&identity.sync_key_hex)?;
    let blob_id = BlobId::from_blake3_hex(&args.blob_id)?;
    let object_key = sync_object_key(&blob_id, args.object_key.as_deref())?;
    let cache = BlobCache::open(&args.cache_root)?;
    let (provider, remote_description) = open_remote_provider(&args.remote)?;
    let downloaded =
        download_blob_to_cache(&cache, provider.as_ref(), &sync_key, &blob_id, &object_key)?;

    println!("Sync download: decrypted into local blob cache");
    println!("Blob id: {blob_id}");
    println!("Object key: {}", downloaded.object_key);
    println!("Plaintext bytes: {}", downloaded.plaintext_bytes);
    println!("Remote bytes: {}", downloaded.remote_bytes);
    println!("Blob cache: {}", cache.root().display());
    print_remote_description(&remote_description);
    print_cloud_auth_boundary(&remote_description);

    Ok(())
}

fn open_sync_metadata_store(
    metadata: &SyncMetadataArgs,
) -> Result<SqliteMetadataStore, Box<dyn std::error::Error>> {
    let path = metadata
        .db_path
        .as_deref()
        .ok_or("metadata store path is missing")?;
    Ok(SqliteMetadataStore::open_file(path)?)
}

fn open_hosted_metadata_client(
    metadata: &SyncMetadataArgs,
) -> Result<HostedMetadataApiClient, Box<dyn std::error::Error>> {
    let api = metadata.api.as_deref().ok_or("metadata API is missing")?;
    let session_token_env = metadata
        .session_token_env
        .as_deref()
        .unwrap_or("BINDHUB_SESSION_TOKEN");
    Ok(HostedMetadataApiClient::new(
        HostedMetadataApiConfig::from_env(api, session_token_env)?,
    ))
}

fn metadata_import_options(
    request: &ImportSnapshotRequest,
    metadata: &SyncMetadataArgs,
) -> Result<HostedMetadataImportOptions, Box<dyn std::error::Error>> {
    let account_id = if let Some(account_id) = &metadata.account_id {
        account_id.clone()
    } else if let Some(path) = &request.key_source_db_path {
        let source_store = Store::open_file(path)?;
        source_store.apply_migrations()?;
        source_store
            .completed_local_identity()?
            .ok_or("metadata account id could not be derived from --mock-key-source-db")?
            .account_id
    } else {
        return Err(
            "hosted metadata import requires --metadata-account <ACCOUNT_ID> or --mock-key-source-db <PUBLISHER_DB>"
                .into(),
        );
    };

    Ok(HostedMetadataImportOptions {
        account_id,
        project_id: metadata
            .project_id
            .clone()
            .ok_or("metadata project id is missing")?,
    })
}

fn import_snapshot_command(
    request: &ImportSnapshotRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
    metadata: &SyncMetadataArgs,
) -> Result<bindhub_materialize::ImportedSnapshotBundle, MaterializeError> {
    if metadata.mode == SyncMetadataModeArg::MockDevSqlite {
        let mut metadata_store = open_sync_metadata_store(metadata)
            .map_err(|error| MaterializeError::InvalidBundle(error.to_string()))?;
        let options = metadata_import_options(request, metadata)
            .map_err(|error| MaterializeError::InvalidBundle(error.to_string()))?;
        import_snapshot_with_metadata(request, provider, &mut metadata_store, &options)
    } else if metadata.mode == SyncMetadataModeArg::HostedApi {
        let mut metadata_client = open_hosted_metadata_client(metadata)
            .map_err(|error| MaterializeError::InvalidBundle(error.to_string()))?;
        let receiver_store = Store::open_file(&request.db_path)?;
        receiver_store.apply_migrations()?;
        let receiver_identity = receiver_store
            .completed_local_identity()?
            .ok_or(MaterializeError::LocalIdentityMissing)?;
        let options = HostedMetadataImportOptions {
            account_id: receiver_identity.account_id,
            project_id: metadata.project_id.clone().ok_or_else(|| {
                MaterializeError::InvalidBundle("metadata project id is missing".to_string())
            })?,
        };
        import_snapshot_with_metadata(request, provider, &mut metadata_client, &options)
    } else {
        import_snapshot(request, provider)
    }
}

fn materialize_snapshot_command(
    request: &MaterializationRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
    metadata: &SyncMetadataArgs,
) -> Result<bindhub_materialize::MaterializationOutcome, MaterializeError> {
    if metadata.mode == SyncMetadataModeArg::MockDevSqlite {
        let mut metadata_store = open_sync_metadata_store(metadata)
            .map_err(|error| MaterializeError::InvalidBundle(error.to_string()))?;
        let import_request = ImportSnapshotRequest {
            db_path: request.db_path.clone(),
            cache_root: request.cache_root.clone(),
            key_source_db_path: request.key_source_db_path.clone(),
            snapshot_id: request.snapshot_id.clone(),
        };
        let options = metadata_import_options(&import_request, metadata)
            .map_err(|error| MaterializeError::InvalidBundle(error.to_string()))?;
        materialize_snapshot_with_metadata(request, provider, &mut metadata_store, &options)
    } else if metadata.mode == SyncMetadataModeArg::HostedApi {
        let mut metadata_client = open_hosted_metadata_client(metadata)
            .map_err(|error| MaterializeError::InvalidBundle(error.to_string()))?;
        let receiver_store = Store::open_file(&request.db_path)?;
        receiver_store.apply_migrations()?;
        let receiver_identity = receiver_store
            .completed_local_identity()?
            .ok_or(MaterializeError::LocalIdentityMissing)?;
        let options = HostedMetadataImportOptions {
            account_id: receiver_identity.account_id,
            project_id: metadata.project_id.clone().ok_or_else(|| {
                MaterializeError::InvalidBundle("metadata project id is missing".to_string())
            })?,
        };
        materialize_snapshot_with_metadata(request, provider, &mut metadata_client, &options)
    } else {
        materialize_snapshot(request, provider)
    }
}

fn print_sync_metadata_boundary(
    metadata: &SyncMetadataArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    match metadata.mode {
        SyncMetadataModeArg::LocalMock => {
            println!(
                "Boundary: local/mock second-device foundation with pluggable encrypted object remote; hosted metadata/auth/UI not configured"
            );
            println!(
                "Metadata mode: local/mock only; hosted metadata discovery and cursor CAS not configured"
            );
        }
        SyncMetadataModeArg::MockDevSqlite => {
            println!(
                "Metadata mode: hosted mock-dev sqlite; manifest discovery and cursor CAS active"
            );
            println!("Metadata auth mode: mock-dev-headers");
            println!("Metadata store: configured");
            if let Some(endpoint) = &metadata.endpoint {
                let check = MetadataServiceConfig {
                    endpoint: endpoint.clone(),
                    auth_mode: MetadataAuthMode::MockDevHeaders,
                }
                .validate()?;
                println!("Metadata endpoint (sanitized): {}", check.endpoint);
                println!("Metadata network check: {}", check.network_check);
            }
            println!(
                "Boundary: mock-dev metadata wiring only; production OAuth, managed credentials, deployment hardening, Electron UI, and conflict UI are deferred"
            );
        }
        SyncMetadataModeArg::HostedApi => {
            let api = metadata.api.as_deref().ok_or("metadata API is missing")?;
            let session_token_env = metadata
                .session_token_env
                .as_deref()
                .unwrap_or("BINDHUB_SESSION_TOKEN");
            let check = MetadataServiceConfig {
                endpoint: api.to_string(),
                auth_mode: MetadataAuthMode::AccountSession,
            }
            .validate()?;
            println!("Metadata mode: hosted-api; manifest discovery and cursor CAS use HTTP");
            println!("Metadata auth mode: account-session");
            println!("Metadata API (sanitized): {}", check.endpoint);
            println!("Metadata session token env: {session_token_env}");
            println!("Metadata session token: not printed");
            println!(
                "Boundary: external hosted alpha metadata uses the authenticated session account; --metadata-account is not accepted"
            );
        }
    }

    Ok(())
}

fn sync_publish_snapshot(args: &SyncSnapshotArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (provider, remote_description) = open_remote_provider(&args.remote)?;
    let request = PublishSnapshotRequest {
        db_path: PathBuf::from(&args.db_path),
        cache_root: PathBuf::from(&args.cache_root),
        snapshot_id: args.snapshot_id.clone(),
    };
    let published = if args.metadata.mode == SyncMetadataModeArg::MockDevSqlite {
        let mut metadata_store = open_sync_metadata_store(&args.metadata)?;
        publish_snapshot_with_metadata(&request, provider.as_ref(), &mut metadata_store)?
    } else if args.metadata.mode == SyncMetadataModeArg::HostedApi {
        let mut metadata_client = open_hosted_metadata_client(&args.metadata)?;
        publish_snapshot_with_metadata(&request, provider.as_ref(), &mut metadata_client)?
    } else {
        publish_snapshot(&request, provider.as_ref())?
    };

    println!("Sync publish snapshot: encrypted local/mock bundle");
    println!("Account id: {}", published.account_id);
    println!("Device id: {}", published.device_id);
    println!("Project id: {}", published.project_id);
    println!("Snapshot id: {}", published.snapshot_id);
    println!("Manifest object key: {}", published.manifest_object_key);
    println!(
        "Manifest plaintext bytes: {}",
        published.manifest_plaintext_bytes
    );
    println!("Manifest remote bytes: {}", published.manifest_remote_bytes);
    println!("Manifest uploaded: {}", published.manifest_uploaded);
    println!("Included blob count: {}", published.blob_count);
    println!("Uploaded blob count: {}", published.uploaded_blob_count);
    println!("Plaintext blob bytes: {}", published.plaintext_blob_bytes);
    println!("Remote blob bytes: {}", published.remote_blob_bytes);
    print_remote_description(&remote_description);
    print_sync_metadata_boundary(&args.metadata)?;

    Ok(())
}

fn sync_import_snapshot(args: &SyncSnapshotArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (provider, remote_description) = open_remote_provider(&args.remote)?;
    let request = ImportSnapshotRequest {
        db_path: PathBuf::from(&args.db_path),
        cache_root: PathBuf::from(&args.cache_root),
        key_source_db_path: args.mock_key_source_db.as_ref().map(PathBuf::from),
        snapshot_id: args.snapshot_id.clone(),
    };
    let imported = match import_snapshot_command(&request, provider.as_ref(), &args.metadata) {
        Ok(imported) => imported,
        Err(MaterializeError::PreflightBlocked(outcome)) => {
            print_sync_preflight_outcome(&outcome);
            return Err("sync import-snapshot refused by local preflight".into());
        }
        Err(error) => return Err(error.into()),
    };

    println!("Sync import snapshot: decrypted into local/mock second context");
    println!("Source account id: {}", imported.source_account_id);
    println!("Receiver account id: {}", imported.receiver_account_id);
    println!("Receiver device id: {}", imported.receiver_device_id);
    println!("Project id: {}", imported.project_id);
    println!("Snapshot id: {}", imported.snapshot_id);
    println!("Manifest object key: {}", imported.manifest_object_key);
    println!("Snapshot inserted: {}", imported.snapshot_inserted);
    println!("Included blob count: {}", imported.blob_count);
    println!("Downloaded blob count: {}", imported.downloaded_blob_count);
    println!("Plaintext blob bytes: {}", imported.plaintext_blob_bytes);
    println!("Remote blob bytes: {}", imported.remote_blob_bytes);
    println!("Cursor value: {}", imported.cursor_value);
    println!("Cursor updated at: {}", imported.cursor_updated_at);
    print_remote_description(&remote_description);
    if args.mock_key_source_db.is_some() {
        println!(
            "Trust bootstrap: local/mock --mock-key-source-db used; raw keys were not printed"
        );
    } else {
        println!("Trust bootstrap: receiver local identity key");
    }
    print_sync_metadata_boundary(&args.metadata)?;

    Ok(())
}

fn sync_materialize(args: &SyncMaterializeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (provider, remote_description) = open_remote_provider(&args.remote)?;
    let request = MaterializationRequest {
        db_path: PathBuf::from(&args.db_path),
        cache_root: PathBuf::from(&args.cache_root),
        key_source_db_path: args.mock_key_source_db.as_ref().map(PathBuf::from),
        snapshot_id: args.snapshot_id.clone(),
        target: PathBuf::from(&args.target),
        apply: args.apply,
    };
    let outcome = match materialize_snapshot_command(&request, provider.as_ref(), &args.metadata) {
        Ok(outcome) => outcome,
        Err(MaterializeError::PreflightBlocked(outcome)) => {
            print_sync_preflight_outcome(&outcome);
            return Err("sync materialize refused by local preflight".into());
        }
        Err(error) => return Err(error.into()),
    };

    println!("Sync materialize snapshot");
    println!("Mode: {}", if args.apply { "apply" } else { "dry-run" });
    println!("Source account id: {}", outcome.import.source_account_id);
    println!(
        "Receiver account id: {}",
        outcome.import.receiver_account_id
    );
    println!("Receiver device id: {}", outcome.import.receiver_device_id);
    println!("Project id: {}", outcome.import.project_id);
    println!("Snapshot id: {}", outcome.import.snapshot_id);
    println!("Snapshot inserted: {}", outcome.import.snapshot_inserted);
    println!("Target: {}", outcome.target.display());
    println!("Target status: {}", outcome.target_status);
    println!("Apply allowed: {}", outcome.apply_allowed);
    println!("Applied: {}", outcome.applied);
    println!("Directories to create: {}", outcome.plan.dirs_to_create);
    println!("Files to write: {}", outcome.plan.files_to_write);
    println!("Skipped entries: {}", outcome.plan.skipped_entries);
    println!("Missing blobs: {}", outcome.plan.missing_blobs);
    println!("Bytes to write: {}", outcome.plan.bytes_to_write);
    println!("Cursor value: {}", outcome.import.cursor_value);
    println!("Cursor updated at: {}", outcome.import.cursor_updated_at);
    print_remote_description(&remote_description);
    if args.mock_key_source_db.is_some() {
        println!(
            "Trust bootstrap: local/mock --mock-key-source-db used; raw keys were not printed"
        );
    } else {
        println!("Trust bootstrap: receiver local identity key");
    }
    print_sync_metadata_boundary(&args.metadata)?;

    Ok(())
}

fn sync_preflight_command(args: &SyncPreflightArgs) -> Result<(), Box<dyn std::error::Error>> {
    let outcome = sync_preflight(&SyncPreflightRequest {
        db_path: PathBuf::from(&args.db_path),
        project_id: args.project_id.clone(),
        base_snapshot_id: args.base_snapshot_id.clone(),
        local_snapshot_id: Some(args.local_snapshot_id.clone()),
        incoming_snapshot_id: args.incoming_snapshot_id.clone(),
    })?;

    print_sync_preflight_outcome(&outcome);
    if outcome.is_blocked() {
        return Err("sync preflight blocked".into());
    }

    Ok(())
}

fn sync_cursor_get(args: &SyncCursorArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let device_id = args.device_id.as_deref().unwrap_or(&identity.device_id);
    let cursor = store.device_project_cursor(&identity.account_id, device_id, &args.project_id)?;

    println!("Sync cursor");
    println!("Account id: {}", identity.account_id);
    println!("Device id: {device_id}");
    println!("Project id: {}", args.project_id);
    match cursor {
        Some(cursor) => {
            println!("Cursor value: {}", cursor.cursor_value);
            println!("Updated at: {}", cursor.updated_at);
        }
        None => {
            println!("Cursor value: -");
            println!("Updated at: -");
        }
    }
    println!("Provider: local/mock metadata");

    Ok(())
}

fn sync_cursor_set(args: &SyncCursorArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .local_identity()?
        .ok_or("local identity is not initialized; run bindhub init --db <DB_PATH>")?;
    let device_id = args.device_id.clone().unwrap_or(identity.device_id.clone());
    let value = args
        .value
        .as_ref()
        .ok_or("sync cursor set requires --value <CURSOR>")?;
    let now = store.current_timestamp()?;
    let cursor = DeviceProjectCursor {
        account_id: identity.account_id.clone(),
        device_id,
        project_id: args.project_id.clone(),
        cursor_value: value.clone(),
        updated_at: now,
    };
    store.upsert_device_project_cursor(&cursor)?;

    println!("Sync cursor updated");
    println!("Account id: {}", cursor.account_id);
    println!("Device id: {}", cursor.device_id);
    println!("Project id: {}", cursor.project_id);
    println!("Cursor value: {}", cursor.cursor_value);
    println!("Updated at: {}", cursor.updated_at);
    println!("Provider: local/mock metadata");

    Ok(())
}

fn metadata_check(args: &MetadataCheckArgs) -> Result<(), Box<dyn std::error::Error>> {
    let check = MetadataServiceConfig {
        endpoint: args.endpoint.clone(),
        auth_mode: args.auth_mode,
    }
    .validate()?;

    println!("Metadata service check");
    println!("Endpoint (sanitized): {}", check.endpoint);
    match check.auth_mode {
        MetadataAuthMode::MockDevHeaders => {
            println!("Auth mode: mock-dev-headers");
            println!("Required headers: x-bindhub-mock-account-id, x-bindhub-mock-device-id");
        }
        MetadataAuthMode::AccountSession => {
            println!("Auth mode: account-session");
            println!("Required header: Authorization: Bearer <session-token>");
            println!("Session token: not printed");
        }
    }
    println!("Network check: {}", check.network_check);
    println!("Production ready: {}", check.production_ready);
    println!(
        "Boundary: production-shaped service trust only; live OAuth, managed provider provisioning, deployment hardening, and UI are deferred"
    );

    Ok(())
}

fn metadata_alpha_invite_create(
    args: &MetadataAlphaInviteCreateArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_metadata_admin_store(&args.store)?;
    let now_unix = now_unix_seconds();
    let now = format!("unix:{now_unix}");
    let invite_code = match &args.invite_code {
        Some(code) => code.clone(),
        None => generate_alpha_invite_code()?,
    };
    let request = create_alpha_invite_request(
        &invite_code,
        args.email.as_deref(),
        args.domain.as_deref(),
        &now,
        now_unix,
        args.ttl_seconds,
    )?;
    let record = store.create_alpha_invite(request)?;

    println!("Alpha invite created");
    println!("Invite id: {}", record.invite_id);
    println!(
        "Allowed email: {}",
        record.allowed_email.as_deref().unwrap_or("-")
    );
    println!(
        "Allowed domain: {}",
        record.allowed_domain.as_deref().unwrap_or("-")
    );
    println!("Invite state: {}", record.invite_state);
    println!("Expires at unix: {}", record.expires_at_unix);
    println!("Invite code: {invite_code}");
    println!("Stored credential material: invite code hash only");

    Ok(())
}

fn metadata_credential_lease_mock_create(
    args: &MetadataCredentialLeaseArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_metadata_admin_store(&args.store)?;
    let now_unix = now_unix_seconds();
    let now = format!("unix:{now_unix}");
    let session_hash = bindhub_auth::hash_session_token_hex(&args.session_token);
    let existing_session = store.account_session_by_hash(&session_hash)?;
    let existing_context = if existing_session.is_some() {
        Some(authenticate_account_session(
            &*store,
            &args.session_token,
            now_unix,
        )?)
    } else {
        None
    };
    if existing_context.is_none()
        && matches!(
            args.store,
            MetadataAdminStoreSelector::PostgresUrlEnv { .. }
        )
    {
        return Err(
            "metadata credential-lease mock-create with --postgres-url-env requires an existing authenticated session for --session-token"
                .into(),
        );
    }
    let account_id = match (&args.account_id, &existing_context) {
        (Some(account_id), Some(session)) if account_id != &session.account_id => {
            return Err(format!(
                "metadata credential-lease account mismatch: --account {account_id} does not match authenticated session account {}",
                session.account_id
            )
            .into());
        }
        (Some(account_id), _) => account_id.clone(),
        (None, Some(session)) => session.account_id.clone(),
        (None, None) => dev_mock_account_id(
            args.verified_email
                .as_deref()
                .or(args.verified_domain.as_deref())
                .unwrap_or("dev-account"),
        ),
    };
    if existing_context.is_none() {
        let provider_subject = format!("managed-object-dev:{account_id}");
        let proof = create_account_ownership_proof(AccountOwnershipProofInput {
            account_id: &account_id,
            provider_kind: "oidc-dev",
            provider_issuer: "https://bindhub.local/mock-managed-object-lease",
            provider_subject: &provider_subject,
            verified_email: args.verified_email.as_deref(),
            verified_domain: args.verified_domain.as_deref(),
            proof_issued_at: &now,
            proof_expires_at_unix: now_unix + 86_400,
        })?;
        store.upsert_account_ownership_proof(proof.clone())?;
        let session = create_account_session(
            &proof,
            &args.session_token,
            &now,
            now_unix,
            args.ttl_seconds,
        )?;
        store.upsert_account_session(session)?;
    }
    if let Some(project_id) = &args.project_id {
        store.upsert_project(bindhub_metadata::UpsertProjectRequest {
            account_id: account_id.clone(),
            project_id: project_id.clone(),
            display_name: project_id.clone(),
            root_hint: "mock-dev-managed-object-lease".to_string(),
            project_kind: "mock-dev".to_string(),
            updated_at: now.clone(),
        })?;
    }
    let lease =
        store.upsert_managed_object_credential_lease(ManagedObjectCredentialLeaseRequest {
            account_id,
            project_id: args.project_id.clone(),
            lease_id: args.lease_id.clone(),
            provider_kind: args.provider_kind,
            endpoint: args.endpoint.clone(),
            bucket: args.bucket.clone(),
            region: args.region.clone(),
            prefix: args.prefix.clone(),
            credential_reference: mock_credential_reference(&args.lease_id, 0),
            credential_fingerprint: Some(mock_credential_fingerprint_reference(&args.lease_id, 0)),
            capabilities: args.capabilities.clone(),
            issued_at: now,
            expires_at_unix: now_unix + args.ttl_seconds,
            rotation_generation: 0,
        })?;

    println!("Managed object credential lease: mock-created");
    print_managed_object_credential_lease(&lease)?;
    println!(
        "Boundary: no live Cloudflare/AWS provisioning; raw object credentials are not stored or printed"
    );

    Ok(())
}

fn metadata_credential_lease_check(
    args: &MetadataCredentialLeaseLookupArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_metadata_admin_store(&args.store)?;
    let lease = active_managed_object_credential_lease_for_session(
        &*store,
        &args.session_token,
        args.project_id.as_deref(),
        &args.lease_id,
        &args.required_capabilities,
        now_unix_seconds(),
    )?;

    println!("Managed object credential lease: active");
    print_managed_object_credential_lease(&lease)?;
    println!("Active use: accepted for authenticated account/session scope");

    Ok(())
}

fn metadata_credential_lease_revoke(
    args: &MetadataCredentialLeaseMutateArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_metadata_admin_store(&args.store)?;
    let now_unix = now_unix_seconds();
    let session = authenticate_account_session(&*store, &args.session_token, now_unix)?;
    let revoked = store.revoke_managed_object_credential_lease(
        &session.account_id,
        args.project_id.as_deref(),
        &args.lease_id,
        &format!("unix:{now_unix}"),
    )?;

    println!("Managed object credential lease: revoked");
    print_managed_object_credential_lease(&revoked)?;

    Ok(())
}

fn metadata_credential_lease_rotate(
    args: &MetadataCredentialLeaseMutateArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_metadata_admin_store(&args.store)?;
    let now_unix = now_unix_seconds();
    let current = active_managed_object_credential_lease_for_session(
        &*store,
        &args.session_token,
        args.project_id.as_deref(),
        &args.lease_id,
        &[],
        now_unix,
    )?;
    let next_generation = current.rotation_generation + 1;
    let rotated = store.rotate_managed_object_credential_lease(
        &current.account_id,
        current.project_id.as_deref(),
        &current.lease_id,
        mock_credential_reference(&current.lease_id, next_generation),
        Some(mock_credential_fingerprint_reference(
            &current.lease_id,
            next_generation,
        )),
        &format!("unix:{now_unix}"),
    )?;

    println!("Managed object credential lease: rotated");
    print_managed_object_credential_lease(&rotated)?;

    Ok(())
}

fn print_managed_object_credential_lease(
    lease: &bindhub_metadata::ManagedObjectCredentialLeaseRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    let redacted = redacted_managed_object_remote_config(lease)?;
    println!("Account id: {}", lease.account_id);
    println!("Project id: {}", lease.project_id.as_deref().unwrap_or("-"));
    println!("Lease id: {}", lease.lease_id);
    println!("Provider kind: {}", lease.provider_kind);
    println!("Endpoint host: {}", redacted.endpoint_host);
    println!("Bucket: {}", lease.bucket);
    println!("Region: {}", lease.region);
    println!("Prefix: {}", lease.prefix.as_deref().unwrap_or("-"));
    println!(
        "Capabilities: {}",
        cli_capabilities_to_string(&lease.capabilities)
    );
    println!("Generation: {}", lease.rotation_generation);
    println!("Issued at: {}", lease.issued_at);
    println!("Expires at unix: {}", lease.expires_at_unix);
    println!(
        "Revocation status: {}",
        lease.revoked_at.as_deref().unwrap_or("active")
    );
    println!("Credential reference: {}", lease.credential_reference);
    println!("Resolved remote config: {redacted}");
    Ok(())
}

fn print_managed_object_access_grant(grant: &ManagedObjectAccessGrant) {
    println!("Managed object access grant: active");
    println!("Account id: {}", grant.account_id);
    println!("Project id: {}", grant.project_id);
    println!("Lease id: {}", grant.lease_id);
    println!("Provider kind: {}", grant.provider_kind);
    println!("Endpoint: {}", grant.endpoint);
    println!("Endpoint host: {}", grant.endpoint_host);
    println!("Bucket: {}", grant.bucket);
    println!("Region: {}", grant.region);
    println!("Authorized prefix: {}", grant.prefix);
    println!(
        "Capabilities: {}",
        cli_capabilities_to_string(&grant.capabilities)
    );
    println!("Generation: {}", grant.rotation_generation);
    println!("Expires at unix: {}", grant.expires_at_unix);
    println!("Credential delivery: {}", grant.credential_delivery);
    println!("Credential reference: {}", grant.credential_reference);
    println!("Client object credentials: not returned");
    println!("Shared bucket rule: use only the authorized prefix for this account/project");
    println!(
        "Hosted transfer: external testers use --remote-kind hosted with this API/session/lease boundary and no local bucket keys"
    );
    println!("Direct S3 smoke: trusted operators may pair this prefix with locally supplied --s3-* env names");
}

fn cli_capabilities_to_string(capabilities: &[ManagedObjectCapability]) -> String {
    capabilities
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn mock_credential_reference(lease_id: &str, generation: u64) -> String {
    format!("mock-managed-object-ref:{lease_id}:generation-{generation}")
}

fn mock_credential_fingerprint_reference(lease_id: &str, generation: u64) -> String {
    format!("mock-fingerprint-ref:{lease_id}:generation-{generation}")
}

fn stable_secret_policy_rule_id(project_id: &str, path: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"bindhub-secret-policy-rule-v1\n");
    hasher.update(project_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(path.as_bytes());
    format!("secret-policy-b3-{}", hasher.finalize().to_hex())
}

fn print_secret_policy_rule(rule: &SecretPolicyRuleRecord) {
    println!("Policy rule id: {}", rule.id);
    println!("Project id: {}", rule.project_id);
    println!("Path: {}", path_to_store_string(&rule.path));
    println!("Action: {}", rule.action.as_str());
    println!(
        "Envelope reference: {}",
        rule.envelope_ref.as_deref().unwrap_or("-")
    );
    println!("Note: {}", rule.note.as_deref().unwrap_or("-"));
    println!("Updated at: {}", rule.updated_at);
}

fn validate_non_secret_reference(value: &str, field: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if trimmed.contains("-----BEGIN ")
        || trimmed.to_ascii_lowercase().contains("bearer ")
        || trimmed
            .to_ascii_lowercase()
            .contains("aws_secret_access_key")
        || [
            "sk-",
            "sk_live_",
            "sk_test_",
            "ghp_",
            "github_pat_",
            "AKIA",
            "ASIA",
        ]
        .iter()
        .any(|marker| trimmed.contains(marker))
    {
        return Err(format!("{field} must not contain secret-looking material"));
    }
    Ok(())
}

fn dev_mock_account_id(value: &str) -> String {
    let suffix = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    format!("account-managed-{}", suffix)
}

fn sync_object_key(
    blob_id: &BlobId,
    explicit_key: Option<&str>,
) -> Result<ObjectKey, Box<dyn std::error::Error>> {
    match explicit_key {
        Some(key) => Ok(ObjectKey::new(key)?),
        None => Ok(encrypted_blob_object_key(blob_id)),
    }
}

fn snapshot_dry_run(cache_root: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    preflight_cache_root(Path::new(cache_root), Path::new(path))?;
    let cache = BlobCache::open(cache_root)?;
    let snapshot = SnapshotManifestBuilder::new(cache).build_draft(path)?;
    let summary = snapshot.summary();

    println!("Snapshot root: {}", snapshot.root().display());
    println!("Draft snapshot id: {}", snapshot.id());
    println!("Manifest entries: {}", summary.total_entries());
    println!("Included files: {}", summary.included_files());
    println!("Included directories: {}", summary.included_directories());
    println!("Included symlinks: {}", summary.included_symlinks());
    println!("Policy exclusions: {}", summary.excluded_entries());
    println!("Blocked secrets: {}", summary.blocked_secret_entries());
    print_draft_blocked_secret_entries(snapshot.entries());
    println!("Included file bytes: {}", summary.total_file_bytes());
    println!("SQLite persistence: deferred");

    Ok(())
}

fn snapshot_create(args: &SnapshotCreateArgs) -> Result<(), Box<dyn std::error::Error>> {
    preflight_cache_root(Path::new(&args.cache_root), Path::new(&args.path))?;
    let db_path = args
        .db_path
        .as_deref()
        .expect("persistent snapshot args require a db path");
    preflight_db_path(Path::new(db_path), Path::new(&args.path))?;

    let cache = BlobCache::open(&args.cache_root)?;
    let snapshot = SnapshotManifestBuilder::new(cache).build_draft(&args.path)?;

    let mut store = Store::open_file(db_path)?;
    store.apply_migrations()?;
    let created_at = store.current_timestamp()?;
    let project_id = local_project_id(snapshot.root());
    let project_id = project_id.to_string();
    let root_path = snapshot.root().display().to_string();
    let display_name = snapshot
        .root()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root_path.clone());
    let project_kind = project_kind_for_root(snapshot.root());
    let snapshot_id = snapshot.id().to_string();
    let reason = "manual";
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
    let draft = NewSnapshotDraft {
        project: NewProject {
            id: &project_id,
            root_path: &root_path,
            kind: &project_kind,
            display_name: &display_name,
            discovered_at: &created_at,
        },
        snapshot: NewSnapshot {
            id: &snapshot_id,
            project_id: &project_id,
            parent_snapshot_id: None,
            created_at: &created_at,
            reason,
            manifest_entry_count: snapshot.summary().total_entries() as u64,
            total_size_bytes: snapshot.summary().total_file_bytes(),
        },
        entries,
    };

    let persisted = store.persist_draft_snapshot(&draft)?;
    print_persisted_snapshot_summary(&persisted, db_path, &args.cache_root);

    Ok(())
}

fn changes_scan(args: &ChangesScanArgs) -> Result<(), Box<dyn std::error::Error>> {
    let scan = scan_local_change_feed(&LocalChangeFeedScanOptions::new(
        &args.db_path,
        &args.cache_root,
        &args.path,
    ))?;
    let db_path = scan.db_path().display().to_string();
    let cache_root = scan.cache_root().display().to_string();
    print_changes_scan_summary(
        scan.project_id(),
        scan.base_snapshot_id(),
        scan.summary(),
        scan.pending_operations(),
        &db_path,
        &cache_root,
    );

    Ok(())
}

fn conflicts_compare(args: &ConflictCompareArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let base = args
        .base_snapshot_id
        .as_deref()
        .map(|id| load_snapshot(&store, id))
        .transpose()?;
    let local = load_snapshot(&store, &args.local_snapshot_id)?;
    let incoming = load_snapshot(&store, &args.incoming_snapshot_id)?;
    let base_comparable = base.as_ref().map(snapshot_to_comparable);
    let local_comparable = snapshot_to_comparable(&local);
    let incoming_comparable = snapshot_to_comparable(&incoming);
    let comparison = compare_snapshots(
        base_comparable.as_ref(),
        &local_comparable,
        &incoming_comparable,
    )?;
    let created_at = store.current_timestamp()?;
    let new_rows = comparison
        .rows()
        .iter()
        .map(new_conflict_row)
        .collect::<Vec<_>>();
    let persisted = store.persist_conflict(
        &NewConflict {
            id: comparison.conflict_id(),
            project_id: comparison.project_id(),
            base_snapshot_id: comparison.base_snapshot_id(),
            local_snapshot_id: comparison.local_snapshot_id(),
            incoming_snapshot_id: comparison.incoming_snapshot_id(),
            summary: comparison.summary(),
            created_at: &created_at,
        },
        &new_rows,
    )?;

    print_conflict_detail(
        &persisted.conflict,
        &persisted.rows,
        Some("Conflict compare: divergent snapshots"),
    );

    Ok(())
}

fn conflicts_list(args: &ConflictListArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let conflicts = store.list_conflicts(args.project_id.as_deref())?;

    println!(
        "Conflict id\tStatus\tProject id\tBase snapshot id\tLocal snapshot id\tIncoming snapshot id\tRows\tDifferent\tPolicy/deferred\tCreated at"
    );
    for conflict in conflicts {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            conflict.id,
            conflict.status.as_str(),
            conflict.project_id,
            conflict.base_snapshot_id.as_deref().unwrap_or("-"),
            conflict.local_snapshot_id,
            conflict.incoming_snapshot_id,
            conflict.row_count,
            conflict.both_modified_different_count
                + conflict.local_only_count
                + conflict.incoming_only_count
                + conflict.local_deleted_count
                + conflict.incoming_deleted_count,
            conflict.policy_excluded_count
                + conflict.policy_deferred_count
                + conflict.policy_blocked_count
                + conflict.unsupported_count,
            conflict.created_at
        );
    }

    Ok(())
}

fn conflicts_show(args: &ConflictShowArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let conflict = store
        .conflict_with_rows(&args.conflict_id)?
        .ok_or_else(|| format!("conflict not found: {}", args.conflict_id))?;

    print_conflict_detail(&conflict.conflict, &conflict.rows, None);

    Ok(())
}

fn conflicts_update_status(
    args: &ConflictShowArgs,
    status: ConflictStatus,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let updated_at = store.current_timestamp()?;
    let conflict = store
        .update_conflict_status(&args.conflict_id, status, &updated_at)?
        .ok_or_else(|| format!("conflict not found: {}", args.conflict_id))?;

    println!("Conflict id: {}", conflict.id);
    println!("Status: {}", conflict.status.as_str());
    println!("Updated at: {}", conflict.updated_at);

    Ok(())
}

fn conflicts_resolve(args: &ConflictResolveArgs) -> Result<(), Box<dyn std::error::Error>> {
    if !args.confirm_no_auto_apply {
        return Err(
            "conflicts resolve requires --confirm-no-auto-apply after manual review".into(),
        );
    }

    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let existing = store
        .conflict_with_rows(&args.conflict_id)?
        .ok_or_else(|| format!("conflict not found: {}", args.conflict_id))?;
    if existing.conflict.status != ConflictStatus::Open {
        return Err(format!(
            "conflict {} is {}; only open conflicts can be manually resolved",
            existing.conflict.id,
            existing.conflict.status.as_str()
        )
        .into());
    }

    let updated_at = store.current_timestamp()?;
    let conflict = store
        .update_conflict_status(&args.conflict_id, ConflictStatus::Resolved, &updated_at)?
        .ok_or_else(|| format!("conflict not found: {}", args.conflict_id))?;

    println!("Conflict id: {}", conflict.id);
    println!("Status: {}", conflict.status.as_str());
    println!("Manual resolution: {}", args.manual_resolution.as_str());
    println!("Rows reviewed: {}", existing.rows.len());
    println!("Updated at: {}", conflict.updated_at);
    println!("Automatic apply: not performed");
    println!("Source file contents: not printed");

    Ok(())
}

fn secret_policy_add(args: &SecretPolicyAddArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let project = store
        .project(&args.project_id)?
        .ok_or_else(|| format!("project not found: {}", args.project_id))?;
    let created_at = store.current_timestamp()?;
    let id = stable_secret_policy_rule_id(&args.project_id, &args.path);
    let rule = store.upsert_secret_policy_rule(&NewSecretPolicyRule {
        id: &id,
        project_id: &project.id,
        path: Path::new(&args.path),
        action: args.action,
        envelope_ref: args.envelope_ref.as_deref(),
        note: args.note.as_deref(),
        created_at: &created_at,
    })?;

    println!("Secret policy: upserted");
    print_secret_policy_rule(&rule);
    println!("Raw secret material: not stored or printed");
    println!("Boundary: local alpha policy record only; hosted/team policy remains deferred");

    Ok(())
}

fn secret_policy_list(args: &SecretPolicyListArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let rules = store.list_secret_policy_rules(args.project_id.as_deref())?;

    println!("Project id\tPath\tAction\tEnvelope ref\tNote\tUpdated at");
    for rule in rules {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            rule.project_id,
            path_to_store_string(&rule.path),
            rule.action.as_str(),
            rule.envelope_ref.as_deref().unwrap_or("-"),
            rule.note.as_deref().unwrap_or("-"),
            rule.updated_at
        );
    }

    Ok(())
}

fn changes_list(args: &ChangesListArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let changes = store.list_pending_local_changes(args.project_id.as_deref())?;

    println!(
        "Project id\tBase snapshot id\tChange\tPath\tBytes\tBlob id\tPrevious blob id\tDetected at"
    );
    for change in &changes {
        print_pending_change(change);
    }

    Ok(())
}

fn changes_clear(args: &ChangesListArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let cleared = store.clear_pending_local_changes(args.project_id.as_deref())?;

    println!("Cleared pending changes: {cleared}");

    Ok(())
}

fn snapshot_restore(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_snapshot_restore_args(args).map_err(|message| {
        format!(
            "{message}\nUsage: bindhub snapshot restore --db <DB_PATH> --cache <CACHE_ROOT> --to <TARGET_DIR> <SNAPSHOT_ID> [--dry-run|--apply]"
        )
    })?;

    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let persisted = store
        .snapshot_with_entries(&args.snapshot_id)?
        .ok_or_else(|| format!("snapshot not found: {}", args.snapshot_id))?;
    let cache = BlobCache::open(&args.cache_root)?;
    let plan = RestorePlan::from_persisted_snapshot(&persisted, &cache, &args.target)?;

    if !args.apply {
        print_restore_plan(&plan);
        return Ok(());
    }

    if !plan.apply_allowed() {
        return Err(restore_block_reason(&plan).into());
    }

    let summary = RestoreMaterializer::new(cache).apply(&plan)?;
    print_restore_apply_result(&plan, summary.bytes_to_write);

    Ok(())
}

fn print_changes_scan_summary(
    project_id: &str,
    base_snapshot_id: Option<&str>,
    summary: &bindhub_snapshot::LocalChangeSummary,
    pending_operations: usize,
    db_path: &str,
    cache_root: &str,
) {
    println!("Project id: {project_id}");
    println!("Base snapshot id: {}", base_snapshot_id.unwrap_or("-"));
    println!("Created: {}", summary.created());
    println!("Modified: {}", summary.modified());
    println!("Deleted: {}", summary.deleted());
    println!("Unchanged: {}", summary.unchanged());
    println!("Skipped/deferred: {}", summary.skipped_deferred());
    println!("Pending upload bytes: {}", summary.bytes_to_upload());
    println!("Deleted bytes: {}", summary.bytes_deleted());
    println!("Pending operations: {pending_operations}");
    println!("SQLite database: {db_path}");
    println!("Blob cache: {cache_root}");
}

fn print_pending_change(change: &PendingLocalChangeRecord) {
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        change.project_id,
        change.base_snapshot_id.as_deref().unwrap_or("-"),
        local_change_kind_name(&change.change_kind),
        path_to_store_string(&change.relative_path),
        change.size_bytes,
        change
            .blob_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "-".to_string()),
        change
            .previous_blob_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "-".to_string()),
        change.detected_at,
    );
}

fn print_device_trust(device: &DeviceTrustRecord) {
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        device.device_id,
        device.account_id,
        device.is_local,
        device.display_name,
        device.trust_state,
        device.approved_at.as_deref().unwrap_or("-"),
        device.revoked_at.as_deref().unwrap_or("-"),
        device.last_seen_at,
    );
}

fn identity_view(identity: &LocalIdentityRecord) -> LocalIdentityView {
    LocalIdentityView {
        account_id: identity.account_id.clone(),
        device_id: identity.device_id.clone(),
        device_name: identity.device_name.clone(),
        sync_key_hex: identity.sync_key_hex.clone(),
    }
}

fn local_change_kind_name(kind: &LocalChangeKind) -> &'static str {
    kind.as_str()
}

fn snapshot_list(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [flag, db_path] = args else {
        return Err("Usage: bindhub snapshot list --db <DB_PATH>".into());
    };
    if flag != "--db" {
        return Err("Usage: bindhub snapshot list --db <DB_PATH>".into());
    }

    let store = open_existing_metadata_store(db_path)?;
    store.apply_migrations()?;
    let snapshots = store.list_snapshots()?;

    println!("Snapshot id\tCreated at\tProject\tEntries\tBytes");
    for snapshot in snapshots {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            snapshot.id,
            snapshot.created_at,
            snapshot.project_root_path,
            snapshot.manifest_entry_count,
            snapshot.total_size_bytes
        );
    }

    Ok(())
}

fn snapshot_show(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [flag, db_path, snapshot_id] = args else {
        return Err("Usage: bindhub snapshot show --db <DB_PATH> <SNAPSHOT_ID>".into());
    };
    if flag != "--db" {
        return Err("Usage: bindhub snapshot show --db <DB_PATH> <SNAPSHOT_ID>".into());
    }

    let store = open_existing_metadata_store(db_path)?;
    store.apply_migrations()?;
    let persisted = store
        .snapshot_with_entries(snapshot_id)?
        .ok_or_else(|| format!("snapshot not found: {snapshot_id}"))?;

    print_snapshot_detail(&persisted);

    Ok(())
}

fn project_kind_for_root(root: &Path) -> String {
    ProjectScanner
        .scan_path(root)
        .ok()
        .and_then(|scan| {
            scan.projects()
                .iter()
                .find(|project| project.relative_path().as_os_str().is_empty())
                .or_else(|| scan.projects().first())
                .map(|project| project.kind().to_string())
        })
        .unwrap_or_else(|| "local".to_string())
}

fn print_persisted_snapshot_summary(
    persisted: &PersistedSnapshot,
    db_path: &str,
    cache_root: &str,
) {
    let (
        included_files,
        included_directories,
        included_symlinks,
        deferred_entries,
        excluded,
        blocked_secrets,
    ) = summarize_entries(&persisted.entries);

    println!("Snapshot id: {}", persisted.snapshot.id);
    println!("Project id: {}", persisted.project.id);
    println!("Project path: {}", persisted.project.root_path);
    println!("Project name: {}", persisted.project.display_name);
    println!("Created at: {}", persisted.snapshot.created_at);
    println!(
        "Manifest entries: {}",
        persisted.snapshot.manifest_entry_count
    );
    println!("Included files: {included_files}");
    println!("Included directories: {included_directories}");
    println!("Included symlinks: {included_symlinks}");
    println!("Policy exclusions: {excluded}");
    println!("Deferred entries: {deferred_entries}");
    println!("Blocked secrets: {blocked_secrets}");
    print_persisted_blocked_secret_entries(&persisted.entries);
    println!(
        "Included file bytes: {}",
        persisted.snapshot.total_size_bytes
    );
    println!("SQLite database: {db_path}");
    println!("Blob cache: {cache_root}");
}

fn print_draft_blocked_secret_entries(entries: &[SnapshotManifestEntry]) {
    for entry in entries {
        if let PolicyDecision::RequiresUserDecision { reason } = entry.policy_decision() {
            if is_secret_block_reason(reason) {
                println!(
                    "SECRET\t{}\t{}",
                    path_to_store_string(entry.relative_path()),
                    reason
                );
            }
        }
    }
}

fn print_persisted_blocked_secret_entries(entries: &[ManifestEntryRecord]) {
    for entry in entries {
        if let PolicyDecision::RequiresUserDecision { reason } = &entry.policy_decision {
            if is_secret_block_reason(reason) {
                println!(
                    "SECRET\t{}\t{}",
                    path_to_store_string(&entry.relative_path),
                    reason
                );
            }
        }
    }
}

fn print_snapshot_detail(persisted: &PersistedSnapshot) {
    let (
        included_files,
        included_directories,
        included_symlinks,
        deferred_entries,
        excluded,
        blocked_secrets,
    ) = summarize_entries(&persisted.entries);

    println!("Snapshot id: {}", persisted.snapshot.id);
    println!("Project id: {}", persisted.project.id);
    println!("Project path: {}", persisted.project.root_path);
    println!("Project name: {}", persisted.project.display_name);
    println!("Created at: {}", persisted.snapshot.created_at);
    println!(
        "Manifest entries: {}",
        persisted.snapshot.manifest_entry_count
    );
    println!("Included files: {included_files}");
    println!("Included directories: {included_directories}");
    println!("Included symlinks: {included_symlinks}");
    println!("Policy exclusions: {excluded}");
    println!("Deferred entries: {deferred_entries}");
    println!("Blocked secrets: {blocked_secrets}");
    println!(
        "Included file bytes: {}",
        persisted.snapshot.total_size_bytes
    );
    println!("Entries:");
    println!("Path\tKind\tDecision\tBytes\tBlob id\tObject ref\tReason");
    for entry in &persisted.entries {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            path_to_store_string(&entry.relative_path),
            manifest_kind_name(entry),
            policy_decision_name(&entry.policy_decision),
            entry.size_bytes,
            entry
                .blob_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "-".to_string()),
            entry.object_ref.as_deref().unwrap_or("-"),
            policy_reason(&entry.policy_decision).unwrap_or("-")
        );
    }
}

fn load_snapshot(store: &Store, id: &str) -> Result<PersistedSnapshot, Box<dyn std::error::Error>> {
    store
        .snapshot_with_entries(id)?
        .ok_or_else(|| format!("snapshot not found: {id}").into())
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

fn print_conflict_detail(
    conflict: &bindhub_store::ConflictRecord,
    rows: &[ConflictRowRecord],
    title: Option<&str>,
) {
    if let Some(title) = title {
        println!("{title}");
    }
    println!("Conflict id: {}", conflict.id);
    println!("Status: {}", conflict.status.as_str());
    println!("Project id: {}", conflict.project_id);
    println!(
        "Base snapshot id: {}",
        conflict.base_snapshot_id.as_deref().unwrap_or("-")
    );
    println!("Local snapshot id: {}", conflict.local_snapshot_id);
    println!("Incoming snapshot id: {}", conflict.incoming_snapshot_id);
    println!("Created at: {}", conflict.created_at);
    println!("Updated at: {}", conflict.updated_at);
    println!("Rows: {}", conflict.row_count);
    println!("Same: {}", conflict.same_count);
    println!("Local only: {}", conflict.local_only_count);
    println!("Incoming only: {}", conflict.incoming_only_count);
    println!("Local deleted: {}", conflict.local_deleted_count);
    println!("Incoming deleted: {}", conflict.incoming_deleted_count);
    println!("Both modified same: {}", conflict.both_modified_same_count);
    println!(
        "Both modified different: {}",
        conflict.both_modified_different_count
    );
    println!("Policy excluded: {}", conflict.policy_excluded_count);
    println!("Policy deferred: {}", conflict.policy_deferred_count);
    println!("Policy blocked: {}", conflict.policy_blocked_count);
    println!("Unsupported: {}", conflict.unsupported_count);
    println!("Entries:");
    println!(
        "Path\tState\tKind\tBase blob id\tLocal blob id\tIncoming blob id\tBase bytes\tLocal bytes\tIncoming bytes\tLocal decision\tIncoming decision\tReason"
    );
    for row in rows {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            path_to_conflict_string(&row.relative_path),
            row.state.as_str(),
            manifest_kind_value(&row.entry_kind),
            optional_blob_id(row.base_blob_id.as_ref()),
            optional_blob_id(row.local_blob_id.as_ref()),
            optional_blob_id(row.incoming_blob_id.as_ref()),
            optional_u64(row.base_size_bytes),
            optional_u64(row.local_size_bytes),
            optional_u64(row.incoming_size_bytes),
            optional_policy_decision(row.local_policy_decision.as_ref()),
            optional_policy_decision(row.incoming_policy_decision.as_ref()),
            conflict_row_reason(row).unwrap_or("-")
        );
    }
}

fn print_sync_preflight_outcome(outcome: &SyncPreflightOutcome) {
    println!("Preflight: {}", outcome.status.as_str());
    println!("Project id: {}", outcome.project_id);
    println!(
        "Base snapshot id: {}",
        outcome.base_snapshot_id.as_deref().unwrap_or("-")
    );
    println!(
        "Local snapshot id: {}",
        outcome.local_snapshot_id.as_deref().unwrap_or("-")
    );
    println!("Incoming snapshot id: {}", outcome.incoming_snapshot_id);

    if let Some(conflict) = &outcome.conflict {
        println!("Conflict id: {}", conflict.conflict.id);
        println!("Rows: {}", conflict.conflict.row_count);
        println!("Same: {}", conflict.conflict.same_count);
        println!("Local only: {}", conflict.conflict.local_only_count);
        println!("Incoming only: {}", conflict.conflict.incoming_only_count);
        println!("Local deleted: {}", conflict.conflict.local_deleted_count);
        println!(
            "Incoming deleted: {}",
            conflict.conflict.incoming_deleted_count
        );
        println!(
            "Both modified same: {}",
            conflict.conflict.both_modified_same_count
        );
        println!(
            "Both modified different: {}",
            conflict.conflict.both_modified_different_count
        );
        println!(
            "Policy excluded: {}",
            conflict.conflict.policy_excluded_count
        );
        println!(
            "Policy deferred: {}",
            conflict.conflict.policy_deferred_count
        );
        println!("Policy blocked: {}", conflict.conflict.policy_blocked_count);
        println!("Unsupported: {}", conflict.conflict.unsupported_count);
    } else {
        println!("Conflict id: -");
        println!("Rows: 0");
        println!("Same: 0");
        println!("Local only: 0");
        println!("Incoming only: 0");
        println!("Local deleted: 0");
        println!("Incoming deleted: 0");
        println!("Both modified same: 0");
        println!("Both modified different: 0");
        println!("Policy excluded: 0");
        println!("Policy deferred: 0");
        println!("Policy blocked: 0");
        println!("Unsupported: 0");
    }
}

fn optional_blob_id(blob_id: Option<&BlobId>) -> String {
    blob_id
        .map(ToString::to_string)
        .unwrap_or_else(|| "-".to_string())
}

fn optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn optional_policy_decision(policy: Option<&PolicyDecision>) -> &'static str {
    policy.map(policy_decision_name).unwrap_or("-")
}

fn conflict_row_reason(row: &ConflictRowRecord) -> Option<&str> {
    row.local_policy_decision
        .as_ref()
        .and_then(policy_reason)
        .or_else(|| {
            row.incoming_policy_decision
                .as_ref()
                .and_then(policy_reason)
        })
}

fn print_restore_plan(plan: &RestorePlan) {
    println!("Restore mode: dry-run");
    println!("Snapshot id: {}", plan.snapshot_id());
    println!("Target: {}", plan.target().display());
    println!("Target status: {}", plan.target_status().as_str());
    println!("Apply allowed: {}", plan.apply_allowed());
    println!("Bytes to write: {}", plan.total_bytes());
    println!("Directories to create: {}", plan.dirs_to_create().len());
    for dir in plan.dirs_to_create() {
        println!("DIR\t{}", path_to_store_string(dir));
    }
    println!("Files to write: {}", plan.files_to_write().len());
    for file in plan.files_to_write() {
        print_restore_file("FILE", file);
    }
    println!("Skipped entries: {}", plan.skipped_entries().len());
    for entry in plan.skipped_entries() {
        print_restore_skip(entry);
    }
    println!("Missing blobs: {}", plan.missing_blobs().len());
    for missing in plan.missing_blobs() {
        println!(
            "MISSING\t{}\t{}\t{}\t{}",
            path_to_store_string(&missing.path),
            missing
                .blob_id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "-".to_string()),
            missing.object_ref.as_deref().unwrap_or("-"),
            missing.reason
        );
    }
}

fn print_restore_apply_result(plan: &RestorePlan, bytes_written: u64) {
    println!("Restore mode: apply");
    println!("Snapshot id: {}", plan.snapshot_id());
    println!("Target: {}", plan.target().display());
    println!("Directories created: {}", plan.dirs_to_create().len());
    for dir in plan.dirs_to_create() {
        println!("DIR\t{}", path_to_store_string(dir));
    }
    println!("Files written: {}", plan.files_to_write().len());
    for file in plan.files_to_write() {
        print_restore_file("FILE", file);
    }
    println!("Skipped entries: {}", plan.skipped_entries().len());
    for entry in plan.skipped_entries() {
        print_restore_skip(entry);
    }
    println!("Bytes written: {bytes_written}");
}

fn print_restore_file(prefix: &str, file: &RestoreWrite) {
    println!(
        "{}\t{}\t{}\t{}\t{}",
        prefix,
        path_to_store_string(&file.path),
        file.size_bytes,
        file.blob_id,
        file.object_ref
    );
}

fn print_restore_skip(entry: &RestoreSkippedEntry) {
    println!(
        "SKIP\t{}\t{}\t{}\t{}",
        path_to_store_string(&entry.path),
        manifest_kind_value(&entry.kind),
        entry.decision,
        entry.reason
    );
}

fn restore_block_reason(plan: &RestorePlan) -> String {
    if !matches!(
        plan.target_status(),
        RestoreTargetStatus::Missing | RestoreTargetStatus::EmptyDirectory
    ) {
        return format!(
            "restore apply blocked: target must be missing or empty; target status is {}",
            plan.target_status().as_str()
        );
    }

    if !plan.missing_blobs().is_empty() {
        return format!(
            "restore apply blocked: {} blob reference(s) are missing",
            plan.missing_blobs().len()
        );
    }

    "restore apply blocked".to_string()
}

fn summarize_entries(
    entries: &[ManifestEntryRecord],
) -> (usize, usize, usize, usize, usize, usize) {
    let mut included_files = 0;
    let mut included_directories = 0;
    let mut included_symlinks = 0;
    let mut deferred_entries = 0;
    let mut excluded_entries = 0;
    let mut blocked_secret_entries = 0;

    for entry in entries {
        match &entry.policy_decision {
            PolicyDecision::Include => match entry.kind {
                bindhub_core::ManifestEntryKind::File => included_files += 1,
                bindhub_core::ManifestEntryKind::Directory => included_directories += 1,
                bindhub_core::ManifestEntryKind::Symlink => included_symlinks += 1,
                bindhub_core::ManifestEntryKind::Unsupported => deferred_entries += 1,
            },
            PolicyDecision::Exclude { .. } => excluded_entries += 1,
            PolicyDecision::RequiresUserDecision { reason } => {
                deferred_entries += 1;
                if is_secret_block_reason(reason) {
                    blocked_secret_entries += 1;
                }
            }
        }
    }

    (
        included_files,
        included_directories,
        included_symlinks,
        deferred_entries,
        excluded_entries,
        blocked_secret_entries,
    )
}

fn manifest_kind_name(entry: &ManifestEntryRecord) -> &'static str {
    manifest_kind_value(&entry.kind)
}

fn manifest_kind_value(kind: &ManifestEntryKind) -> &'static str {
    match kind {
        ManifestEntryKind::File => "file",
        ManifestEntryKind::Directory => "directory",
        ManifestEntryKind::Symlink => "symlink",
        ManifestEntryKind::Unsupported => "unsupported",
    }
}

fn policy_decision_name(policy: &PolicyDecision) -> &'static str {
    match policy {
        PolicyDecision::Include => "include",
        PolicyDecision::Exclude { .. } => "exclude",
        PolicyDecision::RequiresUserDecision { .. } => "requires_user_decision",
    }
}

fn policy_reason(policy: &PolicyDecision) -> Option<&str> {
    match policy {
        PolicyDecision::Include => None,
        PolicyDecision::Exclude { reason } | PolicyDecision::RequiresUserDecision { reason } => {
            Some(reason)
        }
    }
}

fn print_snapshot_usage() {
    eprintln!("Usage:");
    eprintln!("  bindhub snapshot --cache <CACHE_ROOT> --dry-run <PATH>");
    eprintln!("  bindhub snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PATH>");
    eprintln!("  bindhub snapshot list --db <DB_PATH>");
    eprintln!("  bindhub snapshot show --db <DB_PATH> <SNAPSHOT_ID>");
    eprintln!(
        "  bindhub snapshot restore --db <DB_PATH> --cache <CACHE_ROOT> --to <TARGET_DIR> <SNAPSHOT_ID> [--dry-run|--apply]"
    );
}

fn print_changes_usage() {
    eprintln!("Usage:");
    eprintln!("  bindhub changes scan --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>");
    eprintln!("  bindhub changes list --db <DB_PATH> [--project <PROJECT_ID>]");
    eprintln!("  bindhub changes clear --db <DB_PATH> [--project <PROJECT_ID>]");
}

fn print_conflicts_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  bindhub conflicts compare --db <DB_PATH> --local <LOCAL_SNAPSHOT_ID> --incoming <INCOMING_SNAPSHOT_ID> [--base <BASE_SNAPSHOT_ID>]"
    );
    eprintln!("  bindhub conflicts list --db <DB_PATH> [--project <PROJECT_ID>]");
    eprintln!("  bindhub conflicts show --db <DB_PATH> <CONFLICT_ID>");
    eprintln!(
        "  bindhub conflicts resolve --db <DB_PATH> <CONFLICT_ID> --manual-resolution keep-local|keep-incoming|keep-both|exported --confirm-no-auto-apply"
    );
    eprintln!("  bindhub conflicts dismiss --db <DB_PATH> <CONFLICT_ID>");
}

fn print_secrets_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  bindhub secrets policy add --db <DB_PATH> --project <PROJECT_ID> --path <REL_PATH> --action block|template|envelope [--envelope-ref <REF>] [--note <TEXT>]"
    );
    eprintln!("  bindhub secrets policy list --db <DB_PATH> [--project <PROJECT_ID>]");
    eprintln!(
        "  Secret policy commands are local/no-network alpha records; raw secret material is never accepted as a policy value."
    );
}

fn print_devices_usage() {
    eprintln!("Usage:");
    eprintln!("  bindhub devices list --db <DB_PATH>");
    eprintln!("  bindhub devices invite --db <DB_PATH> [--ttl-seconds <SECONDS>]");
    eprintln!("  bindhub devices approve --db <DB_PATH> --token <TOKEN> --device-name <NAME>");
    eprintln!("  bindhub devices join --db <RECEIVER_DB> --token-env <ENV>|--token <TOKEN> --device-name <NAME>");
    eprintln!("  bindhub devices approve-join --db <SOURCE_DB> --token-env <ENV>|--token <TOKEN> --join-request-env <ENV>|--join-request <REQUEST> --device-name <NAME>");
    eprintln!("  bindhub devices complete --db <RECEIVER_DB> --completion-env <ENV>|--completion <COMPLETION>");
    eprintln!("  bindhub devices revoke --db <DB_PATH> <DEVICE_ID> [--reason <TEXT>]");
    eprintln!(
        "  bindhub devices recovery create --db <DB_PATH> --device <DEVICE_ID> --recovery-ref <REDACTED_REF> [--audit-label <TEXT>] [--ttl-seconds <SECONDS>]"
    );
    eprintln!("  bindhub devices recovery revoke --db <DB_PATH> <GRANT_ID>");
    eprintln!(
        "  bindhub devices rotate-key-envelope --db <DB_PATH> --device <DEVICE_ID> [--session-id <SESSION_ID>] [--reason <TEXT>] [--ttl-seconds <SECONDS>]"
    );
}

fn print_sync_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  bindhub sync publish-snapshot --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <SNAPSHOT_ID>"
    );
    eprintln!(
        "  bindhub sync import-snapshot --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> [--mock-key-source-db <PUBLISHER_DB>] <SNAPSHOT_ID>"
    );
    eprintln!(
        "  bindhub sync materialize --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> --to <TARGET_DIR> [--mock-key-source-db <PUBLISHER_DB>] <SNAPSHOT_ID> [--dry-run|--apply]"
    );
    eprintln!(
        "  bindhub sync preflight --db <DB_PATH> --project <PROJECT_ID> --local <LOCAL_SNAPSHOT_ID> --incoming <INCOMING_SNAPSHOT_ID> [--base <BASE_SNAPSHOT_ID>]"
    );
    eprintln!(
        "  bindhub sync upload --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <BLOB_ID> [--object-key <KEY>]"
    );
    eprintln!(
        "  bindhub sync download --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <BLOB_ID> [--object-key <KEY>]"
    );
    eprintln!("  bindhub sync remote check --remote <REMOTE_DIR> [--validate-only]");
    eprintln!(
        "  bindhub sync remote check --remote-kind s3 --s3-endpoint <URL> --s3-bucket <BUCKET> [--s3-region <REGION>] [--s3-prefix <PREFIX>] [--s3-access-key-env <ENV> --s3-secret-key-env <ENV>] [--s3-session-token-env <ENV>] [--validate-only]"
    );
    eprintln!(
        "  bindhub sync remote check --remote-kind hosted --object-access-api <URL> --object-access-project <PROJECT_ID> --object-access-lease <LEASE_ID> [--object-access-session-token-env BINDHUB_SESSION_TOKEN] [--validate-only]"
    );
    eprintln!(
        "  Add --remote-kind hosted plus --object-access-* flags to publish-snapshot, import-snapshot, materialize, upload, or download without client bucket keys."
    );
    eprintln!(
        "  Add --remote-kind s3 plus the --s3-* flags above only for trusted-operator direct S3/R2 smoke."
    );
    eprintln!(
        "  Add --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> to publish-snapshot for hosted mock-dev metadata registration."
    );
    eprintln!(
        "  Add --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> to import-snapshot or materialize for hosted mock-dev manifest discovery and cursor CAS."
    );
    eprintln!(
        "  Add --metadata-mode hosted-api --metadata-api <URL> [--metadata-session-token-env BINDHUB_SESSION_TOKEN] for external hosted metadata registration, latest discovery, and cursor CAS."
    );
    eprintln!(
        "  Local/mock import/materialize metadata account scope is --metadata-account <ACCOUNT_ID>, or it is derived from --mock-key-source-db <PUBLISHER_DB> in the local/mock trust bootstrap path; hosted-api derives account from the authenticated session."
    );
    eprintln!(
        "  Optional --metadata-endpoint <URL> validates and prints a sanitized label only for mock-dev SQLite mode; hosted-api uses --metadata-api."
    );
    eprintln!(
        "  bindhub sync cursor get --db <DB_PATH> --project <PROJECT_ID> [--device <DEVICE_ID>]"
    );
    eprintln!(
        "  bindhub sync cursor set --db <DB_PATH> --project <PROJECT_ID> --value <CURSOR> [--device <DEVICE_ID>]"
    );
}

fn print_metadata_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  bindhub metadata check --endpoint <URL> [--auth-mode mock-dev-headers|account-session]"
    );
    eprintln!(
        "  bindhub metadata alpha-invite create (--db <METADATA_DB>|--postgres-url-env <ENV>) --email <EMAIL>|--domain <DOMAIN> [--invite-code <CODE>] [--ttl-seconds <SECONDS>]"
    );
    eprintln!(
        "  bindhub metadata credential-lease mock-create (--db <METADATA_DB>|--postgres-url-env <ENV>) --session-token <TOKEN> --verified-email <EMAIL>|--verified-domain <DOMAIN> --project <PROJECT_ID> --lease <LEASE_ID> --endpoint <URL> --bucket <BUCKET> [--provider-kind r2|s3|minio-compatible] [--region <REGION>] [--prefix <PREFIX>] [--capabilities read,write,list,head] [--ttl-seconds <SECONDS>]"
    );
    eprintln!(
        "  bindhub metadata credential-lease check (--db <METADATA_DB>|--postgres-url-env <ENV>) --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID> [--require-capabilities read,head]"
    );
    eprintln!(
        "  bindhub metadata credential-lease rotate (--db <METADATA_DB>|--postgres-url-env <ENV>) --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>"
    );
    eprintln!(
        "  bindhub metadata credential-lease revoke (--db <METADATA_DB>|--postgres-url-env <ENV>) --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>"
    );
    eprintln!(
        "  bindhub metadata object-access resolve --api <URL> [--session-token-env BINDHUB_SESSION_TOKEN] --project <PROJECT_ID> --lease <LEASE_ID> [--require-capabilities read,write,list,head]"
    );
    eprintln!(
        "  Use --postgres-url-env DATABASE_URL for Railway/Postgres admin seeding; raw database URLs are intentionally not accepted on argv. Credential-lease commands are no-network mock/dev smoke commands. Object-access resolve calls the hosted API and returns a redacted server-mediated shared-bucket prefix grant."
    );
}

fn open_existing_metadata_store(db_path: &str) -> Result<Store, Box<dyn std::error::Error>> {
    let path = Path::new(db_path);
    if !path.is_file() {
        return Err(format!("metadata database does not exist: {}", path.display()).into());
    }

    Ok(Store::open_file(path)?)
}

fn run_status(args: &[String]) -> ExitCode {
    match args {
        [flag, path] if flag == "--db" => match status_for_db(path) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bindhub: {error}");
                ExitCode::from(1)
            }
        },
        _ => product::run_status(args),
    }
}

fn status_for_db(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::open_file(path)?;
    store.apply_migrations()?;
    let summary = store.schema_summary()?;

    println!("Metadata database: {path}");
    println!("Schema version: {}", summary.version);
    println!("Tables:");
    for table in summary.tables {
        println!("- {}: {}", table.table, table.rows);
    }

    Ok(())
}

fn run_scan(args: &[String]) -> ExitCode {
    if args.len() != 1 {
        eprintln!("bindhub: scan requires exactly one path");
        eprintln!("Usage: bindhub scan <PATH>");
        return ExitCode::from(2);
    }

    let scanner = ProjectScanner;
    match scanner.scan_path(&args[0]) {
        Ok(scan) => {
            println!("Scan root: {}", scan.root().display());
            println!("Projects detected: {}", scan.projects().len());

            for project in scan.projects() {
                println!(
                    "- {} project at {}",
                    project.kind(),
                    display_relative_path(project.relative_path())
                );

                let signals = project
                    .signals()
                    .iter()
                    .map(|signal| signal.path().display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("  signals: {signals}");

                let hints = project
                    .rehydration_hints()
                    .iter()
                    .map(|hint| hint.command())
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("  rehydrate: {hints}");
            }

            let exclusions = scan.excluded_paths().collect::<Vec<_>>();
            println!("Policy exclusions: {}", exclusions.len());
            for evaluation in exclusions {
                if let PolicyDecision::Exclude { reason } = evaluation.decision() {
                    println!(
                        "- {}: {}",
                        display_relative_path(evaluation.relative_path()),
                        reason
                    );
                }
            }

            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("bindhub: {error}");
            ExitCode::from(1)
        }
    }
}

fn display_relative_path(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_string()
    } else {
        path.display().to_string()
    }
}

fn print_help() {
    println!("bindhub {VERSION}");
    println!();
    println!("Usage: bindhub <COMMAND>");
    println!();
    println!("Product commands:");
    println!("  login      Authenticate this machine with Bindhub");
    println!("  share      Share a folder through Bindhub hosted services");
    println!("  clone      Materialize a shared folder on this machine");
    println!("  manage     Manage a shared folder");
    println!("  status     Show shared-folder and machine sync status");
    println!("  warm       Download useful small files for a folder path");
    println!("  hydrate    Download a path or folder exactly");
    println!("  keep       Protect a path from free-space cleanup");
    println!("  free-space Safely remove backed-up local bytes");
    println!("  doctor     Check this machine and point to folder diagnostics");
    println!("  pause      Pause sync for a shared folder");
    println!("  resume     Resume sync for a shared folder");
    println!("  unlink     Remove this machine's link to a shared folder");
    println!();
    println!("Advanced compatibility commands:");
    println!("  scan       Classify a local directory and explain default policy exclusions");
    println!("  init       Initialize local account and current-device identity");
    println!("  auth       Manage local/mock auth and dev account/session proof status");
    println!("  devices    List, invite, approve, and revoke local/mock trusted devices");
    println!("  metadata   Validate hosted metadata config and administer alpha invites/leases");
    println!("  sync       Inspect or repair legacy shared-folder transfer state");
    println!("  snapshot   Build, persist, list, show, and restore local snapshot manifests");
    println!("  changes    Scan, list, and clear the pending local change feed");
    println!("  conflicts  Compare, persist, list, and update divergent snapshot conflicts");
    println!("  secrets    Manage explicit local secret policy records");
    println!("  status --db <PATH>");
    println!("             Inspect local alpha metadata");
    println!("  restore    Placeholder for snapshot restore");
    println!("  explain    Placeholder for policy and sync explanations");
    println!();
    println!("Options:");
    println!("  -h, --help     Print help");
    println!("  -V, --version  Print version");
}

#[cfg(test)]
mod tests {
    use super::*;
    use bindhub_snapshot::SnapshotPreflightError;
    use std::fs;

    #[test]
    fn preflight_rejects_in_tree_cache_without_creating_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).expect("project dir creates");
        let cache_root = root.join("z-cache");

        let error =
            preflight_cache_root(&cache_root, &root).expect_err("in-tree cache root is rejected");

        assert!(matches!(
            error,
            SnapshotPreflightError::CacheInsideSnapshotRoot { .. }
        ));
        assert!(!cache_root.exists());
    }

    #[test]
    fn preflight_allows_outside_cache_without_creating_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).expect("project dir creates");
        let cache_root = dir.path().join("cache");

        preflight_cache_root(&cache_root, &root).expect("outside cache root is accepted");

        assert!(!cache_root.exists());
    }

    #[test]
    fn preflight_rejects_in_tree_db_without_creating_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).expect("project dir creates");
        let db_path = root.join("bindhub.sqlite3");

        let error = preflight_db_path(&db_path, &root).expect_err("in-tree db path is rejected");

        assert!(matches!(
            error,
            SnapshotPreflightError::DatabaseInsideSnapshotRoot { .. }
        ));
        assert!(!db_path.exists());
    }

    #[test]
    fn preflight_allows_outside_db_without_creating_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).expect("project dir creates");
        let db_path = dir.path().join("bindhub.sqlite3");

        preflight_db_path(&db_path, &root).expect("outside db path is accepted");

        assert!(!db_path.exists());
    }

    #[test]
    fn metadata_check_args_default_to_mock_dev_headers() {
        let args = vec![
            "--endpoint".to_string(),
            "http://127.0.0.1:8787".to_string(),
        ];

        let parsed = parse_metadata_check_args(&args).expect("metadata check args parse");

        assert_eq!(parsed.endpoint, "http://127.0.0.1:8787");
        assert_eq!(parsed.auth_mode, MetadataAuthMode::MockDevHeaders);

        let session_args = vec![
            "--endpoint".to_string(),
            "http://127.0.0.1:8787".to_string(),
            "--auth-mode".to_string(),
            "account-session".to_string(),
        ];
        let parsed = parse_metadata_check_args(&session_args).expect("session auth args parse");
        assert_eq!(parsed.auth_mode, MetadataAuthMode::AccountSession);
    }

    #[test]
    fn metadata_check_rejects_secret_like_endpoint_material() {
        let args = MetadataCheckArgs {
            endpoint: "https://metadata.example/sync-key/raw".to_string(),
            auth_mode: MetadataAuthMode::MockDevHeaders,
        };

        let error = metadata_check(&args).expect_err("secret-like config is rejected");

        assert_eq!(
            error.to_string(),
            "metadata endpoint must not contain secret-looking material"
        );
    }

    #[test]
    fn metadata_alpha_invite_create_args_accept_email_or_domain() {
        let email_args = vec![
            "--db".to_string(),
            "metadata.sqlite3".to_string(),
            "--email".to_string(),
            "dev@example.com".to_string(),
            "--ttl-seconds".to_string(),
            "600".to_string(),
        ];
        let parsed =
            parse_metadata_alpha_invite_create_args(&email_args).expect("email invite args parse");
        assert!(matches!(
            parsed.store,
            MetadataAdminStoreSelector::Sqlite { ref db_path } if db_path == "metadata.sqlite3"
        ));
        assert_eq!(parsed.email.as_deref(), Some("dev@example.com"));
        assert_eq!(parsed.domain, None);
        assert_eq!(parsed.ttl_seconds, 600);

        let domain_args = vec![
            "--db".to_string(),
            "metadata.sqlite3".to_string(),
            "--domain".to_string(),
            "example.com".to_string(),
        ];
        let parsed = parse_metadata_alpha_invite_create_args(&domain_args)
            .expect("domain invite args parse");
        assert_eq!(parsed.domain.as_deref(), Some("example.com"));
        assert_eq!(parsed.ttl_seconds, 60 * 60 * 24 * 14);

        let missing_contact = vec!["--db".to_string(), "metadata.sqlite3".to_string()];
        let error = parse_metadata_alpha_invite_create_args(&missing_contact)
            .expect_err("contact scope is required");
        assert_eq!(
            error,
            "metadata alpha-invite create requires --email or --domain"
        );

        let postgres_args = vec![
            "--postgres-url-env".to_string(),
            "DATABASE_URL".to_string(),
            "--email".to_string(),
            "dev@example.com".to_string(),
        ];
        let parsed = parse_metadata_alpha_invite_create_args(&postgres_args)
            .expect("postgres invite args parse");
        assert!(matches!(
            parsed.store,
            MetadataAdminStoreSelector::PostgresUrlEnv { ref env_name } if env_name == "DATABASE_URL"
        ));
        assert!(!format!("{parsed:?}").contains("postgres://"));

        let ambiguous = vec![
            "--db".to_string(),
            "metadata.sqlite3".to_string(),
            "--postgres-url-env".to_string(),
            "DATABASE_URL".to_string(),
            "--email".to_string(),
            "dev@example.com".to_string(),
        ];
        let error = parse_metadata_alpha_invite_create_args(&ambiguous)
            .expect_err("ambiguous admin store is rejected");
        assert_eq!(
            error,
            "metadata alpha-invite create accepts only one of --db <METADATA_DB> or --postgres-url-env <ENV>"
        );
    }

    #[test]
    fn metadata_credential_lease_args_accept_postgres_admin_store_selector() {
        let args = vec![
            "--postgres-url-env".to_string(),
            "DATABASE_URL".to_string(),
            "--session-token".to_string(),
            "raw-session-token".to_string(),
            "--verified-email".to_string(),
            "dev@example.com".to_string(),
            "--project".to_string(),
            "project-bindhub".to_string(),
            "--lease".to_string(),
            "lease-alpha".to_string(),
            "--endpoint".to_string(),
            "https://example.r2.cloudflarestorage.com".to_string(),
            "--bucket".to_string(),
            "bindhub".to_string(),
        ];

        let parsed = parse_metadata_credential_lease_create_args(&args).expect("lease args parse");

        assert!(matches!(
            parsed.store,
            MetadataAdminStoreSelector::PostgresUrlEnv { ref env_name } if env_name == "DATABASE_URL"
        ));
        assert!(!format!("{parsed:?}").contains("raw-session-token"));

        let lookup_args = vec![
            "--postgres-url-env".to_string(),
            "DATABASE_URL".to_string(),
            "--session-token".to_string(),
            "raw-session-token".to_string(),
            "--project".to_string(),
            "project-bindhub".to_string(),
            "--lease".to_string(),
            "lease-alpha".to_string(),
            "--require-capabilities".to_string(),
            "read,head".to_string(),
        ];
        let parsed = parse_metadata_credential_lease_lookup_args(&lookup_args)
            .expect("lease lookup args parse");
        assert_eq!(
            parsed.required_capabilities,
            vec![ManagedObjectCapability::Read, ManagedObjectCapability::Head]
        );
        assert!(matches!(
            parsed.store,
            MetadataAdminStoreSelector::PostgresUrlEnv { ref env_name } if env_name == "DATABASE_URL"
        ));
    }

    #[test]
    fn metadata_object_access_resolve_args_are_hosted_and_redacted() {
        let args = vec![
            "--api".to_string(),
            "https://metadata.example".to_string(),
            "--session-token-env".to_string(),
            "BINDHUB_SESSION_TOKEN".to_string(),
            "--project".to_string(),
            "project-bindhub".to_string(),
            "--lease".to_string(),
            "lease-alpha".to_string(),
            "--require-capabilities".to_string(),
            "read,head".to_string(),
        ];

        let parsed =
            parse_metadata_object_access_resolve_args(&args).expect("object access args parse");

        assert_eq!(parsed.api, "https://metadata.example");
        assert_eq!(parsed.session_token_env, "BINDHUB_SESSION_TOKEN");
        assert_eq!(parsed.project_id, "project-bindhub");
        assert_eq!(parsed.lease_id, "lease-alpha");
        assert_eq!(
            parsed.required_capabilities,
            vec![ManagedObjectCapability::Read, ManagedObjectCapability::Head]
        );
        assert!(!format!("{parsed:?}").contains("raw-hosted-session-token"));

        let wildcard_project = vec![
            "--api".to_string(),
            "https://metadata.example".to_string(),
            "--project".to_string(),
            "*".to_string(),
            "--lease".to_string(),
            "lease-alpha".to_string(),
        ];
        let error = parse_metadata_object_access_resolve_args(&wildcard_project)
            .expect_err("wildcard project is rejected");
        assert_eq!(
            error,
            "project id '*' is reserved for account-wide managed object credential leases"
        );

        let path_escape =
            api_path_segment("project/escape", "project id").expect_err("path escape is rejected");
        assert_eq!(
            path_escape.to_string(),
            "project id must be a safe hosted API path segment"
        );
    }

    #[test]
    fn hosted_auth_args_parse_and_default_session_env() {
        let login_args = vec![
            "--api".to_string(),
            "https://metadata.example".to_string(),
            "--email".to_string(),
            "dev@example.com".to_string(),
            "--invite-code".to_string(),
            "raw-code".to_string(),
        ];
        let login = parse_auth_hosted_login_args(&login_args).expect("login args parse");
        assert_eq!(login.api, "https://metadata.example");
        assert_eq!(login.email, "dev@example.com");
        assert_eq!(login.invite_code.as_deref(), Some("raw-code"));
        assert_eq!(login.invite_code_env, None);
        assert!(!format!("{login:?}").contains("raw-code"));

        let login_env_args = vec![
            "--api".to_string(),
            "https://metadata.example".to_string(),
            "--email".to_string(),
            "dev@example.com".to_string(),
            "--invite-code-env".to_string(),
            "BINDHUB_ALPHA_INVITE_CODE".to_string(),
        ];
        let login = parse_auth_hosted_login_args(&login_env_args).expect("env login args parse");
        assert_eq!(
            login.invite_code_env.as_deref(),
            Some("BINDHUB_ALPHA_INVITE_CODE")
        );
        assert_eq!(login.invite_code, None);

        let missing_invite = vec![
            "--api".to_string(),
            "https://metadata.example".to_string(),
            "--email".to_string(),
            "dev@example.com".to_string(),
        ];
        let error =
            parse_auth_hosted_login_args(&missing_invite).expect_err("invite source is required");
        assert_eq!(
            error,
            "auth hosted-login requires exactly one of --invite-code or --invite-code-env"
        );

        let status_args = vec!["--api".to_string(), "https://metadata.example".to_string()];
        let status = parse_auth_hosted_session_args(&status_args, "hosted-status")
            .expect("status args parse");
        assert_eq!(status.session_token_env, "BINDHUB_SESSION_TOKEN");

        let custom_env_args = vec![
            "--api".to_string(),
            "https://metadata.example".to_string(),
            "--session-token-env".to_string(),
            "BINDHUB_TEST_SESSION".to_string(),
        ];
        let logout = parse_auth_hosted_session_args(&custom_env_args, "hosted-logout")
            .expect("logout args parse");
        assert_eq!(logout.session_token_env, "BINDHUB_TEST_SESSION");
    }

    #[test]
    fn device_join_approve_join_and_complete_args_redact_pairing_payloads() {
        let join_args = vec![
            "--db".to_string(),
            "receiver.sqlite3".to_string(),
            "--token".to_string(),
            "raw-pairing-token".to_string(),
            "--device-name".to_string(),
            "Laptop".to_string(),
        ];
        let join = parse_device_join_args(&join_args).expect("join args parse");
        assert_eq!(join.db_path, "receiver.sqlite3");
        assert_eq!(join.token.as_deref(), Some("raw-pairing-token"));
        assert_eq!(join.token_env, None);
        assert_eq!(join.device_name, "Laptop");
        assert!(!format!("{join:?}").contains("raw-pairing-token"));

        let join_env_args = vec![
            "--db".to_string(),
            "receiver.sqlite3".to_string(),
            "--token-env".to_string(),
            "BINDHUB_PAIRING_TOKEN".to_string(),
            "--device-name".to_string(),
            "Laptop".to_string(),
        ];
        let join = parse_device_join_args(&join_env_args).expect("join env args parse");
        assert_eq!(join.token, None);
        assert_eq!(join.token_env.as_deref(), Some("BINDHUB_PAIRING_TOKEN"));

        let mixed_token_source = vec![
            "--db".to_string(),
            "receiver.sqlite3".to_string(),
            "--token".to_string(),
            "raw-pairing-token".to_string(),
            "--token-env".to_string(),
            "BINDHUB_PAIRING_TOKEN".to_string(),
            "--device-name".to_string(),
            "Laptop".to_string(),
        ];
        assert_eq!(
            parse_device_join_args(&mixed_token_source)
                .expect_err("token source must not be mixed"),
            "devices join requires exactly one of --token or --token-env"
        );

        let approve_join_args = vec![
            "--db".to_string(),
            "source.sqlite3".to_string(),
            "--token".to_string(),
            "raw-pairing-token".to_string(),
            "--join-request".to_string(),
            "raw-join-request".to_string(),
            "--device-name".to_string(),
            "Laptop".to_string(),
        ];
        let approve_join =
            parse_device_approve_join_args(&approve_join_args).expect("approve join args parse");
        assert_eq!(
            approve_join.join_request.as_deref(),
            Some("raw-join-request")
        );
        assert_eq!(approve_join.token.as_deref(), Some("raw-pairing-token"));
        assert_eq!(approve_join.token_env, None);
        assert_eq!(approve_join.join_request_env, None);
        assert!(!format!("{approve_join:?}").contains("raw-join-request"));
        assert!(!format!("{approve_join:?}").contains("raw-pairing-token"));

        let approve_join_env_args = vec![
            "--db".to_string(),
            "source.sqlite3".to_string(),
            "--token-env".to_string(),
            "BINDHUB_PAIRING_TOKEN".to_string(),
            "--join-request-env".to_string(),
            "BINDHUB_PAIRING_JOIN_REQUEST".to_string(),
            "--device-name".to_string(),
            "Laptop".to_string(),
        ];
        let approve_join = parse_device_approve_join_args(&approve_join_env_args)
            .expect("approve join env args parse");
        assert_eq!(approve_join.join_request, None);
        assert_eq!(
            approve_join.join_request_env.as_deref(),
            Some("BINDHUB_PAIRING_JOIN_REQUEST")
        );
        assert_eq!(
            approve_join.token_env.as_deref(),
            Some("BINDHUB_PAIRING_TOKEN")
        );

        let missing_join_source = vec![
            "--db".to_string(),
            "source.sqlite3".to_string(),
            "--token-env".to_string(),
            "BINDHUB_PAIRING_TOKEN".to_string(),
            "--device-name".to_string(),
            "Laptop".to_string(),
        ];
        assert_eq!(
            parse_device_approve_join_args(&missing_join_source)
                .expect_err("join request source is required"),
            "devices approve-join requires exactly one of --join-request or --join-request-env"
        );
        let mixed_join_source = vec![
            "--db".to_string(),
            "source.sqlite3".to_string(),
            "--token-env".to_string(),
            "BINDHUB_PAIRING_TOKEN".to_string(),
            "--join-request".to_string(),
            "raw-join-request".to_string(),
            "--join-request-env".to_string(),
            "BINDHUB_PAIRING_JOIN_REQUEST".to_string(),
            "--device-name".to_string(),
            "Laptop".to_string(),
        ];
        assert_eq!(
            parse_device_approve_join_args(&mixed_join_source)
                .expect_err("join request source must not be mixed"),
            "devices approve-join requires exactly one of --join-request or --join-request-env"
        );

        let mixed_approval_token_source = vec![
            "--db".to_string(),
            "source.sqlite3".to_string(),
            "--token".to_string(),
            "raw-pairing-token".to_string(),
            "--token-env".to_string(),
            "BINDHUB_PAIRING_TOKEN".to_string(),
            "--join-request-env".to_string(),
            "BINDHUB_PAIRING_JOIN_REQUEST".to_string(),
            "--device-name".to_string(),
            "Laptop".to_string(),
        ];
        assert_eq!(
            parse_device_approve_join_args(&mixed_approval_token_source)
                .expect_err("approval token source must not be mixed"),
            "devices approve-join requires exactly one of --token or --token-env"
        );

        let complete_args = vec![
            "--db".to_string(),
            "receiver.sqlite3".to_string(),
            "--completion".to_string(),
            "raw-pairing-completion".to_string(),
        ];
        let complete = parse_device_complete_args(&complete_args).expect("complete args parse");
        assert_eq!(
            complete.completion.as_deref(),
            Some("raw-pairing-completion")
        );
        assert_eq!(complete.completion_env, None);
        assert!(!format!("{complete:?}").contains("raw-pairing-completion"));

        let complete_env_args = vec![
            "--db".to_string(),
            "receiver.sqlite3".to_string(),
            "--completion-env".to_string(),
            "BINDHUB_PAIRING_COMPLETION".to_string(),
        ];
        let complete =
            parse_device_complete_args(&complete_env_args).expect("complete env args parse");
        assert_eq!(complete.completion, None);
        assert_eq!(
            complete.completion_env.as_deref(),
            Some("BINDHUB_PAIRING_COMPLETION")
        );

        let mixed_completion_source = vec![
            "--db".to_string(),
            "receiver.sqlite3".to_string(),
            "--completion".to_string(),
            "raw-pairing-completion".to_string(),
            "--completion-env".to_string(),
            "BINDHUB_PAIRING_COMPLETION".to_string(),
        ];
        assert_eq!(
            parse_device_complete_args(&mixed_completion_source)
                .expect_err("completion source must not be mixed"),
            "devices complete requires exactly one of --completion or --completion-env"
        );
    }

    #[test]
    fn auth_mock_verified_bootstrap_args_require_verified_contact_and_token() {
        let missing_contact = vec![
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--session-token".to_string(),
            "raw-token".to_string(),
        ];

        let error = parse_auth_mock_verified_bootstrap_args(&missing_contact)
            .expect_err("verified contact is required");
        assert_eq!(
            error,
            "auth mock-verified-bootstrap requires --verified-email or --verified-domain"
        );

        let args = vec![
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--verified-email".to_string(),
            "user@example.com".to_string(),
            "--session-token".to_string(),
            "raw-token".to_string(),
            "--ttl-seconds".to_string(),
            "60".to_string(),
        ];

        let parsed = parse_auth_mock_verified_bootstrap_args(&args).expect("bootstrap args parse");
        assert_eq!(parsed.provider_kind, "oidc-dev");
        assert_eq!(parsed.verified_email.as_deref(), Some("user@example.com"));
        assert_eq!(parsed.session_token, "raw-token");
        assert_eq!(parsed.ttl_seconds, 60);

        let formatted = format!("{parsed:?}");
        assert!(!formatted.contains("raw-token"));
        assert!(formatted.contains("<redacted>"));

        let proof_check = parse_auth_proof_check_args(&[
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--session-token".to_string(),
            "raw-token".to_string(),
        ])
        .expect("proof check args parse");
        let formatted = format!("{proof_check:?}");
        assert!(!formatted.contains("raw-token"));
        assert!(formatted.contains("<redacted>"));
    }

    #[test]
    fn device_recovery_and_rotation_smoke_commands_are_sanitized() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("bindhub.sqlite3");
        let raw_recovery_secret = "raw-recovery-secret-should-not-print";
        let approved_device_id;

        {
            let mut store = Store::open_file(&db_path).expect("store opens");
            store.apply_migrations().expect("migrations apply");
            let identity = store
                .ensure_local_identity(&EnsureLocalIdentityOptions {
                    device_name: Some("Current machine"),
                })
                .expect("identity initializes");
            let view = identity_view(&identity);
            let draft = create_pairing_invitation(&view, "2026-06-18T10:00:00Z", 100, 600)
                .expect("invitation creates");
            store
                .insert_pairing_invitation(&draft.invitation)
                .expect("invitation persists");
            let approval = approve_pairing_invitation(
                &view,
                &draft.invitation,
                &draft.token,
                "Laptop",
                "2026-06-18T10:01:00Z",
                101,
            )
            .expect("approval creates");
            approved_device_id = approval.device.device_id.clone();
            store
                .persist_pairing_approval(&approval)
                .expect("approval persists");
        }

        let rejected = devices_recovery_grant_create(&DeviceRecoveryGrantArgs {
            db_path: db_path.display().to_string(),
            device_id: approved_device_id.clone(),
            recovery_ref: raw_recovery_secret.to_string(),
            audit_label: "laptop recovery".to_string(),
            ttl_seconds: 600,
        })
        .expect_err("raw recovery secret is rejected");
        assert!(!rejected.to_string().contains(raw_recovery_secret));

        devices_recovery_grant_create(&DeviceRecoveryGrantArgs {
            db_path: db_path.display().to_string(),
            device_id: approved_device_id.clone(),
            recovery_ref: "recovery-ref:laptop:alpha".to_string(),
            audit_label: "laptop recovery".to_string(),
            ttl_seconds: 600,
        })
        .expect("redacted recovery grant creates");
        devices_rotate_key_envelope(&DeviceRotateEnvelopeArgs {
            db_path: db_path.display().to_string(),
            device_id: approved_device_id.clone(),
            reason: "recovery rotation".to_string(),
            session_id: None,
            ttl_seconds: 600,
        })
        .expect("key envelope rotates");

        let store = Store::open_file(&db_path).expect("store reopens");
        store.apply_migrations().expect("migrations apply");
        let envelope = store
            .key_envelope_for_device(&approved_device_id)
            .expect("envelope reads")
            .expect("envelope exists");
        assert_eq!(envelope.rotation_generation, 1);
    }

    #[test]
    fn sync_snapshot_args_default_to_local_mock_metadata() {
        let args = vec![
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--cache".to_string(),
            "cache".to_string(),
            "--remote".to_string(),
            "remote".to_string(),
            "snapshot-1".to_string(),
        ];

        let parsed = parse_sync_snapshot_args(&args, false).expect("sync args parse");

        assert_eq!(parsed.metadata.mode, SyncMetadataModeArg::LocalMock);
        assert_eq!(parsed.metadata.db_path, None);
        assert_eq!(parsed.metadata.account_id, None);
        assert_eq!(parsed.metadata.project_id, None);
    }

    #[test]
    fn sync_import_metadata_mode_requires_project_id() {
        let args = vec![
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--cache".to_string(),
            "cache".to_string(),
            "--remote".to_string(),
            "remote".to_string(),
            "--metadata-mode".to_string(),
            "mock-dev-sqlite".to_string(),
            "--metadata-db".to_string(),
            "metadata.sqlite3".to_string(),
            "snapshot-1".to_string(),
        ];

        let error =
            parse_sync_snapshot_args(&args, true).expect_err("metadata import requires project id");

        assert_eq!(
            error,
            "sync snapshot requires --metadata-project <PROJECT_ID> with --metadata-mode mock-dev-sqlite"
        );
    }

    #[test]
    fn sync_hosted_api_metadata_args_use_session_token_env_and_reject_account() {
        let args = vec![
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--cache".to_string(),
            "cache".to_string(),
            "--remote-kind".to_string(),
            "hosted".to_string(),
            "--object-access-api".to_string(),
            "https://metadata.example".to_string(),
            "--object-access-project".to_string(),
            "project-1".to_string(),
            "--object-access-lease".to_string(),
            "lease-1".to_string(),
            "--metadata-mode".to_string(),
            "hosted-api".to_string(),
            "--metadata-api".to_string(),
            "https://metadata.example".to_string(),
            "--metadata-session-token-env".to_string(),
            "BINDHUB_SESSION_TOKEN".to_string(),
            "--metadata-project".to_string(),
            "project-1".to_string(),
            "snapshot-1".to_string(),
        ];

        let parsed = parse_sync_snapshot_args(&args, true).expect("hosted metadata args parse");

        assert_eq!(parsed.metadata.mode, SyncMetadataModeArg::HostedApi);
        assert_eq!(
            parsed.metadata.session_token_env.as_deref(),
            Some("BINDHUB_SESSION_TOKEN")
        );
        assert_eq!(parsed.metadata.account_id, None);
        assert!(!format!("{:?}", parsed.metadata).contains("raw-hosted-token"));

        let mut forged_account = args.clone();
        let insert_at = forged_account.len() - 1;
        forged_account.insert(insert_at, "--metadata-account".to_string());
        forged_account.insert(insert_at + 1, "account-forged".to_string());
        let error = parse_sync_snapshot_args(&forged_account, true)
            .expect_err("hosted API account must not be supplied");
        assert_eq!(
            error,
            "sync snapshot hosted API metadata derives account identity from the authenticated session; remove --metadata-account"
        );

        let mut bad_env = args.clone();
        let env_index = bad_env
            .iter()
            .position(|arg| arg == "--metadata-session-token-env")
            .expect("env flag exists")
            + 1;
        bad_env[env_index] = "raw-hosted-token".to_string();
        let error =
            parse_sync_snapshot_args(&bad_env, true).expect_err("invalid env name is rejected");
        assert_eq!(
            error,
            "--metadata-session-token-env requires an environment variable name"
        );
        assert!(!error.contains("raw-hosted-token"));
    }

    #[test]
    fn sync_metadata_endpoint_validation_does_not_reflect_secret_material() {
        let args = vec![
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--cache".to_string(),
            "cache".to_string(),
            "--remote".to_string(),
            "remote".to_string(),
            "--metadata-mode".to_string(),
            "mock-dev-sqlite".to_string(),
            "--metadata-db".to_string(),
            "metadata.sqlite3".to_string(),
            "--metadata-endpoint".to_string(),
            "https://metadata.example/sync-key/raw".to_string(),
            "snapshot-1".to_string(),
        ];

        let error = parse_sync_snapshot_args(&args, false)
            .expect_err("secret-looking endpoint is rejected");

        assert_eq!(
            error,
            "metadata endpoint must not contain secret-looking material"
        );
        assert!(!error.contains("sync-key/raw"));
    }

    #[test]
    fn sync_metadata_import_options_derive_account_from_mock_key_source_db() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source_db = dir.path().join("source.sqlite3");
        let mut source = Store::open_file(&source_db).expect("source opens");
        source.apply_migrations().expect("migrations apply");
        let source_identity = source
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Desk"),
            })
            .expect("source identity initializes");

        let options = metadata_import_options(
            &ImportSnapshotRequest {
                db_path: dir.path().join("receiver.sqlite3"),
                cache_root: dir.path().join("cache"),
                key_source_db_path: Some(source_db),
                snapshot_id: "snapshot-1".to_string(),
            },
            &SyncMetadataArgs {
                mode: SyncMetadataModeArg::MockDevSqlite,
                db_path: Some("metadata.sqlite3".to_string()),
                account_id: None,
                project_id: Some("project-1".to_string()),
                endpoint: None,
                api: None,
                session_token_env: None,
            },
        )
        .expect("metadata options derive account");

        assert_eq!(options.account_id, source_identity.account_id);
        assert_eq!(options.project_id, "project-1");
    }

    #[test]
    fn sync_publish_rejects_unused_metadata_account_scope_flags() {
        let args = vec![
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--cache".to_string(),
            "cache".to_string(),
            "--remote".to_string(),
            "remote".to_string(),
            "--metadata-mode".to_string(),
            "mock-dev-sqlite".to_string(),
            "--metadata-db".to_string(),
            "metadata.sqlite3".to_string(),
            "--metadata-account".to_string(),
            "account-source".to_string(),
            "snapshot-1".to_string(),
        ];

        let error =
            parse_sync_snapshot_args(&args, false).expect_err("publish rejects account scope");

        assert_eq!(
            error,
            "sync snapshot accepts --metadata-account/--metadata-project only for import-snapshot or materialize"
        );
    }

    #[test]
    fn secret_policy_args_reject_secret_like_envelope_refs() {
        let raw = ["sk-", "abcdefghijklmnop", "qrstuvwxyzABCDEFGH123456"].concat();
        let unsafe_ref = format!("secret-envelope-ref:legacy/{raw}");
        let args = vec![
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--project".to_string(),
            "project-1".to_string(),
            "--path".to_string(),
            ".env".to_string(),
            "--action".to_string(),
            "envelope".to_string(),
            "--envelope-ref".to_string(),
            unsafe_ref.clone(),
        ];

        let error =
            parse_secret_policy_add_args(&args).expect_err("raw secret envelope ref is rejected");

        assert_eq!(
            error,
            "secret envelope reference must not contain secret-looking material"
        );
        assert!(!error.contains(&raw));
        assert!(!error.contains(&unsafe_ref));

        let missing_scheme = vec![
            "--db".to_string(),
            "bindhub.sqlite3".to_string(),
            "--project".to_string(),
            "project-1".to_string(),
            "--path".to_string(),
            ".env".to_string(),
            "--action".to_string(),
            "envelope".to_string(),
            "--envelope-ref".to_string(),
            "vault:env".to_string(),
        ];
        let error =
            parse_secret_policy_add_args(&missing_scheme).expect_err("opaque scheme is required");
        assert_eq!(
            error,
            "secret envelope reference must use the secret-envelope-ref: opaque scheme"
        );
    }

    #[test]
    fn secret_policy_add_persists_sanitized_alpha_rule() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("bindhub.sqlite3");
        {
            let store = Store::open_file(&db_path).expect("store opens");
            store.apply_migrations().expect("migrations apply");
            store
                .insert_project(&NewProject {
                    id: "project-1",
                    root_path: "/workspace/bindhub",
                    kind: "Rust",
                    display_name: "bindhub",
                    discovered_at: "2026-06-18T10:00:00Z",
                })
                .expect("project inserts");
        }

        secret_policy_add(&SecretPolicyAddArgs {
            db_path: db_path.display().to_string(),
            project_id: "project-1".to_string(),
            path: ".env.example".to_string(),
            action: SecretPolicyAction::Template,
            envelope_ref: None,
            note: Some("sync variable names only".to_string()),
        })
        .expect("template policy adds");

        let store = Store::open_file(&db_path).expect("store reopens");
        store.apply_migrations().expect("migrations apply");
        let rules = store
            .list_secret_policy_rules(Some("project-1"))
            .expect("rules list");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].action, SecretPolicyAction::Template);
        assert_eq!(rules[0].envelope_ref, None);
    }

    #[test]
    fn conflict_resolve_requires_manual_confirmation_and_open_status() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("bindhub.sqlite3");
        let conflict_id = "conflict-manual";
        {
            let mut store = Store::open_file(&db_path).expect("store opens");
            store.apply_migrations().expect("migrations apply");
            let include = PolicyDecision::Include;
            let blob = BlobId::from_blake3_hex(
                "a3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03",
            )
            .expect("valid blob id");
            let readme = Path::new("README.md");
            let entries = [NewSnapshotManifestEntry {
                relative_path: readme,
                kind: ManifestEntryKind::File,
                size_bytes: 4,
                blob_id: Some(&blob),
                object_ref: Some("blobs/b3/readme"),
                policy_decision: &include,
            }];
            store
                .persist_draft_snapshot(&NewSnapshotDraft {
                    project: NewProject {
                        id: "project-1",
                        root_path: "/workspace/bindhub",
                        kind: "Rust",
                        display_name: "bindhub",
                        discovered_at: "2026-06-18T10:00:00Z",
                    },
                    snapshot: NewSnapshot {
                        id: "snapshot-local",
                        project_id: "project-1",
                        parent_snapshot_id: None,
                        created_at: "2026-06-18T10:01:00Z",
                        reason: "manual",
                        manifest_entry_count: entries.len() as u64,
                        total_size_bytes: 4,
                    },
                    entries: entries.to_vec(),
                })
                .expect("local snapshot persists");
            store
                .persist_draft_snapshot(&NewSnapshotDraft {
                    project: NewProject {
                        id: "project-1",
                        root_path: "/workspace/bindhub",
                        kind: "Rust",
                        display_name: "bindhub",
                        discovered_at: "2026-06-18T10:00:00Z",
                    },
                    snapshot: NewSnapshot {
                        id: "snapshot-incoming",
                        project_id: "project-1",
                        parent_snapshot_id: None,
                        created_at: "2026-06-18T10:02:00Z",
                        reason: "manual",
                        manifest_entry_count: entries.len() as u64,
                        total_size_bytes: 4,
                    },
                    entries: entries.to_vec(),
                })
                .expect("incoming snapshot persists");
            let summary = bindhub_conflict::ConflictSummary::from_rows(&[]);
            store
                .persist_conflict(
                    &NewConflict {
                        id: conflict_id,
                        project_id: "project-1",
                        base_snapshot_id: None,
                        local_snapshot_id: "snapshot-local",
                        incoming_snapshot_id: "snapshot-incoming",
                        summary: &summary,
                        created_at: "2026-06-18T10:03:00Z",
                    },
                    &[],
                )
                .expect("conflict persists");
        }

        let missing_confirmation = conflicts_resolve(&ConflictResolveArgs {
            db_path: db_path.display().to_string(),
            conflict_id: conflict_id.to_string(),
            manual_resolution: ManualConflictResolution::KeepBoth,
            confirm_no_auto_apply: false,
        })
        .expect_err("confirmation is required");
        assert!(missing_confirmation
            .to_string()
            .contains("--confirm-no-auto-apply"));

        conflicts_resolve(&ConflictResolveArgs {
            db_path: db_path.display().to_string(),
            conflict_id: conflict_id.to_string(),
            manual_resolution: ManualConflictResolution::KeepBoth,
            confirm_no_auto_apply: true,
        })
        .expect("manual conflict resolves");

        let already_resolved = conflicts_resolve(&ConflictResolveArgs {
            db_path: db_path.display().to_string(),
            conflict_id: conflict_id.to_string(),
            manual_resolution: ManualConflictResolution::KeepBoth,
            confirm_no_auto_apply: true,
        })
        .expect_err("resolved conflicts cannot be resolved again");
        assert!(already_resolved.to_string().contains("only open conflicts"));
    }
}
