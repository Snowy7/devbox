use devbox_auth::{
    approve_pairing_invitation, create_pairing_invitation, mock_login, now_unix_seconds,
    DeviceProjectCursor, DeviceTrustRecord, LocalIdentityView, PairingInvitationToken,
};
use devbox_conflict::{
    compare_snapshots, path_to_conflict_string, ComparableEntry, ComparableSnapshot,
    PathComparisonRow,
};
use devbox_core::scanner::ProjectScanner;
use devbox_core::{BlobId, ManifestEntryKind, PolicyDecision};
use devbox_materialize::{
    import_snapshot, import_snapshot_with_metadata, materialize_snapshot,
    materialize_snapshot_with_metadata, publish_snapshot, publish_snapshot_with_metadata,
    sync_preflight, HostedMetadataImportOptions, ImportSnapshotRequest, MaterializationRequest,
    MaterializeError, PublishSnapshotRequest, SyncPreflightOutcome, SyncPreflightRequest,
};
use devbox_metadata::{MetadataAuthMode, MetadataServiceConfig, SqliteMetadataStore};
use devbox_snapshot::{
    is_secret_block_reason, preflight_cache_root, preflight_db_path, scan_local_change_feed,
    LocalChangeFeedScanOptions, RestoreMaterializer, RestorePlan, RestoreSkippedEntry,
    RestoreTargetStatus, RestoreWrite, SnapshotManifestBuilder, SnapshotManifestEntry,
};
use devbox_store::{
    local_project_id, path_to_store_string, BlobCache, ConflictRowRecord, ConflictStatus,
    EnsureLocalIdentityOptions, LocalChangeKind, LocalIdentityRecord, ManifestEntryRecord,
    NewConflict, NewConflictRow, NewProject, NewSnapshot, NewSnapshotDraft,
    NewSnapshotManifestEntry, PendingLocalChangeRecord, PersistedSnapshot, Store,
};
use devbox_sync::{
    download_blob_to_cache, encrypted_blob_object_key, upload_blob_from_cache,
    LocalFilesystemBlobProvider, ObjectKey, RemoteBlobProvider, S3CompatibleBlobProvider,
    S3CompatibleConfig, S3CredentialsSource, S3RedactedConfig, SyncKey,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") | Some("version") => {
            println!("devbox {VERSION}");
            ExitCode::SUCCESS
        }
        Some("scan") => run_scan(&args[1..]),
        Some("init") => run_init(&args[1..]),
        Some("auth") => run_auth(&args[1..]),
        Some("devices") => run_devices(&args[1..]),
        Some("metadata") => run_metadata(&args[1..]),
        Some("sync") => run_sync(&args[1..]),
        Some("conflicts") => run_conflicts(&args[1..]),
        Some("status") => run_status(&args[1..]),
        Some("snapshot") => run_snapshot(&args[1..]),
        Some("changes") => run_changes(&args[1..]),
        Some("restore" | "explain") => {
            println!("devbox: command placeholder; daemon integration is not implemented yet");
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("devbox: unknown command '{command}'");
            eprintln!("Run 'devbox --help' for usage.");
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
                eprintln!("devbox: {error}");
                ExitCode::from(1)
            }
        },
        Some("list") => match parse_changes_list_args(&args[1..])
            .and_then(|args| changes_list(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                ExitCode::from(1)
            }
        },
        Some("clear") => match parse_changes_list_args(&args[1..])
            .and_then(|args| changes_clear(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("devbox: changes requires scan, list, or clear");
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
                eprintln!("devbox: {error}");
                print_conflicts_usage();
                ExitCode::from(1)
            }
        },
        Some("list") => match parse_conflict_list_args(&args[1..])
            .and_then(|args| conflicts_list(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                print_conflicts_usage();
                ExitCode::from(1)
            }
        },
        Some("show") => match parse_conflict_show_args(&args[1..])
            .and_then(|args| conflicts_show(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                print_conflicts_usage();
                ExitCode::from(1)
            }
        },
        Some("resolve") => match parse_conflict_show_args(&args[1..]).and_then(|args| {
            conflicts_update_status(&args, ConflictStatus::Resolved)
                .map_err(|error| error.to_string())
        }) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
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
                eprintln!("devbox: {error}");
                print_conflicts_usage();
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("devbox: conflicts requires compare, list, show, resolve, or dismiss");
            print_conflicts_usage();
            ExitCode::from(2)
        }
    }
}

fn run_snapshot(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("restore") => match snapshot_restore(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                ExitCode::from(1)
            }
        },
        Some("list") => match snapshot_list(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                ExitCode::from(1)
            }
        },
        Some("show") => match snapshot_show(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                ExitCode::from(1)
            }
        },
        _ => match parse_snapshot_create_args(args) {
            Ok(create_args) if create_args.dry_run => {
                match snapshot_dry_run(&create_args.cache_root, &create_args.path) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        eprintln!("devbox: {error}");
                        ExitCode::from(1)
                    }
                }
            }
            Ok(create_args) => match snapshot_create(&create_args) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("devbox: {error}");
                    ExitCode::from(1)
                }
            },
            Err(message) => {
                eprintln!("devbox: {message}");
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
struct InitArgs {
    db_path: String,
    device_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DbOnlyArgs {
    db_path: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceRevokeArgs {
    db_path: String,
    device_id: String,
    reason: Option<String>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncMetadataModeArg {
    LocalMock,
    MockDevSqlite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncMetadataArgs {
    mode: SyncMetadataModeArg,
    db_path: Option<String>,
    account_id: Option<String>,
    project_id: Option<String>,
    endpoint: Option<String>,
}

impl Default for SyncMetadataArgs {
    fn default() -> Self {
        Self {
            mode: SyncMetadataModeArg::LocalMock,
            db_path: None,
            account_id: None,
            project_id: None,
            endpoint: None,
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

fn run_init(args: &[String]) -> ExitCode {
    match parse_init_args(args)
        .and_then(|args| init_identity(&args).map_err(|error| error.to_string()))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("devbox: {error}");
            eprintln!("Usage: devbox init --db <DB_PATH> [--device-name <NAME>]");
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
                    eprintln!("devbox: {error}");
                    eprintln!("Usage: devbox auth mock-login --db <DB_PATH>");
                    ExitCode::from(1)
                }
            }
        }
        Some("status") => match parse_db_only_args(&args[1..], "auth status")
            .and_then(|args| auth_status(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                eprintln!("Usage: devbox auth status --db <DB_PATH>");
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("devbox: auth requires mock-login or status");
            eprintln!("Usage:");
            eprintln!("  devbox auth mock-login --db <DB_PATH>");
            eprintln!("  devbox auth status --db <DB_PATH>");
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
                eprintln!("devbox: {error}");
                eprintln!("Usage: devbox devices list --db <DB_PATH>");
                ExitCode::from(1)
            }
        },
        Some("invite") => match parse_device_invite_args(&args[1..])
            .and_then(|args| devices_invite(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                eprintln!("Usage: devbox devices invite --db <DB_PATH> [--ttl-seconds <SECONDS>]");
                ExitCode::from(1)
            }
        },
        Some("approve") => match parse_device_approve_args(&args[1..])
            .and_then(|args| devices_approve(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                eprintln!(
                    "Usage: devbox devices approve --db <DB_PATH> --token <TOKEN> --device-name <NAME>"
                );
                ExitCode::from(1)
            }
        },
        Some("revoke") => match parse_device_revoke_args(&args[1..])
            .and_then(|args| devices_revoke(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                eprintln!(
                    "Usage: devbox devices revoke --db <DB_PATH> <DEVICE_ID> [--reason <TEXT>]"
                );
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("devbox: devices requires list, invite, approve, or revoke");
            eprintln!("Usage:");
            eprintln!("  devbox devices list --db <DB_PATH>");
            eprintln!("  devbox devices invite --db <DB_PATH> [--ttl-seconds <SECONDS>]");
            eprintln!(
                "  devbox devices approve --db <DB_PATH> --token <TOKEN> --device-name <NAME>"
            );
            eprintln!("  devbox devices revoke --db <DB_PATH> <DEVICE_ID> [--reason <TEXT>]");
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
                eprintln!("devbox: {error}");
                print_metadata_usage();
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("devbox: metadata requires check");
            print_metadata_usage();
            ExitCode::from(2)
        }
    }
}

fn run_sync(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("publish-snapshot") => match parse_sync_snapshot_args(&args[1..], false)
            .and_then(|args| sync_publish_snapshot(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("import-snapshot") => match parse_sync_snapshot_args(&args[1..], true)
            .and_then(|args| sync_import_snapshot(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("materialize") => match parse_sync_materialize_args(&args[1..])
            .and_then(|args| sync_materialize(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("preflight") => match parse_sync_preflight_args(&args[1..])
            .and_then(|args| sync_preflight_command(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("upload") => match parse_sync_blob_args(&args[1..])
            .and_then(|args| sync_upload(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                print_sync_usage();
                ExitCode::from(1)
            }
        },
        Some("download") => match parse_sync_blob_args(&args[1..])
            .and_then(|args| sync_download(&args).map_err(|error| error.to_string()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
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
                    eprintln!("devbox: {error}");
                    print_sync_usage();
                    ExitCode::from(1)
                }
            },
            _ => {
                eprintln!("devbox: sync remote requires check");
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
                    eprintln!("devbox: {error}");
                    print_sync_usage();
                    ExitCode::from(1)
                }
            },
            Some("set") => match parse_sync_cursor_args(&args[2..], true)
                .and_then(|args| sync_cursor_set(&args).map_err(|error| error.to_string()))
            {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("devbox: {error}");
                    print_sync_usage();
                    ExitCode::from(1)
                }
            },
            _ => {
                eprintln!("devbox: sync cursor requires get or set");
                print_sync_usage();
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!(
                "devbox: sync requires publish-snapshot, import-snapshot, materialize, preflight, upload, download, remote, or cursor"
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
                let value = args
                    .get(index)
                    .ok_or_else(|| "--auth-mode requires mock-dev-headers".to_string())?;
                auth_mode = match value.as_str() {
                    "mock-dev-headers" => MetadataAuthMode::MockDevHeaders,
                    _ => return Err("--auth-mode requires mock-dev-headers".to_string()),
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
                "--metadata-mode requires local-mock or mock-dev-sqlite".to_string()
            })?;
            metadata.mode = match value.as_str() {
                "local-mock" => SyncMetadataModeArg::LocalMock,
                "mock-dev-sqlite" => SyncMetadataModeArg::MockDevSqlite,
                _ => {
                    return Err("--metadata-mode requires local-mock or mock-dev-sqlite".to_string())
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
        {
            return Err(format!(
                "{command} metadata flags require --metadata-mode mock-dev-sqlite"
            ));
        }
        return Ok(metadata);
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
                .ok_or_else(|| "--remote-kind requires local or s3".to_string())?;
            remote.kind = match value.as_str() {
                "local" => SyncRemoteKindArg::Local,
                "s3" => SyncRemoteKindArg::S3,
                _ => return Err("--remote-kind requires local or s3".to_string()),
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
            Ok(remote)
        }
        SyncRemoteKindArg::S3 => {
            if remote.local_root.is_some() {
                return Err(format!(
                    "{command_name} uses --s3-endpoint/--s3-bucket for --remote-kind s3, not --remote"
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
        .local_identity()?
        .ok_or("local identity is not initialized; run devbox init --db <DB_PATH>")?;
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
        .local_identity()?
        .ok_or("local identity is not initialized; run devbox init --db <DB_PATH>")?;
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
    println!("Production authentication: not configured");

    Ok(())
}

fn devices_invite(args: &DeviceInviteArgs) -> Result<(), Box<dyn std::error::Error>> {
    let store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .local_identity()?
        .ok_or("local identity is not initialized; run devbox init --db <DB_PATH>")?;
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
    println!("Provider: local/mock metadata");
    println!("Raw account/device keys: not printed");

    Ok(())
}

fn devices_approve(args: &DeviceApproveArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = open_existing_metadata_store(&args.db_path)?;
    store.apply_migrations()?;
    let identity = store
        .local_identity()?
        .ok_or("local identity is not initialized; run devbox init --db <DB_PATH>")?;
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

#[derive(Debug, Clone)]
enum RemoteProviderDescription {
    Local { root: PathBuf },
    S3 { redacted: S3RedactedConfig },
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
                let probe = ObjectKey::new("devbox/remote-check/probe")?;
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
                let probe = ObjectKey::new("devbox/remote-check/probe")?;
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
        .local_identity()?
        .ok_or("local identity is not initialized; run devbox init --db <DB_PATH>")?;
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
        .local_identity()?
        .ok_or("local identity is not initialized; run devbox init --db <DB_PATH>")?;
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
            .local_identity()?
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
) -> Result<devbox_materialize::ImportedSnapshotBundle, MaterializeError> {
    if metadata.mode == SyncMetadataModeArg::MockDevSqlite {
        let mut metadata_store = open_sync_metadata_store(metadata)
            .map_err(|error| MaterializeError::InvalidBundle(error.to_string()))?;
        let options = metadata_import_options(request, metadata)
            .map_err(|error| MaterializeError::InvalidBundle(error.to_string()))?;
        import_snapshot_with_metadata(request, provider, &mut metadata_store, &options)
    } else {
        import_snapshot(request, provider)
    }
}

fn materialize_snapshot_command(
    request: &MaterializationRequest,
    provider: &(impl RemoteBlobProvider + ?Sized),
    metadata: &SyncMetadataArgs,
) -> Result<devbox_materialize::MaterializationOutcome, MaterializeError> {
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
        .ok_or("local identity is not initialized; run devbox init --db <DB_PATH>")?;
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
        .ok_or("local identity is not initialized; run devbox init --db <DB_PATH>")?;
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
    println!("Auth mode: mock-dev-headers");
    println!("Required headers: x-devbox-mock-account-id, x-devbox-mock-device-id");
    println!("Network check: {}", check.network_check);
    println!("Production ready: {}", check.production_ready);
    println!(
        "Boundary: local tests/dev only; production OAuth and managed credentials are deferred"
    );

    Ok(())
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
            "{message}\nUsage: devbox snapshot restore --db <DB_PATH> --cache <CACHE_ROOT> --to <TARGET_DIR> <SNAPSHOT_ID> [--dry-run|--apply]"
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
    summary: &devbox_snapshot::LocalChangeSummary,
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
        return Err("Usage: devbox snapshot list --db <DB_PATH>".into());
    };
    if flag != "--db" {
        return Err("Usage: devbox snapshot list --db <DB_PATH>".into());
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
        return Err("Usage: devbox snapshot show --db <DB_PATH> <SNAPSHOT_ID>".into());
    };
    if flag != "--db" {
        return Err("Usage: devbox snapshot show --db <DB_PATH> <SNAPSHOT_ID>".into());
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
    conflict: &devbox_store::ConflictRecord,
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
                devbox_core::ManifestEntryKind::File => included_files += 1,
                devbox_core::ManifestEntryKind::Directory => included_directories += 1,
                devbox_core::ManifestEntryKind::Symlink => included_symlinks += 1,
                devbox_core::ManifestEntryKind::Unsupported => deferred_entries += 1,
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
    eprintln!("  devbox snapshot --cache <CACHE_ROOT> --dry-run <PATH>");
    eprintln!("  devbox snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PATH>");
    eprintln!("  devbox snapshot list --db <DB_PATH>");
    eprintln!("  devbox snapshot show --db <DB_PATH> <SNAPSHOT_ID>");
    eprintln!(
        "  devbox snapshot restore --db <DB_PATH> --cache <CACHE_ROOT> --to <TARGET_DIR> <SNAPSHOT_ID> [--dry-run|--apply]"
    );
}

fn print_changes_usage() {
    eprintln!("Usage:");
    eprintln!("  devbox changes scan --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>");
    eprintln!("  devbox changes list --db <DB_PATH> [--project <PROJECT_ID>]");
    eprintln!("  devbox changes clear --db <DB_PATH> [--project <PROJECT_ID>]");
}

fn print_conflicts_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  devbox conflicts compare --db <DB_PATH> --local <LOCAL_SNAPSHOT_ID> --incoming <INCOMING_SNAPSHOT_ID> [--base <BASE_SNAPSHOT_ID>]"
    );
    eprintln!("  devbox conflicts list --db <DB_PATH> [--project <PROJECT_ID>]");
    eprintln!("  devbox conflicts show --db <DB_PATH> <CONFLICT_ID>");
    eprintln!("  devbox conflicts resolve --db <DB_PATH> <CONFLICT_ID>");
    eprintln!("  devbox conflicts dismiss --db <DB_PATH> <CONFLICT_ID>");
}

fn print_sync_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  devbox sync publish-snapshot --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <SNAPSHOT_ID>"
    );
    eprintln!(
        "  devbox sync import-snapshot --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> [--mock-key-source-db <PUBLISHER_DB>] <SNAPSHOT_ID>"
    );
    eprintln!(
        "  devbox sync materialize --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> --to <TARGET_DIR> [--mock-key-source-db <PUBLISHER_DB>] <SNAPSHOT_ID> [--dry-run|--apply]"
    );
    eprintln!(
        "  devbox sync preflight --db <DB_PATH> --project <PROJECT_ID> --local <LOCAL_SNAPSHOT_ID> --incoming <INCOMING_SNAPSHOT_ID> [--base <BASE_SNAPSHOT_ID>]"
    );
    eprintln!(
        "  devbox sync upload --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <BLOB_ID> [--object-key <KEY>]"
    );
    eprintln!(
        "  devbox sync download --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <BLOB_ID> [--object-key <KEY>]"
    );
    eprintln!("  devbox sync remote check --remote <REMOTE_DIR> [--validate-only]");
    eprintln!(
        "  devbox sync remote check --remote-kind s3 --s3-endpoint <URL> --s3-bucket <BUCKET> [--s3-region <REGION>] [--s3-prefix <PREFIX>] [--s3-access-key-env <ENV> --s3-secret-key-env <ENV>] [--s3-session-token-env <ENV>] [--validate-only]"
    );
    eprintln!(
        "  Add --remote-kind s3 plus the --s3-* flags above to publish-snapshot, import-snapshot, materialize, upload, or download."
    );
    eprintln!(
        "  Add --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> to publish-snapshot for hosted mock-dev metadata registration."
    );
    eprintln!(
        "  Add --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> to import-snapshot or materialize for hosted mock-dev manifest discovery and cursor CAS."
    );
    eprintln!(
        "  Import/materialize metadata account scope is --metadata-account <ACCOUNT_ID>, or it is derived from --mock-key-source-db <PUBLISHER_DB> in the local/mock trust bootstrap path."
    );
    eprintln!(
        "  Optional --metadata-endpoint <URL> validates and prints a sanitized label only; sync metadata mode remains in-process and no network check runs."
    );
    eprintln!(
        "  devbox sync cursor get --db <DB_PATH> --project <PROJECT_ID> [--device <DEVICE_ID>]"
    );
    eprintln!(
        "  devbox sync cursor set --db <DB_PATH> --project <PROJECT_ID> --value <CURSOR> [--device <DEVICE_ID>]"
    );
}

fn print_metadata_usage() {
    eprintln!("Usage:");
    eprintln!("  devbox metadata check --endpoint <URL> [--auth-mode mock-dev-headers]");
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
        [] => {
            println!("devbox: status placeholder; pass --db <PATH> to inspect local metadata");
            ExitCode::SUCCESS
        }
        [flag, path] if flag == "--db" => match status_for_db(path) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("devbox: {error}");
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("devbox: status accepts either no arguments or --db <PATH>");
            eprintln!("Usage: devbox status --db <PATH>");
            ExitCode::from(2)
        }
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
        eprintln!("devbox: scan requires exactly one path");
        eprintln!("Usage: devbox scan <PATH>");
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
            eprintln!("devbox: {error}");
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
    println!("devbox {VERSION}");
    println!();
    println!("Usage: devbox <COMMAND>");
    println!();
    println!("Commands:");
    println!("  scan       Classify a local directory and explain default policy exclusions");
    println!("  init       Initialize local account and current-device identity");
    println!("  auth       Manage local/mock auth session status");
    println!("  devices    List, invite, approve, and revoke local/mock trusted devices");
    println!("  metadata   Validate hosted metadata service config without a network request");
    println!("  sync       Upload/download encrypted blobs and manage local cursors");
    println!("  snapshot   Build, persist, list, show, and restore local snapshot manifests");
    println!("  changes    Scan, list, and clear the pending local change feed");
    println!("  conflicts  Compare, persist, list, and update divergent snapshot conflicts");
    println!("  status     Placeholder status, or inspect local metadata with --db <PATH>");
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
    use devbox_snapshot::SnapshotPreflightError;
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
        let db_path = root.join("devbox.sqlite3");

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
        let db_path = dir.path().join("devbox.sqlite3");

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
    fn sync_snapshot_args_default_to_local_mock_metadata() {
        let args = vec![
            "--db".to_string(),
            "devbox.sqlite3".to_string(),
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
            "devbox.sqlite3".to_string(),
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
    fn sync_metadata_endpoint_validation_does_not_reflect_secret_material() {
        let args = vec![
            "--db".to_string(),
            "devbox.sqlite3".to_string(),
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
            "devbox.sqlite3".to_string(),
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
}
