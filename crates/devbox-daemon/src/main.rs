use devbox_materialize::{
    import_snapshot, import_snapshot_with_metadata, materialize_snapshot,
    materialize_snapshot_with_metadata, publish_snapshot, publish_snapshot_with_metadata,
    HostedMetadataImportOptions, ImportSnapshotRequest, MaterializationRequest, MaterializeError,
    PublishSnapshotRequest,
};
use devbox_metadata::{
    ManagedObjectAccessGrant, ManagedObjectAccessRequest, ManagedObjectCapability,
    MetadataAuthMode, MetadataServiceConfig, MetadataStore, SqliteMetadataStore,
};
use devbox_snapshot::{
    preflight_cache_root, preflight_db_path, scan_local_change_feed, LocalChangeFeedScan,
    LocalChangeFeedScanOptions, SnapshotManifestBuilder,
};
use devbox_store::{
    local_project_id, BlobCache, NewProject, NewSnapshot, NewSnapshotDraft,
    NewSnapshotManifestEntry, Store, StoreError,
};
use devbox_sync::{
    HostedObjectTransferConfig, HostedObjectTransferProvider, HostedRedactedConfig,
    LocalFilesystemBlobProvider, RemoteBlobProvider, S3CompatibleBlobProvider, S3CompatibleConfig,
    S3CredentialsSource, S3RedactedConfig,
};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_DEBOUNCE_MS: u64 = 500;

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") | Some("version") => {
            println!("devbox-daemon {VERSION}");
            ExitCode::SUCCESS
        }
        Some("watch") => match parse_watch_args(&args[1..]).and_then(run_watch) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("watch error={}", script_value(&error));
                ExitCode::from(1)
            }
        },
        Some("sync") => match parse_sync_args(&args[1..]).and_then(run_sync) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("sync error={}", script_value(&error));
                ExitCode::from(1)
            }
        },
        Some("--help") | Some("-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("devbox-daemon: unknown command '{command}'");
            eprintln!("Run 'devbox-daemon --help' for usage.");
            ExitCode::from(2)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchArgs {
    db_path: PathBuf,
    cache_root: PathBuf,
    project_root: PathBuf,
    once: bool,
    debounce_ms: u64,
    exit_after_idle_ms: Option<u64>,
    max_scans: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncArgs {
    db_path: PathBuf,
    cache_root: PathBuf,
    project_root: PathBuf,
    remote: SyncRemoteArgs,
    metadata: SyncMetadataArgs,
    object_access: ObjectAccessArgs,
    push: bool,
    pull: bool,
    pull_snapshot_id: Option<String>,
    target: Option<PathBuf>,
    apply: bool,
    once: bool,
    debounce_ms: u64,
    exit_after_idle_ms: Option<u64>,
    max_cycles: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncRemoteArgs {
    kind: SyncRemoteKind,
    local_root: Option<PathBuf>,
    s3_endpoint: Option<String>,
    s3_bucket: Option<String>,
    s3_region: String,
    s3_prefix: Option<String>,
    s3_access_key_env: Option<String>,
    s3_secret_key_env: Option<String>,
    s3_session_token_env: Option<String>,
}

impl Default for SyncRemoteArgs {
    fn default() -> Self {
        Self {
            kind: SyncRemoteKind::Local,
            local_root: None,
            s3_endpoint: None,
            s3_bucket: None,
            s3_region: "auto".to_string(),
            s3_prefix: None,
            s3_access_key_env: None,
            s3_secret_key_env: None,
            s3_session_token_env: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncRemoteKind {
    Local,
    S3,
    Hosted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncMetadataArgs {
    mode: SyncMetadataMode,
    db_path: Option<PathBuf>,
    account_id: Option<String>,
    project_id: Option<String>,
    endpoint: Option<String>,
}

impl Default for SyncMetadataArgs {
    fn default() -> Self {
        Self {
            mode: SyncMetadataMode::LocalMock,
            db_path: None,
            account_id: None,
            project_id: None,
            endpoint: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncMetadataMode {
    LocalMock,
    MockDevSqlite,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ObjectAccessArgs {
    api: Option<String>,
    session_token_env: Option<String>,
    lease_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteProviderDescription {
    Local { root: PathBuf },
    S3 { redacted: S3RedactedConfig },
    Hosted { redacted: HostedRedactedConfig },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LivePersistedSnapshot {
    project_id: String,
    snapshot_id: String,
    reused: bool,
}

fn parse_watch_args(args: &[String]) -> Result<WatchArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut project_root = None;
    let mut once = false;
    let mut debounce_ms = DEFAULT_DEBOUNCE_MS;
    let mut exit_after_idle_ms = None;
    let mut max_scans = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--db" => {
                index += 1;
                db_path = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "--db requires a path".to_string())?,
                ));
            }
            "--cache" => {
                index += 1;
                cache_root = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "--cache requires a path".to_string())?,
                ));
            }
            "--once" => once = true,
            "--debounce-ms" => {
                index += 1;
                debounce_ms = parse_u64_flag(
                    "--debounce-ms",
                    args.get(index)
                        .ok_or_else(|| "--debounce-ms requires a value".to_string())?,
                )?;
            }
            "--exit-after-idle-ms" => {
                index += 1;
                exit_after_idle_ms = Some(parse_u64_flag(
                    "--exit-after-idle-ms",
                    args.get(index)
                        .ok_or_else(|| "--exit-after-idle-ms requires a value".to_string())?,
                )?);
            }
            "--max-scans" => {
                index += 1;
                max_scans = Some(parse_usize_flag(
                    "--max-scans",
                    args.get(index)
                        .ok_or_else(|| "--max-scans requires a value".to_string())?,
                )?);
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown watch option '{value}'"));
            }
            value => {
                if project_root.replace(PathBuf::from(value)).is_some() {
                    return Err("watch accepts exactly one project root".to_string());
                }
            }
        }

        index += 1;
    }

    Ok(WatchArgs {
        db_path: db_path.ok_or_else(|| "watch requires --db <DB_PATH>".to_string())?,
        cache_root: cache_root.ok_or_else(|| "watch requires --cache <CACHE_ROOT>".to_string())?,
        project_root: project_root.ok_or_else(|| "watch requires a project root".to_string())?,
        once,
        debounce_ms,
        exit_after_idle_ms,
        max_scans,
    })
}

fn parse_sync_args(args: &[String]) -> Result<SyncArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut project_root = None;
    let mut remote = SyncRemoteArgs::default();
    let mut metadata = SyncMetadataArgs::default();
    let mut object_access = ObjectAccessArgs::default();
    let mut push = false;
    let mut pull = false;
    let mut pull_snapshot_id = None;
    let mut target = None;
    let mut apply = false;
    let mut once = false;
    let mut debounce_ms = DEFAULT_DEBOUNCE_MS;
    let mut exit_after_idle_ms = None;
    let mut max_cycles = None;
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
        if parse_object_access_arg(current, args, &mut index, &mut object_access)? {
            index += 1;
            continue;
        }

        match current {
            "--db" => {
                index += 1;
                db_path = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "--db requires a path".to_string())?,
                ));
            }
            "--cache" => {
                index += 1;
                cache_root = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| "--cache requires a path".to_string())?,
                ));
            }
            "--push" => push = true,
            "--pull" => pull = true,
            "--two-way" => {
                push = true;
                pull = true;
            }
            "--pull-snapshot" => {
                index += 1;
                pull_snapshot_id = Some(
                    args.get(index)
                        .ok_or_else(|| "--pull-snapshot requires a snapshot id".to_string())?
                        .clone(),
                );
            }
            "--to" => {
                index += 1;
                target =
                    Some(PathBuf::from(args.get(index).ok_or_else(|| {
                        "--to requires a target directory".to_string()
                    })?));
            }
            "--apply" => apply = true,
            "--dry-run" => apply = false,
            "--once" => once = true,
            "--debounce-ms" => {
                index += 1;
                debounce_ms = parse_u64_flag(
                    "--debounce-ms",
                    args.get(index)
                        .ok_or_else(|| "--debounce-ms requires a value".to_string())?,
                )?;
            }
            "--exit-after-idle-ms" => {
                index += 1;
                exit_after_idle_ms = Some(parse_u64_flag(
                    "--exit-after-idle-ms",
                    args.get(index)
                        .ok_or_else(|| "--exit-after-idle-ms requires a value".to_string())?,
                )?);
            }
            "--max-cycles" => {
                index += 1;
                max_cycles = Some(parse_usize_flag(
                    "--max-cycles",
                    args.get(index)
                        .ok_or_else(|| "--max-cycles requires a value".to_string())?,
                )?);
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown sync option '{value}'"));
            }
            value => {
                if project_root.replace(PathBuf::from(value)).is_some() {
                    return Err("sync accepts exactly one project root".to_string());
                }
            }
        }

        index += 1;
    }

    if !push && !pull {
        push = true;
    }
    if apply && target.is_none() {
        return Err("sync --apply requires --to <TARGET_DIR>".to_string());
    }
    if target.is_some() && !pull {
        return Err("sync --to is only valid with --pull".to_string());
    }
    if pull_snapshot_id.is_some() && !pull {
        return Err("sync --pull-snapshot is only valid with --pull".to_string());
    }

    let args = SyncArgs {
        db_path: db_path.ok_or_else(|| "sync requires --db <DB_PATH>".to_string())?,
        cache_root: cache_root.ok_or_else(|| "sync requires --cache <CACHE_ROOT>".to_string())?,
        project_root: project_root.ok_or_else(|| "sync requires a project root".to_string())?,
        remote: finalize_sync_remote(remote)?,
        metadata: finalize_sync_metadata(metadata, pull && pull_snapshot_id.is_none())?,
        object_access,
        push,
        pull,
        pull_snapshot_id,
        target,
        apply,
        once,
        debounce_ms,
        exit_after_idle_ms,
        max_cycles,
    };
    validate_sync_args(&args)?;
    Ok(args)
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
                "local" => SyncRemoteKind::Local,
                "s3" => SyncRemoteKind::S3,
                "hosted" => SyncRemoteKind::Hosted,
                _ => return Err("--remote-kind requires local, s3, or hosted".to_string()),
            };
        }
        "--remote" => {
            *index += 1;
            remote.local_root = Some(PathBuf::from(
                args.get(*index)
                    .ok_or_else(|| "--remote requires a path".to_string())?,
            ));
        }
        "--s3-endpoint" => {
            *index += 1;
            remote.s3_endpoint = args.get(*index).cloned();
        }
        "--s3-bucket" => {
            *index += 1;
            remote.s3_bucket = args.get(*index).cloned();
        }
        "--s3-region" => {
            *index += 1;
            remote.s3_region = args
                .get(*index)
                .cloned()
                .ok_or_else(|| "--s3-region requires a value".to_string())?;
        }
        "--s3-prefix" => {
            *index += 1;
            remote.s3_prefix = args.get(*index).cloned();
        }
        "--s3-access-key-env" => {
            *index += 1;
            remote.s3_access_key_env = args.get(*index).cloned();
        }
        "--s3-secret-key-env" => {
            *index += 1;
            remote.s3_secret_key_env = args.get(*index).cloned();
        }
        "--s3-session-token-env" => {
            *index += 1;
            remote.s3_session_token_env = args.get(*index).cloned();
        }
        _ => return Ok(false),
    }

    Ok(true)
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
                "local-mock" => SyncMetadataMode::LocalMock,
                "mock-dev-sqlite" => SyncMetadataMode::MockDevSqlite,
                _ => {
                    return Err("--metadata-mode requires local-mock or mock-dev-sqlite".to_string())
                }
            };
        }
        "--metadata-db" => {
            *index += 1;
            metadata.db_path =
                Some(PathBuf::from(args.get(*index).ok_or_else(|| {
                    "--metadata-db requires a path".to_string()
                })?));
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

fn parse_object_access_arg(
    flag: &str,
    args: &[String],
    index: &mut usize,
    object_access: &mut ObjectAccessArgs,
) -> Result<bool, String> {
    match flag {
        "--object-access-api" => {
            *index += 1;
            object_access.api = args.get(*index).cloned();
        }
        "--object-access-session-token-env" => {
            *index += 1;
            object_access.session_token_env = args.get(*index).cloned();
        }
        "--object-access-lease" => {
            *index += 1;
            object_access.lease_id = args.get(*index).cloned();
        }
        _ => return Ok(false),
    }

    Ok(true)
}

fn finalize_sync_remote(remote: SyncRemoteArgs) -> Result<SyncRemoteArgs, String> {
    match remote.kind {
        SyncRemoteKind::Local => {
            if remote.local_root.is_none() {
                return Err("sync local remote requires --remote <REMOTE_DIR>".to_string());
            }
            if remote.s3_endpoint.is_some()
                || remote.s3_bucket.is_some()
                || remote.s3_prefix.is_some()
                || remote.s3_access_key_env.is_some()
                || remote.s3_secret_key_env.is_some()
                || remote.s3_session_token_env.is_some()
            {
                return Err("sync s3 flags require --remote-kind s3".to_string());
            }
        }
        SyncRemoteKind::S3 => {
            if remote.local_root.is_some() {
                return Err("sync s3 remote does not accept --remote".to_string());
            }
            if remote.s3_endpoint.is_none() {
                return Err("sync s3 remote requires --s3-endpoint <URL>".to_string());
            }
            if remote.s3_bucket.is_none() {
                return Err("sync s3 remote requires --s3-bucket <BUCKET>".to_string());
            }
            match (&remote.s3_access_key_env, &remote.s3_secret_key_env) {
                (Some(_), Some(_)) | (None, None) => {}
                _ => {
                    return Err(
                        "--s3-access-key-env and --s3-secret-key-env must be provided together"
                            .to_string(),
                    );
                }
            }
            if remote.s3_session_token_env.is_some()
                && (remote.s3_access_key_env.is_none() || remote.s3_secret_key_env.is_none())
            {
                return Err(
                    "--s3-session-token-env requires --s3-access-key-env and --s3-secret-key-env"
                        .to_string(),
                );
            }
            if let Some(name) = &remote.s3_access_key_env {
                validate_env_name(name, "--s3-access-key-env")?;
            }
            if let Some(name) = &remote.s3_secret_key_env {
                validate_env_name(name, "--s3-secret-key-env")?;
            }
            if let Some(name) = &remote.s3_session_token_env {
                validate_env_name(name, "--s3-session-token-env")?;
            }
        }
        SyncRemoteKind::Hosted => {
            if remote.local_root.is_some() {
                return Err("sync hosted remote does not accept --remote".to_string());
            }
            if remote.s3_endpoint.is_some()
                || remote.s3_bucket.is_some()
                || remote.s3_prefix.is_some()
                || remote.s3_access_key_env.is_some()
                || remote.s3_secret_key_env.is_some()
                || remote.s3_session_token_env.is_some()
            {
                return Err(
                    "sync hosted remote uses --object-access-* flags, not --s3-*".to_string(),
                );
            }
        }
    }

    Ok(remote)
}

fn finalize_sync_metadata(
    metadata: SyncMetadataArgs,
    latest_discovery_required: bool,
) -> Result<SyncMetadataArgs, String> {
    if metadata.mode == SyncMetadataMode::LocalMock {
        if metadata.db_path.is_some()
            || metadata.account_id.is_some()
            || metadata.project_id.is_some()
            || metadata.endpoint.is_some()
        {
            return Err("sync metadata flags require --metadata-mode mock-dev-sqlite".to_string());
        }
        if latest_discovery_required {
            return Err(
                "sync pull discovery requires --metadata-mode mock-dev-sqlite or --pull-snapshot"
                    .to_string(),
            );
        }
        return Ok(metadata);
    }

    if metadata.db_path.is_none() {
        return Err(
            "sync requires --metadata-db <DB_PATH> with --metadata-mode mock-dev-sqlite"
                .to_string(),
        );
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

fn validate_sync_args(args: &SyncArgs) -> Result<(), String> {
    match args.remote.kind {
        SyncRemoteKind::Local => {
            if args.object_access.api.is_some()
                || args.object_access.lease_id.is_some()
                || args.object_access.session_token_env.is_some()
            {
                return Err(
                    "live sync local remote does not use --object-access-* flags".to_string(),
                );
            }
        }
        SyncRemoteKind::S3 => {
            if let Some(name) = &args.object_access.session_token_env {
                validate_env_name(name, "--object-access-session-token-env")?;
            }
            if args.remote.s3_prefix.is_none() {
                return Err(
                    "live sync with --remote-kind s3 requires --s3-prefix from object-access grant"
                        .to_string(),
                );
            }
            if args.object_access.api.is_none() || args.object_access.lease_id.is_none() {
                return Err(
                    "live sync with --remote-kind s3 requires --object-access-api and --object-access-lease"
                        .to_string(),
                );
            }
            if args.metadata.mode != SyncMetadataMode::MockDevSqlite
                && args.object_access.session_token_env.is_none()
            {
                return Err(
                    "live sync with --remote-kind s3 requires --object-access-session-token-env"
                        .to_string(),
                );
            }
        }
        SyncRemoteKind::Hosted => {
            if let Some(name) = &args.object_access.session_token_env {
                validate_env_name(name, "--object-access-session-token-env")?;
            }
            if args.object_access.api.is_none() || args.object_access.lease_id.is_none() {
                return Err(
                    "live sync with --remote-kind hosted requires --object-access-api and --object-access-lease"
                        .to_string(),
                );
            }
            if args.metadata.project_id.is_none() {
                return Err(
                    "live sync with --remote-kind hosted requires --metadata-project for object-access scope"
                        .to_string(),
                );
            }
        }
    }

    Ok(())
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

fn parse_u64_flag(flag: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn parse_usize_flag(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn run_watch(args: WatchArgs) -> Result<(), String> {
    preflight_cache_root(&args.cache_root, &args.project_root)
        .map_err(|error| error.to_string())?;
    preflight_db_path(&args.db_path, &args.project_root).map_err(|error| error.to_string())?;

    println!(
        "watch status=start db={} cache={} project={} debounce_ms={} once={} max_scans={}",
        script_value(&args.db_path.display().to_string()),
        script_value(&args.cache_root.display().to_string()),
        script_value(&args.project_root.display().to_string()),
        args.debounce_ms,
        args.once,
        args.max_scans
            .map(|scans| scans.to_string())
            .unwrap_or_else(|| "-".to_string())
    );

    if args.once {
        println!("watch event=batched reason=once events=0");
        run_scan(&args, 1)?;
        println!("watch status=idle scans=1");
        return Ok(());
    }

    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |event| {
            let _ = tx.send(event);
        },
        Config::default(),
    )
    .map_err(|error| format!("watcher_create_failed:{error}"))?;
    watcher
        .watch(&args.project_root, RecursiveMode::Recursive)
        .map_err(|error| format!("watcher_watch_failed:{error}"))?;

    let start = Instant::now();
    let mut planner = DebouncePlanner::new(args.debounce_ms);
    let mut scans = 0usize;
    let mut idle_since = Instant::now();

    loop {
        let timeout = receive_timeout(&planner, start, args.exit_after_idle_ms, idle_since);
        match rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                let pending = planner.record_event(elapsed_ms(start));
                idle_since = Instant::now();
                println!(
                    "watch event=received pending_batch={} kind={:?} paths={}",
                    pending,
                    event.kind,
                    event.paths.len()
                );
            }
            Ok(Err(error)) => {
                eprintln!("watch error={}", script_value(&format!("notify:{error}")));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("watch event channel disconnected".to_string());
            }
        }

        if let Some(batch_size) = planner.take_due_batch(elapsed_ms(start)) {
            println!(
                "watch event=batched reason=debounce events={} debounce_ms={}",
                batch_size, args.debounce_ms
            );
            scans += 1;
            run_scan(&args, scans)?;
            idle_since = Instant::now();
            println!("watch status=idle scans={scans}");

            if args.max_scans.is_some_and(|max_scans| scans >= max_scans) {
                return Ok(());
            }
        }

        if !planner.has_pending()
            && args
                .exit_after_idle_ms
                .is_some_and(|idle_ms| idle_since.elapsed() >= Duration::from_millis(idle_ms))
        {
            println!("watch status=idle_timeout scans={scans}");
            return Ok(());
        }
    }
}

fn run_sync(args: SyncArgs) -> Result<(), String> {
    preflight_cache_root(&args.cache_root, &args.project_root)
        .map_err(|error| error.to_string())?;
    preflight_db_path(&args.db_path, &args.project_root).map_err(|error| error.to_string())?;
    ensure_completed_identity(&args.db_path)?;
    resolve_object_access_for_cloud_remote(&args)?;

    println!(
        "sync status=start db={} cache={} project={} mode={} remote_kind={} metadata_mode={} debounce_ms={} once={} max_cycles={}",
        script_value(&args.db_path.display().to_string()),
        script_value(&args.cache_root.display().to_string()),
        script_value(&args.project_root.display().to_string()),
        sync_mode_label(&args),
        remote_kind_label(args.remote.kind),
        metadata_mode_label(args.metadata.mode),
        args.debounce_ms,
        args.once,
        args.max_cycles
            .map(|cycles| cycles.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    print_remote_config(&args)?;
    print_metadata_config(&args.metadata);

    if args.once {
        println!("sync event=batched reason=once events=0");
        run_sync_cycle(&args, 1)?;
        println!("sync status=idle cycles=1");
        return Ok(());
    }

    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |event| {
            let _ = tx.send(event);
        },
        Config::default(),
    )
    .map_err(|error| format!("watcher_create_failed:{error}"))?;
    watcher
        .watch(&args.project_root, RecursiveMode::Recursive)
        .map_err(|error| format!("watcher_watch_failed:{error}"))?;

    let start = Instant::now();
    let mut planner = DebouncePlanner::new(args.debounce_ms);
    let mut cycles = 0usize;
    let mut idle_since = Instant::now();

    loop {
        let timeout = receive_timeout(&planner, start, args.exit_after_idle_ms, idle_since);
        match rx.recv_timeout(timeout) {
            Ok(Ok(event)) => {
                let pending = planner.record_event(elapsed_ms(start));
                idle_since = Instant::now();
                println!(
                    "sync event=received pending_batch={} kind={:?} paths={}",
                    pending,
                    event.kind,
                    event.paths.len()
                );
            }
            Ok(Err(error)) => {
                eprintln!("sync error={}", script_value(&format!("notify:{error}")));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("sync event channel disconnected".to_string());
            }
        }

        if let Some(batch_size) = planner.take_due_batch(elapsed_ms(start)) {
            println!(
                "sync event=batched reason=debounce events={} debounce_ms={}",
                batch_size, args.debounce_ms
            );
            cycles += 1;
            run_sync_cycle(&args, cycles)?;
            idle_since = Instant::now();
            println!("sync status=idle cycles={cycles}");

            if args
                .max_cycles
                .is_some_and(|max_cycles| cycles >= max_cycles)
            {
                return Ok(());
            }
        }

        if !planner.has_pending()
            && args
                .exit_after_idle_ms
                .is_some_and(|idle_ms| idle_since.elapsed() >= Duration::from_millis(idle_ms))
        {
            println!("sync status=idle_timeout cycles={cycles}");
            return Ok(());
        }
    }
}

fn run_sync_cycle(args: &SyncArgs, cycle_index: usize) -> Result<(), String> {
    let scan = run_scan_for_sync(args, cycle_index)?;
    let local_identity = completed_identity(&args.db_path)?;
    let account_id = args
        .metadata
        .account_id
        .clone()
        .unwrap_or_else(|| local_identity.account_id.clone());
    let project_id = args
        .metadata
        .project_id
        .clone()
        .unwrap_or_else(|| scan.project_id().to_string());

    if args.push && args.metadata.project_id.is_some() && project_id != scan.project_id() {
        return Err(format!(
            "metadata project {} does not match scanned project {}",
            project_id,
            scan.project_id()
        ));
    }

    let mut prechecked_pull_snapshot = None;
    if args.push && args.pull {
        let remote_snapshot = discover_pull_snapshot(args, &account_id, &project_id)?;
        if scan.pending_operations() > 0 {
            ensure_two_way_remote_base_is_safe(
                scan.base_snapshot_id(),
                remote_snapshot.as_deref(),
                &project_id,
            )?;
        } else {
            prechecked_pull_snapshot = remote_snapshot;
        }
    }

    let mut local_pending_after_push = scan.pending_operations();
    if args.push {
        if scan.pending_operations() == 0 {
            println!(
                "sync cycle={} action=publish status=skipped reason=no_local_pending project_id={}",
                cycle_index,
                script_value(&project_id)
            );
        } else {
            let persisted = persist_live_snapshot(args)?;
            if persisted.project_id != project_id {
                return Err(format!(
                    "published project {} does not match metadata project {}",
                    persisted.project_id, project_id
                ));
            }
            let (provider, remote_description) = open_remote_provider(args)?;
            let published = if args.metadata.mode == SyncMetadataMode::MockDevSqlite {
                let mut metadata_store = open_metadata_store(&args.metadata)?;
                publish_snapshot_with_metadata(
                    &PublishSnapshotRequest {
                        db_path: args.db_path.clone(),
                        cache_root: args.cache_root.clone(),
                        snapshot_id: persisted.snapshot_id.clone(),
                    },
                    provider.as_ref(),
                    &mut metadata_store,
                )
                .map_err(|error| error.to_string())?
            } else {
                publish_snapshot(
                    &PublishSnapshotRequest {
                        db_path: args.db_path.clone(),
                        cache_root: args.cache_root.clone(),
                        snapshot_id: persisted.snapshot_id.clone(),
                    },
                    provider.as_ref(),
                )
                .map_err(|error| error.to_string())?
            };
            clear_pending_for_project(&args.db_path, &published.project_id)?;
            local_pending_after_push = 0;
            println!(
                "sync cycle={} action=publish status=ok account_id={} device_id={} project_id={} snapshot_id={} reused_snapshot={} manifest_uploaded={} blobs={} uploaded_blobs={} plaintext_bytes={} remote_bytes={}",
                cycle_index,
                script_value(&published.account_id),
                script_value(&published.device_id),
                script_value(&published.project_id),
                script_value(&published.snapshot_id),
                persisted.reused,
                published.manifest_uploaded,
                published.blob_count,
                published.uploaded_blob_count,
                published.plaintext_blob_bytes,
                published.remote_blob_bytes
            );
            print_remote_description(&remote_description);
        }
    }

    if args.pull {
        if local_pending_after_push > 0 {
            return Err(format!(
                "live sync pull refused: {} local pending changes exist; push or resolve before pulling",
                local_pending_after_push
            ));
        }
        let blocked_secrets = blocked_secret_entry_count(&args.cache_root, &args.project_root)?;
        if blocked_secrets > 0 {
            return Err(format!(
                "live sync pull refused: {blocked_secrets} secret-blocked entries require policy"
            ));
        }
        let pull_snapshot = if let Some(snapshot_id) = prechecked_pull_snapshot {
            Some(snapshot_id)
        } else {
            discover_pull_snapshot(args, &account_id, &project_id)?
        };
        let Some(snapshot_id) = pull_snapshot else {
            println!(
                "sync cycle={} action=pull status=no_remote_snapshot account_id={} project_id={}",
                cycle_index,
                script_value(&account_id),
                script_value(&project_id)
            );
            return Ok(());
        };
        let (provider, remote_description) = open_remote_provider(args)?;
        if let Some(target) = &args.target {
            let outcome = materialize_for_live_sync(
                args,
                provider.as_ref(),
                &account_id,
                &project_id,
                &snapshot_id,
                target,
            )?;
            println!(
                "sync cycle={} action=materialize status=ok account_id={} project_id={} snapshot_id={} target={} apply={} applied={} files_to_write={} skipped_entries={} cursor_value={}",
                cycle_index,
                script_value(&account_id),
                script_value(&outcome.import.project_id),
                script_value(&outcome.import.snapshot_id),
                script_value(&outcome.target.display().to_string()),
                outcome.apply,
                outcome.applied,
                outcome.plan.files_to_write,
                outcome.plan.skipped_entries,
                script_value(&outcome.import.cursor_value)
            );
        } else {
            let imported = import_for_live_sync(
                args,
                provider.as_ref(),
                &account_id,
                &project_id,
                &snapshot_id,
            )?;
            println!(
                "sync cycle={} action=import status=ok source_account_id={} receiver_account_id={} receiver_device_id={} project_id={} snapshot_id={} inserted={} downloaded_blobs={} cursor_value={}",
                cycle_index,
                script_value(&imported.source_account_id),
                script_value(&imported.receiver_account_id),
                script_value(&imported.receiver_device_id),
                script_value(&imported.project_id),
                script_value(&imported.snapshot_id),
                imported.snapshot_inserted,
                imported.downloaded_blob_count,
                script_value(&imported.cursor_value)
            );
        }
        print_remote_description(&remote_description);
    }

    Ok(())
}

fn ensure_two_way_remote_base_is_safe(
    local_base_snapshot_id: Option<&str>,
    remote_snapshot_id: Option<&str>,
    project_id: &str,
) -> Result<(), String> {
    let Some(remote_snapshot_id) = remote_snapshot_id else {
        return Ok(());
    };
    if local_base_snapshot_id == Some(remote_snapshot_id) {
        return Ok(());
    }

    Err(format!(
        "live sync two-way refused before publish: remote latest {} for project {} does not match local base {}; pull or resolve before publishing local changes",
        remote_snapshot_id,
        project_id,
        local_base_snapshot_id.unwrap_or("-")
    ))
}

fn run_scan_for_sync(args: &SyncArgs, scan_index: usize) -> Result<LocalChangeFeedScan, String> {
    let options =
        LocalChangeFeedScanOptions::new(&args.db_path, &args.cache_root, &args.project_root);
    let scan = scan_local_change_feed(&options).map_err(|error| error.to_string())?;
    print_live_scan_summary(scan_index, &scan);
    Ok(scan)
}

fn print_live_scan_summary(scan_index: usize, scan: &LocalChangeFeedScan) {
    let summary = scan.summary();
    println!(
        "sync scan={} project_id={} base_snapshot_id={} created={} modified={} deleted={} unchanged={} skipped_deferred={} pending_operations={} bytes_to_upload={} bytes_deleted={}",
        scan_index,
        script_value(scan.project_id()),
        script_value(scan.base_snapshot_id().unwrap_or("-")),
        summary.created(),
        summary.modified(),
        summary.deleted(),
        summary.unchanged(),
        summary.skipped_deferred(),
        scan.pending_operations(),
        summary.bytes_to_upload(),
        summary.bytes_deleted()
    );
}

fn ensure_completed_identity(db_path: &Path) -> Result<(), String> {
    let _ = completed_identity(db_path)?;
    Ok(())
}

fn completed_identity(db_path: &Path) -> Result<devbox_store::LocalIdentityRecord, String> {
    let store = Store::open_file(db_path).map_err(|error| error.to_string())?;
    store
        .apply_migrations()
        .map_err(|error| error.to_string())?;
    store
        .completed_local_identity()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            "local identity is not initialized; run devbox init --db <DB_PATH>".to_string()
        })
}

fn persist_live_snapshot(args: &SyncArgs) -> Result<LivePersistedSnapshot, String> {
    preflight_cache_root(&args.cache_root, &args.project_root)
        .map_err(|error| error.to_string())?;
    preflight_db_path(&args.db_path, &args.project_root).map_err(|error| error.to_string())?;

    let cache = BlobCache::open(&args.cache_root).map_err(|error| error.to_string())?;
    let snapshot = SnapshotManifestBuilder::new(cache)
        .build_draft(&args.project_root)
        .map_err(|error| error.to_string())?;
    let blocked_secret_entries = snapshot.summary().blocked_secret_entries();
    if blocked_secret_entries > 0 {
        return Err(format!(
            "live sync publish refused: {blocked_secret_entries} secret-blocked entries require policy"
        ));
    }

    let mut store = Store::open_file(&args.db_path).map_err(|error| error.to_string())?;
    store
        .apply_migrations()
        .map_err(|error| error.to_string())?;
    let _ = store
        .completed_local_identity()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            "local identity is not initialized; run devbox init --db <DB_PATH>".to_string()
        })?;
    let created_at = store
        .current_timestamp()
        .map_err(|error| error.to_string())?;
    let project_id = local_project_id(snapshot.root()).to_string();
    let root_path = snapshot.root().display().to_string();
    let display_name = snapshot
        .root()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root_path.clone());
    let parent_snapshot_id = store
        .latest_snapshot_for_project(&project_id)
        .map_err(|error| error.to_string())?
        .map(|snapshot| snapshot.snapshot.id);
    let snapshot_id = snapshot.id().to_string();
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
            kind: "local",
            display_name: &display_name,
            discovered_at: &created_at,
        },
        snapshot: NewSnapshot {
            id: &snapshot_id,
            project_id: &project_id,
            parent_snapshot_id: parent_snapshot_id.as_deref(),
            created_at: &created_at,
            reason: "live-sync",
            manifest_entry_count: snapshot.summary().total_entries() as u64,
            total_size_bytes: snapshot.summary().total_file_bytes(),
        },
        entries,
    };

    let reused = match store.persist_draft_snapshot(&draft) {
        Ok(_) => false,
        Err(StoreError::DuplicateSnapshotId(existing)) if existing == snapshot_id => {
            if store
                .snapshot_with_entries(&snapshot_id)
                .map_err(|error| error.to_string())?
                .is_none()
            {
                return Err(format!(
                    "snapshot already exists but could not be loaded: {snapshot_id}"
                ));
            }
            true
        }
        Err(error) => return Err(error.to_string()),
    };

    Ok(LivePersistedSnapshot {
        project_id,
        snapshot_id,
        reused,
    })
}

fn blocked_secret_entry_count(cache_root: &Path, project_root: &Path) -> Result<usize, String> {
    let cache = BlobCache::open(cache_root).map_err(|error| error.to_string())?;
    let snapshot = SnapshotManifestBuilder::new(cache)
        .build_draft(project_root)
        .map_err(|error| error.to_string())?;
    Ok(snapshot.summary().blocked_secret_entries())
}

fn clear_pending_for_project(db_path: &Path, project_id: &str) -> Result<(), String> {
    let store = Store::open_file(db_path).map_err(|error| error.to_string())?;
    store
        .apply_migrations()
        .map_err(|error| error.to_string())?;
    let cleared = store
        .clear_pending_local_changes(Some(project_id))
        .map_err(|error| error.to_string())?;
    println!(
        "sync pending=cleared project_id={} rows={}",
        script_value(project_id),
        cleared
    );
    Ok(())
}

fn open_remote_provider(
    args: &SyncArgs,
) -> Result<(Box<dyn RemoteBlobProvider>, RemoteProviderDescription), String> {
    match args.remote.kind {
        SyncRemoteKind::Local => {
            let root = args
                .remote
                .local_root
                .as_ref()
                .ok_or_else(|| "local remote requires --remote <REMOTE_DIR>".to_string())?;
            let provider =
                LocalFilesystemBlobProvider::open(root).map_err(|error| error.to_string())?;
            Ok((
                Box::new(provider),
                RemoteProviderDescription::Local { root: root.clone() },
            ))
        }
        SyncRemoteKind::S3 => {
            let config = s3_config_from_args(&args.remote)?;
            let redacted = config.redacted();
            let provider =
                S3CompatibleBlobProvider::from_env(config).map_err(|error| error.to_string())?;
            Ok((
                Box::new(provider),
                RemoteProviderDescription::S3 { redacted },
            ))
        }
        SyncRemoteKind::Hosted => {
            let config = hosted_config_from_args(args)?;
            let redacted = config.redacted();
            let provider = HostedObjectTransferProvider::from_env(config)
                .map_err(|error| error.to_string())?;
            Ok((
                Box::new(provider),
                RemoteProviderDescription::Hosted { redacted },
            ))
        }
    }
}

fn s3_config_from_args(remote: &SyncRemoteArgs) -> Result<S3CompatibleConfig, String> {
    let credentials = match (
        remote.s3_access_key_env.as_ref(),
        remote.s3_secret_key_env.as_ref(),
    ) {
        (Some(access), Some(secret)) => S3CredentialsSource::env(
            access.clone(),
            secret.clone(),
            remote.s3_session_token_env.clone(),
        )
        .map_err(|error| error.to_string())?,
        (None, None) => S3CredentialsSource::default(),
        _ => {
            return Err(
                "--s3-access-key-env and --s3-secret-key-env must be provided together".to_string(),
            );
        }
    };

    S3CompatibleConfig::new(
        remote
            .s3_endpoint
            .as_deref()
            .ok_or_else(|| "--s3-endpoint is required".to_string())?,
        remote
            .s3_bucket
            .as_deref()
            .ok_or_else(|| "--s3-bucket is required".to_string())?,
        &remote.s3_region,
        remote.s3_prefix.as_deref(),
        credentials,
    )
    .map_err(|error| error.to_string())
}

fn hosted_config_from_args(args: &SyncArgs) -> Result<HostedObjectTransferConfig, String> {
    HostedObjectTransferConfig::new(
        args.object_access
            .api
            .as_deref()
            .ok_or_else(|| "--object-access-api is required".to_string())?,
        args.metadata
            .project_id
            .as_deref()
            .ok_or_else(|| "--metadata-project is required".to_string())?,
        args.object_access
            .lease_id
            .as_deref()
            .ok_or_else(|| "--object-access-lease is required".to_string())?,
        args.object_access
            .session_token_env
            .as_deref()
            .unwrap_or("DEVBOX_SESSION_TOKEN"),
    )
    .map_err(|error| error.to_string())
}

fn open_metadata_store(metadata: &SyncMetadataArgs) -> Result<SqliteMetadataStore, String> {
    let path = metadata
        .db_path
        .as_ref()
        .ok_or_else(|| "metadata store path is missing".to_string())?;
    SqliteMetadataStore::open_file(path).map_err(|error| error.to_string())
}

fn discover_pull_snapshot(
    args: &SyncArgs,
    account_id: &str,
    project_id: &str,
) -> Result<Option<String>, String> {
    if let Some(snapshot_id) = &args.pull_snapshot_id {
        println!(
            "sync discovery=explicit account_id={} project_id={} snapshot_id={}",
            script_value(account_id),
            script_value(project_id),
            script_value(snapshot_id)
        );
        return Ok(Some(snapshot_id.clone()));
    }
    let metadata_store = open_metadata_store(&args.metadata)?;
    let latest = metadata_store
        .latest_snapshot(account_id, project_id)
        .map_err(|error| error.to_string())?;
    if let Some(record) = latest {
        println!(
            "sync discovery=latest account_id={} project_id={} snapshot_id={} published_by_device_id={} published_at={}",
            script_value(&record.account_id),
            script_value(&record.project_id),
            script_value(&record.snapshot_id),
            script_value(&record.published_by_device_id),
            script_value(&record.published_at)
        );
        Ok(Some(record.snapshot_id))
    } else {
        Ok(None)
    }
}

fn import_for_live_sync(
    args: &SyncArgs,
    provider: &(impl RemoteBlobProvider + ?Sized),
    account_id: &str,
    project_id: &str,
    snapshot_id: &str,
) -> Result<devbox_materialize::ImportedSnapshotBundle, String> {
    let request = ImportSnapshotRequest {
        db_path: args.db_path.clone(),
        cache_root: args.cache_root.clone(),
        key_source_db_path: None,
        snapshot_id: snapshot_id.to_string(),
    };
    let result = if args.metadata.mode == SyncMetadataMode::MockDevSqlite {
        let mut metadata_store = open_metadata_store(&args.metadata)?;
        let options = HostedMetadataImportOptions {
            account_id: account_id.to_string(),
            project_id: project_id.to_string(),
        };
        import_snapshot_with_metadata(&request, provider, &mut metadata_store, &options)
    } else {
        import_snapshot(&request, provider)
    };
    result.map_err(materialize_error_for_live_sync)
}

fn materialize_for_live_sync(
    args: &SyncArgs,
    provider: &(impl RemoteBlobProvider + ?Sized),
    account_id: &str,
    project_id: &str,
    snapshot_id: &str,
    target: &Path,
) -> Result<devbox_materialize::MaterializationOutcome, String> {
    let request = MaterializationRequest {
        db_path: args.db_path.clone(),
        cache_root: args.cache_root.clone(),
        key_source_db_path: None,
        snapshot_id: snapshot_id.to_string(),
        target: target.to_path_buf(),
        apply: args.apply,
    };
    let result = if args.metadata.mode == SyncMetadataMode::MockDevSqlite {
        let mut metadata_store = open_metadata_store(&args.metadata)?;
        let options = HostedMetadataImportOptions {
            account_id: account_id.to_string(),
            project_id: project_id.to_string(),
        };
        materialize_snapshot_with_metadata(&request, provider, &mut metadata_store, &options)
    } else {
        materialize_snapshot(&request, provider)
    };
    result.map_err(materialize_error_for_live_sync)
}

fn materialize_error_for_live_sync(error: MaterializeError) -> String {
    if let MaterializeError::PreflightBlocked(outcome) = &error {
        println!(
            "sync preflight=blocked project_id={} base_snapshot_id={} local_snapshot_id={} incoming_snapshot_id={} conflict_id={}",
            script_value(&outcome.project_id),
            script_value(outcome.base_snapshot_id.as_deref().unwrap_or("-")),
            script_value(outcome.local_snapshot_id.as_deref().unwrap_or("-")),
            script_value(&outcome.incoming_snapshot_id),
            script_value(outcome.conflict_id().unwrap_or("-"))
        );
        return "live sync pull refused by local preflight".to_string();
    }
    error.to_string()
}

fn resolve_object_access_for_cloud_remote(args: &SyncArgs) -> Result<(), String> {
    if !matches!(
        args.remote.kind,
        SyncRemoteKind::S3 | SyncRemoteKind::Hosted
    ) {
        return Ok(());
    }
    let api = args
        .object_access
        .api
        .as_ref()
        .ok_or_else(|| "missing --object-access-api".to_string())?;
    let lease_id = args
        .object_access
        .lease_id
        .as_ref()
        .ok_or_else(|| "missing --object-access-lease".to_string())?;
    let project_id = args.metadata.project_id.as_ref().ok_or_else(|| {
        "live s3 sync requires --metadata-project for object-access scope".to_string()
    })?;
    let session_token_env = args
        .object_access
        .session_token_env
        .as_deref()
        .unwrap_or("DEVBOX_SESSION_TOKEN");
    let token = secret_from_env(session_token_env, "session token")?;
    let path = format!(
        "/v1/projects/{}/object-access/{}",
        api_path_segment(project_id, "project id")?,
        api_path_segment(lease_id, "lease id")?
    );
    let url = api_url(api, &path)?;
    let grant: ManagedObjectAccessGrant = ureq::post(&url)
        .set("authorization", &format!("Bearer {token}"))
        .send_json(
            serde_json::to_value(ManagedObjectAccessRequest {
                required_capabilities: vec![
                    ManagedObjectCapability::Read,
                    ManagedObjectCapability::Write,
                    ManagedObjectCapability::List,
                    ManagedObjectCapability::Head,
                ],
            })
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| {
            format!(
                "object access grant request failed: {}",
                redact_http_error(error)
            )
        })?
        .into_json()
        .map_err(|error| format!("object access grant response was invalid: {error}"))?;
    validate_object_access_grant(args, &grant)?;
    println!(
        "object_access status=resolved api={} session_token_env={} lease_id={} account_id={} project_id={} prefix={} credential_delivery={} client_bucket_credentials=false",
        script_value(api),
        script_value(session_token_env),
        script_value(&grant.lease_id),
        script_value(&grant.account_id),
        script_value(&grant.project_id),
        script_value(&grant.prefix),
        grant.credential_delivery
    );
    Ok(())
}

fn validate_object_access_grant(
    args: &SyncArgs,
    grant: &ManagedObjectAccessGrant,
) -> Result<(), String> {
    let expected_project_id = args
        .metadata
        .project_id
        .as_deref()
        .ok_or_else(|| "metadata project is missing".to_string())?;
    if grant.project_id != expected_project_id {
        return Err("object access grant project did not match --metadata-project".to_string());
    }
    if let Some(account_id) = args.metadata.account_id.as_deref() {
        if grant.account_id != account_id {
            return Err("object access grant account did not match --metadata-account".to_string());
        }
    }
    if args.remote.kind == SyncRemoteKind::S3 {
        let s3_prefix = args
            .remote
            .s3_prefix
            .as_deref()
            .ok_or_else(|| "s3 prefix is missing".to_string())?;
        if grant.prefix != s3_prefix {
            return Err("object access grant prefix did not match --s3-prefix".to_string());
        }
        if args
            .remote
            .s3_bucket
            .as_deref()
            .is_some_and(|bucket| bucket != grant.bucket)
        {
            return Err("object access grant bucket did not match --s3-bucket".to_string());
        }
        if args.remote.s3_region != grant.region {
            return Err("object access grant region did not match --s3-region".to_string());
        }
    }
    Ok(())
}

fn secret_from_env(name: &str, label: &'static str) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err(format!("{label} env name must not be empty"));
    }
    std::env::var(name).map_err(|_| format!("{label} env var {name} is not set"))
}

fn api_url(base: &str, path: &str) -> Result<String, String> {
    let check = MetadataServiceConfig {
        endpoint: base.to_string(),
        auth_mode: MetadataAuthMode::AccountSession,
    }
    .validate()
    .map_err(|error| error.to_string())?;
    Ok(format!("{}{}", check.endpoint.trim_end_matches('/'), path))
}

fn api_path_segment(value: &str, label: &'static str) -> Result<String, String> {
    if value.trim().is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
    {
        return Err(format!("{label} must be a safe API path segment"));
    }
    Ok(value.to_string())
}

fn redact_http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, _) => format!("http_status_{status}"),
        ureq::Error::Transport(_) => "transport_error".to_string(),
    }
}

fn print_remote_config(args: &SyncArgs) -> Result<(), String> {
    match args.remote.kind {
        SyncRemoteKind::Local => {
            println!(
                "remote provider=local root={} credentials=not_used",
                script_value(
                    &args
                        .remote
                        .local_root
                        .as_ref()
                        .ok_or_else(|| "local remote root is missing".to_string())?
                        .display()
                        .to_string()
                )
            );
        }
        SyncRemoteKind::S3 => {
            let config = s3_config_from_args(&args.remote)?;
            print_remote_description(&RemoteProviderDescription::S3 {
                redacted: config.redacted(),
            });
        }
        SyncRemoteKind::Hosted => {
            let config = hosted_config_from_args(args)?;
            print_remote_description(&RemoteProviderDescription::Hosted {
                redacted: config.redacted(),
            });
        }
    }
    Ok(())
}

fn print_remote_description(description: &RemoteProviderDescription) {
    match description {
        RemoteProviderDescription::Local { root } => {
            println!(
                "remote provider=local root={} credentials=not_used",
                script_value(&root.display().to_string())
            );
        }
        RemoteProviderDescription::S3 { redacted } => {
            println!(
                "remote provider=s3-compatible endpoint_host={} bucket={} region={} prefix={} access_key_env={} secret_key_env={} session_token_env={}",
                script_value(&redacted.endpoint_host),
                script_value(&redacted.bucket),
                script_value(&redacted.region),
                script_value(redacted.prefix.as_deref().unwrap_or("-")),
                script_value(&redacted.access_key_env),
                script_value(&redacted.secret_key_env),
                script_value(redacted.session_token_env.as_deref().unwrap_or("-"))
            );
        }
        RemoteProviderDescription::Hosted { redacted } => {
            println!(
                "remote provider=hosted-object-transfer api_host={} project_id={} lease_id={} session_token_env={} client_bucket_credentials=false",
                script_value(&redacted.api_host),
                script_value(&redacted.project_id),
                script_value(&redacted.lease_id),
                script_value(&redacted.session_token_env)
            );
        }
    }
}

fn print_metadata_config(metadata: &SyncMetadataArgs) {
    match metadata.mode {
        SyncMetadataMode::LocalMock => {
            println!("metadata mode=local-mock discovery=explicit_snapshot_only");
        }
        SyncMetadataMode::MockDevSqlite => {
            println!(
                "metadata mode=mock-dev-sqlite db={} account_id={} project_id={} endpoint={}",
                metadata
                    .db_path
                    .as_ref()
                    .map(|path| script_value(&path.display().to_string()))
                    .unwrap_or_else(|| "-".to_string()),
                script_value(metadata.account_id.as_deref().unwrap_or("-")),
                script_value(metadata.project_id.as_deref().unwrap_or("-")),
                script_value(metadata.endpoint.as_deref().unwrap_or("-"))
            );
        }
    }
}

fn sync_mode_label(args: &SyncArgs) -> &'static str {
    match (args.push, args.pull) {
        (true, true) => "two-way",
        (true, false) => "push",
        (false, true) => "pull",
        (false, false) => "none",
    }
}

fn remote_kind_label(kind: SyncRemoteKind) -> &'static str {
    match kind {
        SyncRemoteKind::Local => "local",
        SyncRemoteKind::S3 => "s3",
        SyncRemoteKind::Hosted => "hosted",
    }
}

fn metadata_mode_label(mode: SyncMetadataMode) -> &'static str {
    match mode {
        SyncMetadataMode::LocalMock => "local-mock",
        SyncMetadataMode::MockDevSqlite => "mock-dev-sqlite",
    }
}

fn run_scan(args: &WatchArgs, scan_index: usize) -> Result<LocalChangeFeedScan, String> {
    let options =
        LocalChangeFeedScanOptions::new(&args.db_path, &args.cache_root, &args.project_root);
    let scan = scan_local_change_feed(&options).map_err(|error| error.to_string())?;
    print_scan_summary(scan_index, &scan);
    Ok(scan)
}

fn print_scan_summary(scan_index: usize, scan: &LocalChangeFeedScan) {
    let summary = scan.summary();
    println!(
        "watch scan={} project_id={} base_snapshot_id={} created={} modified={} deleted={} unchanged={} skipped_deferred={} pending_operations={} bytes_to_upload={} bytes_deleted={}",
        scan_index,
        script_value(scan.project_id()),
        script_value(scan.base_snapshot_id().unwrap_or("-")),
        summary.created(),
        summary.modified(),
        summary.deleted(),
        summary.unchanged(),
        summary.skipped_deferred(),
        scan.pending_operations(),
        summary.bytes_to_upload(),
        summary.bytes_deleted()
    );
}

fn receive_timeout(
    planner: &DebouncePlanner,
    start: Instant,
    exit_after_idle_ms: Option<u64>,
    idle_since: Instant,
) -> Duration {
    let debounce_timeout = planner.next_scan_at_ms().map(|deadline| {
        let now = elapsed_ms(start);
        Duration::from_millis(deadline.saturating_sub(now))
    });
    let idle_timeout = exit_after_idle_ms
        .map(|idle_ms| Duration::from_millis(idle_ms).saturating_sub(idle_since.elapsed()));

    match (debounce_timeout, idle_timeout) {
        (Some(left), Some(right)) => left.min(right),
        (Some(timeout), None) | (None, Some(timeout)) => timeout,
        (None, None) => Duration::from_secs(60),
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn script_value(value: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => sanitized.push('/'),
            ' ' => sanitized.push_str("%20"),
            ch if ch.is_control() => sanitized.push('_'),
            ch => sanitized.push(ch),
        }
    }
    sanitized
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DebouncePlanner {
    debounce_ms: u64,
    pending_events: usize,
    next_scan_at_ms: Option<u64>,
}

impl DebouncePlanner {
    fn new(debounce_ms: u64) -> Self {
        Self {
            debounce_ms,
            pending_events: 0,
            next_scan_at_ms: None,
        }
    }

    fn record_event(&mut self, now_ms: u64) -> usize {
        self.pending_events += 1;
        self.next_scan_at_ms = Some(now_ms.saturating_add(self.debounce_ms));
        self.pending_events
    }

    fn take_due_batch(&mut self, now_ms: u64) -> Option<usize> {
        let due = self
            .next_scan_at_ms
            .is_some_and(|deadline| deadline <= now_ms);
        if !due {
            return None;
        }

        let batch_size = self.pending_events;
        self.pending_events = 0;
        self.next_scan_at_ms = None;
        Some(batch_size)
    }

    fn next_scan_at_ms(&self) -> Option<u64> {
        self.next_scan_at_ms
    }

    fn has_pending(&self) -> bool {
        self.pending_events > 0
    }
}

fn print_help() {
    println!("devbox-daemon {VERSION}");
    println!();
    println!("Usage: devbox-daemon <COMMAND>");
    println!();
    println!("Commands:");
    println!("  watch    Watch a project tree and feed the local pending change log");
    println!("  sync     Watch/scan a project and run live snapshot publish or pull");
    println!();
    println!("Watch usage:");
    println!(
        "  devbox-daemon watch --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT> [--once] [--debounce-ms <MS>] [--exit-after-idle-ms <MS>] [--max-scans <N>]"
    );
    println!();
    println!("Sync usage:");
    println!(
        "  devbox-daemon sync --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> [--metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>] [--push|--pull|--two-way] <PROJECT_ROOT> [--once] [--debounce-ms <MS>] [--exit-after-idle-ms <MS>] [--max-cycles <N>]"
    );
    println!(
        "  devbox-daemon sync --pull --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-account <ACCOUNT_ID> --metadata-project <PROJECT_ID> [--to <TARGET_DIR> --apply] <PROJECT_ROOT> [--once]"
    );
    println!(
        "  Add --remote-kind hosted plus --object-access-api/--object-access-lease and --metadata-project for external hosted object transfer without client bucket keys."
    );
    println!(
        "  Add --remote-kind s3 plus --s3-* flags and --object-access-api/--object-access-lease only for trusted-operator direct R2/S3 smoke."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_batches_bursts_until_quiet_window_elapses() {
        let mut planner = DebouncePlanner::new(100);

        assert_eq!(planner.record_event(0), 1);
        assert_eq!(planner.next_scan_at_ms(), Some(100));
        assert_eq!(planner.record_event(50), 2);
        assert_eq!(planner.next_scan_at_ms(), Some(150));
        assert_eq!(planner.take_due_batch(149), None);
        assert_eq!(planner.take_due_batch(150), Some(2));
        assert_eq!(planner.next_scan_at_ms(), None);
    }

    #[test]
    fn parse_watch_args_defaults_to_debounced_long_running_watch() {
        let args = vec![
            "--db".to_string(),
            "devbox.sqlite3".to_string(),
            "--cache".to_string(),
            "cache".to_string(),
            "project".to_string(),
        ];

        let parsed = parse_watch_args(&args).expect("args parse");

        assert!(!parsed.once);
        assert_eq!(parsed.debounce_ms, DEFAULT_DEBOUNCE_MS);
        assert_eq!(parsed.max_scans, None);
        assert_eq!(parsed.project_root, PathBuf::from("project"));
    }

    #[test]
    fn parse_watch_args_accepts_deterministic_test_flags() {
        let args = vec![
            "--db".to_string(),
            "devbox.sqlite3".to_string(),
            "--cache".to_string(),
            "cache".to_string(),
            "--once".to_string(),
            "--debounce-ms".to_string(),
            "25".to_string(),
            "--exit-after-idle-ms".to_string(),
            "50".to_string(),
            "--max-scans".to_string(),
            "2".to_string(),
            "project".to_string(),
        ];

        let parsed = parse_watch_args(&args).expect("args parse");

        assert!(parsed.once);
        assert_eq!(parsed.debounce_ms, 25);
        assert_eq!(parsed.exit_after_idle_ms, Some(50));
        assert_eq!(parsed.max_scans, Some(2));
    }

    #[test]
    fn script_value_neutralizes_control_characters_for_status_lines() {
        assert_eq!(
            script_value("account one\\project\nnext"),
            "account%20one/project_next"
        );
    }
}
