use devbox_core::scanner::ProjectScanner;
use devbox_core::{BlobId, ManifestEntryKind, PolicyDecision};
use devbox_snapshot::{
    preflight_cache_root, preflight_db_path, scan_local_change_feed, LocalChangeFeedScanOptions,
    RestoreMaterializer, RestorePlan, RestoreSkippedEntry, RestoreTargetStatus, RestoreWrite,
    SnapshotManifestBuilder,
};
use devbox_store::{
    local_project_id, path_to_store_string, BlobCache, DeviceRecord, EnsureLocalIdentityOptions,
    LocalChangeKind, ManifestEntryRecord, NewProject, NewSnapshot, NewSnapshotDraft,
    NewSnapshotManifestEntry, PendingLocalChangeRecord, PersistedSnapshot, Store,
};
use devbox_sync::{
    download_blob_to_cache, encrypted_blob_object_key, upload_blob_from_cache,
    LocalFilesystemBlobProvider, ObjectKey, SyncKey,
};
use std::path::Path;
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
        Some("devices") => run_devices(&args[1..]),
        Some("sync") => run_sync(&args[1..]),
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
struct InitArgs {
    db_path: String,
    device_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncBlobArgs {
    db_path: String,
    cache_root: String,
    remote_root: String,
    blob_id: String,
    object_key: Option<String>,
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
        _ => {
            eprintln!("devbox: devices requires list");
            eprintln!("Usage: devbox devices list --db <DB_PATH>");
            ExitCode::from(2)
        }
    }
}

fn run_sync(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
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
        _ => {
            eprintln!("devbox: sync requires upload or download");
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

fn parse_sync_blob_args(args: &[String]) -> Result<SyncBlobArgs, String> {
    let mut db_path = None;
    let mut cache_root = None;
    let mut remote_root = None;
    let mut object_key = None;
    let mut blob_id = None;
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
            "--remote" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--remote requires a directory".to_string())?;
                remote_root = Some(value.clone());
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
        remote_root: remote_root
            .ok_or_else(|| "sync requires --remote <REMOTE_DIR>".to_string())?,
        blob_id: blob_id.ok_or_else(|| "sync requires a blob id".to_string())?,
        object_key,
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
    let devices = store.list_devices()?;

    println!("Device id\tAccount id\tCurrent local\tName\tLast seen at\tCreated at");
    for device in &devices {
        print_device(device);
    }

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
    let provider = LocalFilesystemBlobProvider::open(&args.remote_root)?;
    let uploaded = upload_blob_from_cache(&cache, &provider, &sync_key, &blob_id, &object_key)?;

    println!("Sync upload: encrypted local-remote object");
    println!("Blob id: {blob_id}");
    println!("Object key: {}", uploaded.object_key);
    println!("Plaintext bytes: {}", uploaded.plaintext_bytes);
    println!("Remote bytes: {}", uploaded.remote_bytes);
    println!("Uploaded: {}", uploaded.uploaded);
    println!("Remote provider: local filesystem");
    println!("Remote root: {}", provider.root().display());
    println!("Cloud authentication: not configured");

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
    let provider = LocalFilesystemBlobProvider::open(&args.remote_root)?;
    let downloaded = download_blob_to_cache(&cache, &provider, &sync_key, &blob_id, &object_key)?;

    println!("Sync download: decrypted into local blob cache");
    println!("Blob id: {blob_id}");
    println!("Object key: {}", downloaded.object_key);
    println!("Plaintext bytes: {}", downloaded.plaintext_bytes);
    println!("Remote bytes: {}", downloaded.remote_bytes);
    println!("Blob cache: {}", cache.root().display());
    println!("Remote provider: local filesystem");
    println!("Remote root: {}", provider.root().display());
    println!("Cloud authentication: not configured");

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

fn print_device(device: &DeviceRecord) {
    println!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        device.id,
        device.account_id,
        device.is_local,
        device.display_name,
        device.last_seen_at,
        device.created_at
    );
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
    let (included_files, included_directories, included_symlinks, deferred_entries, excluded) =
        summarize_entries(&persisted.entries);

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
    println!(
        "Included file bytes: {}",
        persisted.snapshot.total_size_bytes
    );
    println!("SQLite database: {db_path}");
    println!("Blob cache: {cache_root}");
}

fn print_snapshot_detail(persisted: &PersistedSnapshot) {
    let (included_files, included_directories, included_symlinks, deferred_entries, excluded) =
        summarize_entries(&persisted.entries);

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

fn summarize_entries(entries: &[ManifestEntryRecord]) -> (usize, usize, usize, usize, usize) {
    let mut included_files = 0;
    let mut included_directories = 0;
    let mut included_symlinks = 0;
    let mut deferred_entries = 0;
    let mut excluded_entries = 0;

    for entry in entries {
        match &entry.policy_decision {
            PolicyDecision::Include => match entry.kind {
                devbox_core::ManifestEntryKind::File => included_files += 1,
                devbox_core::ManifestEntryKind::Directory => included_directories += 1,
                devbox_core::ManifestEntryKind::Symlink => included_symlinks += 1,
                devbox_core::ManifestEntryKind::Unsupported => deferred_entries += 1,
            },
            PolicyDecision::Exclude { .. } => excluded_entries += 1,
            PolicyDecision::RequiresUserDecision { .. } => deferred_entries += 1,
        }
    }

    (
        included_files,
        included_directories,
        included_symlinks,
        deferred_entries,
        excluded_entries,
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

fn print_sync_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  devbox sync upload --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <BLOB_ID> [--object-key <KEY>]"
    );
    eprintln!(
        "  devbox sync download --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <BLOB_ID> [--object-key <KEY>]"
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
    println!("  devices    List known local account devices");
    println!("  sync       Upload and download encrypted blobs through a local remote provider");
    println!("  snapshot   Build, persist, list, show, and restore local snapshot manifests");
    println!("  changes    Scan, list, and clear the pending local change feed");
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
}
