use bindhub_metadata::{MetadataAuthMode, MetadataServiceConfig};
use bindhub_remote::{BindhubHostedRemote, BindhubHostedRemoteConfig, BINDHUB_HOSTED_REMOTE_KIND};
use loom_core::{FileKind, ObjectId, RevisionBoundary, SharedFolderId};
use loom_daemon::{DaemonLoopOptions, DaemonStartOptions};
use loom_store::{path_to_store_string, LocalStore, RemoteConfig, StoreError};
use loom_sync::{
    hydrate_object_from_remote, import_pack_from_remote_with_progress,
    import_pack_metadata_only_from_remote, sync_store_to_remote_with_progress, LoomRemote,
    SyncError, SyncProgress, DEFAULT_CURSOR_ID, DEFAULT_REMOTE_NAME,
};
use loom_worktree::{
    cache_status_for_scope, evaluate_directory_policy, hydrate_versions, prune_cache_to_limit,
    relative_scope_path, tracked_versions_for_scope, warm_versions_for_scope, CaptureEngine,
    CaptureRequest, DirectoryPolicyDecision, RestoreEngine, WorktreeCapture,
    DEFAULT_WARM_MAX_BYTES,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};
use ureq::Error as UreqError;

const CONFIG_DIR_ENV: &str = "BINDHUB_CONFIG_DIR";
const API_URL_ENV: &str = "BINDHUB_API_URL";
const WEB_URL_ENV: &str = "BINDHUB_WEB_URL";
const LOCAL_DEV_API_URL: &str = "http://127.0.0.1:8787";
const LOCAL_DEV_WEB_URL: &str = "http://localhost:3000";
const DEFAULT_UPDATE_REPO: &str = "Snowy7/devbox";
const UPDATE_INSTALLER_BRANCH: &str = "main";
const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProductConfig {
    api_url: Option<String>,
    account_id: Option<String>,
    session_id: Option<String>,
    session_token: Option<String>,
    device_id: Option<String>,
    device_name: Option<String>,
    shared_folders: Vec<ManagedFolder>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManagedFolder {
    name: String,
    folder_id: String,
    local_path: String,
    paused: bool,
}

#[derive(Debug, Deserialize)]
struct DevSessionResponse {
    account_id: String,
    session_id: String,
    session_token: String,
    device_id: String,
}

#[derive(Debug, Deserialize)]
struct CliDeviceFlowResponse {
    device_code: String,
    user_code: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct CliDeviceFlowPollResponse {
    status: String,
    account_id: Option<String>,
    session_id: Option<String>,
    session_token: Option<String>,
    device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SharedFolderResponse {
    id: String,
    role: String,
    display_name: String,
}

#[derive(Debug, Clone)]
struct Session {
    api_url: String,
    account_id: String,
    session_token: String,
    device_id: String,
    device_name: String,
}

#[derive(Debug, Clone)]
struct LoginArgs {
    api_url: String,
    account_hint: String,
    device_name: String,
    web_url: String,
    open_browser: bool,
    local_dev_direct: bool,
}

#[derive(Debug, Clone)]
struct ShareArgs {
    folder: PathBuf,
    start_background_sync: bool,
}

#[derive(Debug, Clone)]
struct CloneArgs {
    name: Option<String>,
    target: Option<PathBuf>,
    start_background_sync: bool,
    sparse: bool,
}

#[derive(Debug, Clone)]
struct ResumeArgs {
    target: Option<String>,
    start_background_sync: bool,
}

#[derive(Debug, Clone)]
struct SparsePathArgs {
    target: PathBuf,
    max_bytes: u64,
    manifest_only: bool,
}

#[derive(Debug, Clone)]
struct HydrateArgs {
    target: PathBuf,
}

#[derive(Debug, Clone)]
struct KeepArgs {
    target: PathBuf,
}

#[derive(Debug, Clone)]
struct FreeSpaceArgs {
    target: PathBuf,
    max_bytes: u64,
}

#[derive(Debug, Clone)]
struct DaemonEntryArgs {
    folder: PathBuf,
    debounce_ms: u64,
    poll_ms: u64,
    max_cycles: Option<usize>,
}

#[derive(Debug, Clone)]
struct UpdateArgs {
    version: String,
    repo: String,
    yes: bool,
}

pub fn run_command(command: &str, args: &[String]) -> ExitCode {
    if args
        .first()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        print_product_command_help(command);
        return ExitCode::SUCCESS;
    }

    match command {
        "login" => result_to_exit(parse_login_args(args).and_then(run_login)),
        "share" => result_to_exit(parse_share_args(args).and_then(run_share)),
        "clone" => result_to_exit(parse_clone_args(args).and_then(run_clone)),
        "pause" => result_to_exit(run_pause(args)),
        "resume" => result_to_exit(parse_resume_args(args).and_then(run_resume)),
        "unlink" => result_to_exit(run_unlink(args)),
        "warm" => result_to_exit(parse_sparse_path_args("warm", args).and_then(run_warm)),
        "hydrate" => result_to_exit(parse_hydrate_args(args).and_then(run_hydrate)),
        "keep" => result_to_exit(parse_keep_args(args).and_then(run_keep)),
        "free-space" => result_to_exit(parse_free_space_args(args).and_then(run_free_space)),
        "doctor" => result_to_exit(run_doctor(args)),
        "update" => result_to_exit(parse_update_args(args).and_then(run_update)),
        "manage" => {
            println!("bindhub manage: shared-folder management will move here after the MVP CLI.");
            println!("For now, use bindhub status, warm, hydrate, keep, free-space, pause, resume, or unlink.");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("bindhub: unknown product command '{command}'");
            ExitCode::from(2)
        }
    }
}

pub fn run_status(args: &[String]) -> ExitCode {
    match product_status(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bindhub: {error}");
            ExitCode::from(1)
        }
    }
}

pub fn run_loom_daemon_entrypoint(args: &[String]) -> ExitCode {
    match parse_daemon_entry_args(args).and_then(|parsed| {
        let mut options = DaemonLoopOptions::new(parsed.folder);
        options.debounce_ms = parsed.debounce_ms;
        options.poll_ms = parsed.poll_ms;
        options.max_cycles = parsed.max_cycles;
        loom_daemon::run_loop(&options).map_err(product_daemon_error)
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bindhub: background sync failed: {error}");
            ExitCode::from(1)
        }
    }
}

pub fn print_product_command_help(command: &str) {
    let usage = match command {
        "login" => {
            "bindhub login [--api <URL>] [--web <URL>] [--device-name <NAME>] [--no-browser]\n       bindhub login --local-dev-direct [--api <URL>] [--account <NAME>] [--device-name <NAME>]"
        }
        "share" => "bindhub share <FOLDER> [--no-background-sync]",
        "clone" => {
            "bindhub clone\n       bindhub clone <SHARED_FOLDER> [TARGET] [--sparse] [--no-background-sync]"
        }
        "manage" => "bindhub manage <SHARED_FOLDER>",
        "doctor" => "bindhub doctor",
        "pause" => "bindhub pause [SHARED_FOLDER]",
        "resume" => "bindhub resume [SHARED_FOLDER] [--no-background-sync]",
        "unlink" => "bindhub unlink [SHARED_FOLDER]",
        "warm" => "bindhub warm <PATH|FOLDER> [--manifest] [--max-bytes <BYTES>]",
        "hydrate" => "bindhub hydrate <PATH|FOLDER>",
        "keep" => "bindhub keep <PATH|FOLDER>",
        "free-space" => "bindhub free-space <PATH|FOLDER> [--max-bytes <BYTES>]",
        "update" => "bindhub update [--yes] [--version <TAG>] [--repo <OWNER/REPO>]",
        _ => "bindhub <COMMAND>",
    };

    println!("bindhub {command}");
    println!();
    println!("Usage: {usage}");
    println!();
    println!("bindhub keeps folders continuous across machines.");
}

fn run_update(args: UpdateArgs) -> Result<(), String> {
    let command = update_install_command(&args);
    println!("Bindhub updater");
    println!("Current version: {}", env!("CARGO_PKG_VERSION"));
    println!("Release source: {}", args.repo);
    println!("Requested version: {}", args.version);

    if !args.yes {
        println!("Installer command:");
        println!("{command}");
        println!("Run bindhub update --yes to execute it.");
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "$ErrorActionPreference = 'Stop'; $script = Join-Path $env:TEMP 'bindhub-install-{}.ps1'; Invoke-RestMethod -Uri '{}' -OutFile $script; & $script -Version '{}' -Repo '{}'",
            std::process::id(),
            update_installer_url(&args.repo, "install-bindhub.ps1"),
            powershell_single_quote(&args.version),
            powershell_single_quote(&args.repo)
        );
        Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .spawn()
            .map_err(|error| format!("could not start updater: {error}"))?;
        println!("Updater started in the background. Open a new terminal after it finishes.");
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let script = format!(
            "BINDHUB_REPO={} curl -fsSL {} | sh -s -- {}",
            sh_single_quote(&args.repo),
            sh_single_quote(&update_installer_url(&args.repo, "install-bindhub.sh")),
            sh_single_quote(&args.version)
        );
        let status = Command::new("sh")
            .args(["-c", &script])
            .status()
            .map_err(|error| format!("could not start updater: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("updater exited with status {status}"))
        }
    }
}

fn run_doctor(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("doctor does not accept arguments\nUsage: bindhub doctor".to_string());
    }

    let config = read_config()?;
    let Some(session) = config.session_or_none() else {
        println!("Logged in: no");
        println!("Next step: run bindhub login to connect this machine.");
        return Ok(());
    };

    println!("Logged in: yes");
    println!("Account: {}", session.account_id);
    println!("Machine: {} ({})", session.device_name, session.device_id);
    println!("Session: stored locally");

    if config.shared_folders.is_empty() {
        println!("Shared folders: none yet");
        println!("Next step: run bindhub share <folder> or bindhub clone.");
        return Ok(());
    }

    println!("Shared folders:");
    for folder in &config.shared_folders {
        println!("- {}: {}", folder.name, folder.local_path);
        println!("  State: {}", managed_folder_sync_state(folder));
        print_folder_diagnostics(folder);
    }
    Ok(())
}

fn run_login(args: LoginArgs) -> Result<(), String> {
    let mut config = read_config()?;
    let response = if args.local_dev_direct {
        local_dev_login(&args)?
    } else {
        browser_login(&args)?
    };

    config.api_url = Some(args.api_url);
    config.account_id = Some(response.account_id.clone());
    config.session_id = Some(response.session_id.clone());
    config.session_token = Some(response.session_token);
    config.device_id = Some(response.device_id.clone());
    config.device_name = Some(args.device_name);
    write_config(&config)?;

    println!("Logged in to Bindhub");
    println!("Account: {}", response.account_id);
    println!(
        "Machine: {}",
        config
            .device_name
            .unwrap_or_else(|| response.device_id.clone())
    );
    println!("Session: stored locally");
    println!("Token: not printed");
    Ok(())
}

fn local_dev_login(args: &LoginArgs) -> Result<DevSessionResponse, String> {
    ensure_local_dev_direct_api_allowed(&args.api_url)?;
    let request = serde_json::json!({
        "account_hint": args.account_hint,
        "device_id": stable_device_id(&args.device_name),
        "device_display_name": args.device_name,
    });
    send_json(
        ureq::post(&api_url(&args.api_url, "/v1/auth/dev-session")?),
        request,
    )
}

fn browser_login(args: &LoginArgs) -> Result<DevSessionResponse, String> {
    let request = serde_json::json!({
        "device_id": stable_device_id(&args.device_name),
        "device_display_name": args.device_name,
    });
    let flow: CliDeviceFlowResponse = send_json(
        ureq::post(&api_url(&args.api_url, "/v1/auth/cli-device-flow")?),
        request,
    )?;
    let verification_url = cli_verification_url(&args.web_url, &flow.user_code)
        .unwrap_or_else(|| flow.verification_uri_complete.clone());

    println!("Open this link to connect this machine:");
    println!("{verification_url}");
    println!("User code: {}", flow.user_code);
    println!("Waiting for browser sign-in...");

    if args.open_browser {
        if open_browser_url(&verification_url).is_err() {
            println!("Browser: could not open automatically");
        }
    }

    poll_cli_device_flow(args, &flow)
}

fn poll_cli_device_flow(
    args: &LoginArgs,
    flow: &CliDeviceFlowResponse,
) -> Result<DevSessionResponse, String> {
    let deadline = Instant::now() + Duration::from_secs(flow.expires_in);
    let interval = Duration::from_secs(flow.interval.max(1));
    while Instant::now() < deadline {
        let response: CliDeviceFlowPollResponse = call_json(ureq::get(&api_url(
            &args.api_url,
            &format!(
                "/v1/auth/cli-device-flow/{}",
                api_path_segment(&flow.device_code)?
            ),
        )?))?;
        match response.status.as_str() {
            "pending" => std::thread::sleep(interval),
            "approved" => {
                return Ok(DevSessionResponse {
                    account_id: response
                        .account_id
                        .ok_or_else(|| "browser login response was incomplete".to_string())?,
                    session_id: response
                        .session_id
                        .ok_or_else(|| "browser login response was incomplete".to_string())?,
                    session_token: response
                        .session_token
                        .ok_or_else(|| "browser login response was incomplete".to_string())?,
                    device_id: response
                        .device_id
                        .ok_or_else(|| "browser login response was incomplete".to_string())?,
                });
            }
            _ => return Err("browser login response was invalid".to_string()),
        }
    }

    Err("browser login timed out; run bindhub login again".to_string())
}

fn run_share(args: ShareArgs) -> Result<(), String> {
    let mut config = read_config()?;
    let session = config.session()?;
    let opened = LocalStore::open_or_init(&args.folder).map_err(product_store_error)?;
    let store = opened.into_store();
    let folder_id = store.shared_folder().id().as_str().to_string();
    let name = store.shared_folder().display_name().to_string();

    ensure_shared_folder(&session, &folder_id, &name)?;
    configure_hosted_remote(&store, &session)?;
    let synced = sync_once(&store, &session)?;
    upsert_managed_folder(&mut config, &store, false)?;
    write_config(&config)?;

    println!("Shared folder: {name}");
    println!("Folder: {}", store.folder_root().display());
    println!("Sync: up to date");
    println!("Files captured: {}", synced.summary().captured_files());
    start_background_sync(&store, args.start_background_sync)?;
    Ok(())
}

fn run_clone(args: CloneArgs) -> Result<(), String> {
    let mut config = read_config()?;
    let session = config.session()?;
    let folders = list_shared_folders(&session)?;

    let Some(name) = args.name else {
        print_cloneable_folders(&config, &folders);
        return Ok(());
    };
    let folder = find_shared_folder(&folders, &name)?;
    let target = args
        .target
        .unwrap_or_else(|| PathBuf::from(safe_folder_name(&folder.display_name)));
    let folder_id = SharedFolderId::new(folder.id.clone()).map_err(|error| error.to_string())?;
    let remote_config = BindhubHostedRemoteConfig::new(
        &session.api_url,
        folder_id,
        &session.session_token,
        &session.device_id,
    )
    .map_err(|error| error.to_string())?;
    let remote = BindhubHostedRemote::new(remote_config.clone());
    let remote_revision_id = remote
        .get_cursor(DEFAULT_CURSOR_ID)
        .map_err(product_sync_error)?
        .ok_or_else(|| "that shared folder has not synced yet".to_string())?;
    let pack = remote
        .get_pack(&remote_revision_id)
        .map_err(product_sync_error)?;

    validate_clone_target_before_mutation(&target)?;
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    let store = LocalStore::init_clone(
        &target,
        pack.manifest.shared_folder_id.clone(),
        pack.manifest.display_name.clone(),
    )
    .map_err(product_store_error)?;
    if args.sparse {
        import_pack_metadata_only_from_remote(&store, &pack, &remote)
            .map_err(|_| "could not prepare shared folder data on this machine".to_string())?;
    } else {
        import_pack_from_remote_with_progress(&store, &pack, &remote, print_sync_progress)
            .map_err(|_| "could not prepare shared folder data on this machine".to_string())?;
    }
    let revision = store
        .revision_by_id(&remote_revision_id)
        .map_err(product_store_error)?
        .ok_or_else(|| "shared folder data was incomplete".to_string())?;
    if !args.sparse {
        let current = capture_worktree(&store, RevisionBoundary::Restore)?;
        ensure_no_blocked_or_deferred(&current, "clone")?;
        if !current.file_versions().is_empty() {
            return Err(
                "clone refused because the target already contains source files".to_string(),
            );
        }
        RestoreEngine::new(&store)
            .restore(&revision, &current)
            .map_err(|error| error.to_string())?;
        let restored = capture_worktree(&store, RevisionBoundary::Sync)?;
        store
            .coalesce_folder_revision(RevisionBoundary::Sync, restored.file_versions())
            .map_err(product_store_error)?;
    }
    store
        .upsert_remote(
            RemoteConfig::new(
                DEFAULT_REMOTE_NAME,
                BINDHUB_HOSTED_REMOTE_KIND,
                remote_config.clone_url(),
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(product_store_error)?;
    upsert_managed_folder(&mut config, &store, false)?;
    write_config(&config)?;

    println!("Cloned shared folder: {}", folder.display_name);
    println!("Folder: {}", store.folder_root().display());
    println!("Sync: ready");
    if args.sparse {
        println!("Files: available on demand");
        println!(
            "Next step: run bindhub warm {} or bindhub hydrate {}",
            store.folder_root().display(),
            store.folder_root().display()
        );
    } else {
        println!("Files: downloaded");
    }
    start_background_sync(&store, args.start_background_sync)?;
    Ok(())
}

fn run_pause(args: &[String]) -> Result<(), String> {
    let selector = parse_optional_selector(args, "pause")?;
    let mut config = read_config()?;
    let index = resolve_managed_folder(&config, selector.as_deref())?;
    let folder = config.shared_folders[index].clone();
    stop_background_sync_for_path(Path::new(&folder.local_path))?;
    config.shared_folders[index].paused = true;
    write_config(&config)?;

    println!("Paused sync for {}", folder.name);
    println!("Folder: {}", folder.local_path);
    println!("Files: left untouched");
    Ok(())
}

fn run_resume(args: ResumeArgs) -> Result<(), String> {
    let mut config = read_config()?;
    let session = config.session()?;
    let index = resolve_managed_folder(&config, args.target.as_deref())?;
    let folder = config.shared_folders[index].clone();
    let store = LocalStore::open(&folder.local_path).map_err(product_store_error)?;
    configure_hosted_remote(&store, &session)?;
    sync_once(&store, &session)?;
    config.shared_folders[index].paused = false;
    write_config(&config)?;

    println!("Resumed sync for {}", folder.name);
    println!("Folder: {}", store.folder_root().display());
    println!("Sync: up to date");
    start_background_sync(&store, args.start_background_sync)?;
    Ok(())
}

fn run_unlink(args: &[String]) -> Result<(), String> {
    let selector = parse_optional_selector(args, "unlink")?;
    let mut config = read_config()?;
    let index = resolve_managed_folder(&config, selector.as_deref())?;
    let folder = config.shared_folders.remove(index);
    stop_background_sync_for_path(Path::new(&folder.local_path))?;
    write_config(&config)?;

    println!("Unlinked shared folder: {}", folder.name);
    println!("Folder: {}", folder.local_path);
    println!("Files: left untouched");
    Ok(())
}

fn run_warm(args: SparsePathArgs) -> Result<(), String> {
    let config = read_config()?;
    let session = config.session()?;
    let store = open_store_for_path_or_folder(&args.target)?;
    let scope = scope_for_target(&store, &args.target)?;
    let selection = warm_versions_for_scope(&store, &scope, args.max_bytes, args.manifest_only)
        .map_err(product_capture_error)?;
    let avoided_download_bytes = local_bytes_for_versions(&store, selection.versions());
    let fetched_objects = fetch_missing_objects(&store, selection.versions(), &session)?;
    let report = hydrate_versions(&store, selection.versions()).map_err(product_capture_error)?;

    println!("Folder: {}", store.folder_root().display());
    println!("Warmed: {}", display_scope(&scope));
    println!("Warm limit: {} bytes per file", args.max_bytes);
    if args.manifest_only {
        println!("Filter: manifest and config files only");
    }
    println!(
        "Selected: {} files ({} manifest/config, {} source, {} other small)",
        selection.selected_files(),
        selection.selected_manifest_files(),
        selection.selected_source_files(),
        selection.selected_small_files()
    );
    println!(
        "Skipped: {} large, {} outside filter",
        selection.skipped_large_files(),
        selection.skipped_non_manifest_files()
    );
    println!("Downloaded files: {fetched_objects}");
    println!("Already local: {avoided_download_bytes} bytes");
    println!(
        "Files made available: {} written, {} folders, {} already present",
        report.materialized_files(),
        report.materialized_directories(),
        report.already_materialized_files()
    );
    Ok(())
}

fn run_hydrate(args: HydrateArgs) -> Result<(), String> {
    let config = read_config()?;
    let session = config.session()?;
    let store = open_store_for_path_or_folder(&args.target)?;
    let scope = scope_for_target(&store, &args.target)?;
    let versions = tracked_versions_for_scope(&store, &scope).map_err(product_capture_error)?;
    if versions.is_empty() {
        return Err(format!(
            "hydrate found no shared files under {}",
            display_scope(&scope)
        ));
    }

    let fetched_objects = fetch_missing_objects(&store, &versions, &session)?;
    let report = hydrate_versions(&store, &versions).map_err(product_capture_error)?;

    println!("Folder: {}", store.folder_root().display());
    println!("Hydrated: {}", display_scope(&scope));
    println!("Downloaded files: {fetched_objects}");
    println!(
        "Files made available: {} written, {} folders, {} already present",
        report.materialized_files(),
        report.materialized_directories(),
        report.already_materialized_files()
    );
    Ok(())
}

fn run_keep(args: KeepArgs) -> Result<(), String> {
    let store = open_store_for_path_or_folder(&args.target)?;
    let scope = scope_for_target(&store, &args.target)?;
    let revision = store
        .latest_revision()
        .map_err(product_store_error)?
        .ok_or_else(|| "this folder has not synced yet".to_string())?;
    let versions = tracked_versions_for_scope(&store, &scope).map_err(product_capture_error)?;
    if versions.is_empty() {
        return Err(format!(
            "keep found no shared files under {}",
            display_scope(&scope)
        ));
    }
    store
        .pin_revision(
            revision.id(),
            format!("materialization-pin path={}", path_to_store_string(&scope)),
        )
        .map_err(product_store_error)?;

    println!("Folder: {}", store.folder_root().display());
    println!("Kept for offline: {}", display_scope(&scope));
    println!("Protected files: {}", file_version_count(&versions));
    println!("Keep prevents cleanup; run bindhub hydrate to download any missing files.");
    Ok(())
}

fn run_free_space(args: FreeSpaceArgs) -> Result<(), String> {
    let config = read_config()?;
    let session = config.session()?;
    let store = open_store_for_path_or_folder(&args.target)?;
    let scope = scope_for_target(&store, &args.target)?;
    let versions = tracked_versions_for_scope(&store, &scope).map_err(product_capture_error)?;
    if versions.is_empty() {
        return Err(format!(
            "free-space found no shared files under {}",
            display_scope(&scope)
        ));
    }
    let backed_up_objects = backed_up_objects_for_versions(&store, &versions, &session)?;
    let report = prune_cache_to_limit(&store, &scope, args.max_bytes, &backed_up_objects)
        .map_err(product_capture_error)?;

    println!("Folder: {}", store.folder_root().display());
    println!("Freed space under: {}", display_scope(&scope));
    println!("Local byte target: {}", report.limit_bytes());
    println!("Safety: changed and kept files were left alone");
    println!(
        "Local bytes: {} -> {}",
        report.hydrated_bytes_before(),
        report.hydrated_bytes_after()
    );
    println!(
        "Removed: {} files, {} cached file copies; already cloud-only: {} files",
        report.evicted_files(),
        report.evicted_objects(),
        report.already_remote_files()
    );
    println!(
        "Skipped: {} kept, {} changed locally, {} unsupported",
        report.skipped_pinned_files(),
        report.skipped_dirty_files(),
        report.skipped_unsupported_files()
    );
    Ok(())
}

fn product_status(args: &[String]) -> Result<(), String> {
    let selector = parse_status_selector(args)?;
    let config = read_config()?;
    let Some(session) = config.session_or_none() else {
        println!("Logged in: no");
        println!("Run bindhub login to connect this machine.");
        return Ok(());
    };

    println!("Logged in: yes");
    println!("Account: {}", session.account_id);
    println!("Machine: {} ({})", session.device_name, session.device_id);
    println!("Session: stored locally");
    if config.shared_folders.is_empty() {
        println!("Shared folders: none yet");
        return Ok(());
    }

    println!("Shared folders:");
    if let Some(selector) = selector {
        let index = resolve_managed_folder(&config, Some(&selector))?;
        print_managed_folder_status(&config.shared_folders[index], Some(&session))?;
    } else {
        for folder in &config.shared_folders {
            print_managed_folder_status(folder, Some(&session))?;
        }
    }
    Ok(())
}

fn print_managed_folder_status(
    folder: &ManagedFolder,
    session: Option<&Session>,
) -> Result<(), String> {
    let sync_state = managed_folder_sync_state(folder);
    println!("- {}: {} ({})", folder.name, folder.local_path, sync_state);
    if sync_state == "needs attention" || sync_state == "sync status unknown" {
        println!("  Run bindhub doctor for details.");
    }

    let path = Path::new(&folder.local_path);
    if !path.join(".loom").is_dir() {
        return Ok(());
    }

    let store = match LocalStore::open(path) {
        Ok(store) => store,
        Err(error) => {
            println!(
                "  Folder details: unavailable ({})",
                product_safe_reason_fragment(&product_store_error(error))
            );
            return Ok(());
        }
    };
    let versions = match tracked_versions_for_scope(&store, Path::new("")) {
        Ok(versions) => versions,
        Err(error) => {
            println!(
                "  Folder details: unavailable ({})",
                product_safe_reason_fragment(&product_capture_error(error))
            );
            return Ok(());
        }
    };
    let upload = session
        .map(|session| upload_status_for_versions(&store, &versions, session))
        .unwrap_or_else(|| UploadStatus::Unknown {
            reason: "not logged in".to_string(),
        });
    let backed_up_objects = match &upload {
        UploadStatus::Known {
            backed_up_objects, ..
        } => backed_up_objects.clone(),
        UploadStatus::Unknown { .. } => BTreeSet::new(),
    };
    let cache = match cache_status_for_scope(&store, Path::new(""), &backed_up_objects) {
        Ok(cache) => cache,
        Err(error) => {
            println!(
                "  Cache details: unavailable ({})",
                product_safe_reason_fragment(&product_capture_error(error))
            );
            return Ok(());
        }
    };

    println!(
        "  Hydrated: {} files, {} bytes",
        cache.hydrated_files(),
        cache.hydrated_bytes()
    );
    println!(
        "  Cloud-only: {} files, {} bytes",
        cache.remote_only_files(),
        cache.remote_only_bytes()
    );
    println!("  Partial: {} files", cache.partial_files());
    println!("  Changed locally: {} files", cache.dirty_files());
    println!(
        "  Kept offline: {} files, {} bytes",
        cache.pinned_files(),
        cache.pinned_bytes()
    );
    println!(
        "  Can free space: {} files, {} bytes",
        cache.evictable_files(),
        cache.evictable_bytes()
    );
    match upload {
        UploadStatus::Known {
            pending_files,
            pending_bytes,
            ..
        } => println!(
            "  Pending upload: {} files, {} bytes",
            pending_files, pending_bytes
        ),
        UploadStatus::Unknown { reason } => println!("  Pending upload: unknown ({reason})"),
    }
    Ok(())
}

fn fetch_missing_objects(
    store: &LocalStore,
    versions: &[loom_core::FileVersion],
    session: &Session,
) -> Result<usize, String> {
    let missing = versions
        .iter()
        .filter(|version| version.kind() == &FileKind::File)
        .filter_map(|version| {
            version
                .object_id()
                .filter(|object_id| !store.object_cache().exists(object_id))
                .map(|object_id| (object_id.clone(), version.size_bytes()))
        })
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(0);
    }

    let remote = hosted_remote_for_store(store, session)?;
    let mut fetched_objects = 0;
    for (object_id, size_bytes) in missing {
        if hydrate_object_from_remote(store, &remote, &object_id, size_bytes)
            .map_err(product_sync_error)?
        {
            fetched_objects += 1;
        }
    }
    Ok(fetched_objects)
}

fn backed_up_objects_for_versions(
    store: &LocalStore,
    versions: &[loom_core::FileVersion],
    session: &Session,
) -> Result<BTreeSet<ObjectId>, String> {
    let remote = hosted_remote_for_store(store, session)?;
    let mut backed_up = BTreeSet::new();
    for object_id in unique_file_object_ids(versions) {
        match remote.has_object(&object_id) {
            Ok(true) => {
                backed_up.insert(object_id);
            }
            Ok(false) => {
                return Err("free-space refused because some files are not safely backed up yet; run bindhub resume for this folder and try again".to_string());
            }
            Err(error) => {
                return Err(format!(
                    "free-space refused because some files are not safely backed up yet; run bindhub resume for this folder and try again ({})",
                    product_safe_reason_fragment(&product_sync_error(error))
                ));
            }
        }
    }
    Ok(backed_up)
}

fn upload_status_for_versions(
    store: &LocalStore,
    versions: &[loom_core::FileVersion],
    session: &Session,
) -> UploadStatus {
    let remote = match hosted_remote_for_store(store, session) {
        Ok(remote) => remote,
        Err(error) => return UploadStatus::Unknown { reason: error },
    };
    let mut backed_up_objects = BTreeSet::new();
    let mut checked_objects = BTreeSet::new();
    for object_id in unique_file_object_ids(versions) {
        match remote.has_object(&object_id) {
            Ok(true) => {
                backed_up_objects.insert(object_id.clone());
                checked_objects.insert(object_id);
            }
            Ok(false) => {
                checked_objects.insert(object_id);
            }
            Err(error) => {
                return UploadStatus::Unknown {
                    reason: product_sync_error(error),
                }
            }
        }
    }

    let mut pending_files = 0;
    let mut pending_bytes = 0;
    for version in versions {
        if version.kind() != &FileKind::File {
            continue;
        }
        let Some(object_id) = version.object_id() else {
            continue;
        };
        if checked_objects.contains(object_id) && !backed_up_objects.contains(object_id) {
            pending_files += 1;
            pending_bytes += version.size_bytes().unwrap_or(0);
        }
    }

    UploadStatus::Known {
        backed_up_objects,
        pending_files,
        pending_bytes,
    }
}

fn hosted_remote_for_store(
    store: &LocalStore,
    session: &Session,
) -> Result<BindhubHostedRemote, String> {
    let config = BindhubHostedRemoteConfig::new(
        &session.api_url,
        store.shared_folder().id().clone(),
        &session.session_token,
        &session.device_id,
    )
    .map_err(product_sync_error)?;
    Ok(BindhubHostedRemote::new(config))
}

fn unique_file_object_ids(versions: &[loom_core::FileVersion]) -> BTreeSet<ObjectId> {
    versions
        .iter()
        .filter(|version| version.kind() == &FileKind::File)
        .filter_map(|version| version.object_id().cloned())
        .collect()
}

fn local_bytes_for_versions(store: &LocalStore, versions: &[loom_core::FileVersion]) -> u64 {
    versions
        .iter()
        .filter_map(|version| {
            let object_id = version.object_id()?;
            store
                .object_cache()
                .exists(object_id)
                .then(|| version.size_bytes().unwrap_or(0))
        })
        .sum()
}

fn file_version_count(versions: &[loom_core::FileVersion]) -> usize {
    versions
        .iter()
        .filter(|version| version.kind() == &FileKind::File)
        .count()
}

fn scope_for_target(store: &LocalStore, target: &Path) -> Result<PathBuf, String> {
    relative_scope_path(store, &absolute_path_from_path(target)?).map_err(product_capture_error)
}

fn display_scope(scope: &Path) -> String {
    path_to_store_string(scope)
}

enum UploadStatus {
    Known {
        backed_up_objects: BTreeSet<ObjectId>,
        pending_files: usize,
        pending_bytes: u64,
    },
    Unknown {
        reason: String,
    },
}

fn sync_once(store: &LocalStore, session: &Session) -> Result<WorktreeCapture, String> {
    let capture = capture_worktree(store, RevisionBoundary::Sync)?;
    ensure_no_blocked_or_deferred(&capture, "share")?;
    store
        .coalesce_folder_revision(RevisionBoundary::Sync, capture.file_versions())
        .map_err(product_store_error)?;
    let remote_config = BindhubHostedRemoteConfig::new(
        &session.api_url,
        store.shared_folder().id().clone(),
        &session.session_token,
        &session.device_id,
    )
    .map_err(|error| error.to_string())?;
    let remote = BindhubHostedRemote::new(remote_config);
    sync_store_to_remote_with_progress(store, &remote, print_sync_progress)
        .map_err(product_sync_error)?;
    Ok(capture)
}

fn print_sync_progress(progress: SyncProgress) {
    match progress {
        SyncProgress::UploadObject {
            index,
            total,
            size_bytes,
        } => eprintln!(
            "Uploading file data {index}/{total} ({})",
            product_size_label(size_bytes)
        ),
        SyncProgress::DownloadObject {
            index,
            total,
            size_bytes,
        } => eprintln!(
            "Downloading file data {index}/{total} ({})",
            product_size_label(size_bytes)
        ),
    }
}

fn product_size_label(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f >= GIB {
        format!("{:.1} GiB", bytes_f / GIB)
    } else if bytes_f >= MIB {
        format!("{:.1} MiB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.1} KiB", bytes_f / KIB)
    } else {
        format!("{bytes} bytes")
    }
}

fn configure_hosted_remote(store: &LocalStore, session: &Session) -> Result<(), String> {
    let config = BindhubHostedRemoteConfig::new(
        &session.api_url,
        store.shared_folder().id().clone(),
        &session.session_token,
        &session.device_id,
    )
    .map_err(|error| error.to_string())?;
    store
        .upsert_remote(
            RemoteConfig::new(
                DEFAULT_REMOTE_NAME,
                BINDHUB_HOSTED_REMOTE_KIND,
                config.clone_url(),
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(product_store_error)
}

fn capture_worktree(
    store: &LocalStore,
    boundary: RevisionBoundary,
) -> Result<WorktreeCapture, String> {
    CaptureEngine::new(store)
        .capture(&CaptureRequest::new(
            store.shared_folder().clone(),
            boundary,
        ))
        .map_err(|error| error.to_string())
}

fn ensure_no_blocked_or_deferred(capture: &WorktreeCapture, action: &str) -> Result<(), String> {
    if let Some(notice) = capture.blocked().first() {
        return Err(format!(
            "{action} refused because {} {}",
            display_store_path(notice.relative_path()),
            product_blocked_source_reason(notice.reason())
        ));
    }
    if let Some(notice) = capture.deferred().first() {
        return Err(format!(
            "{action} paused because {} {}",
            display_store_path(notice.relative_path()),
            product_deferred_source_reason(notice.reason())
        ));
    }
    Ok(())
}

fn start_background_sync(store: &LocalStore, enabled: bool) -> Result<(), String> {
    if !enabled {
        println!("Live sync: not started");
        return Ok(());
    }
    let report = loom_daemon::start_background(&DaemonStartOptions::new(store.folder_root()))
        .map_err(product_daemon_error)?;
    if report.already_running {
        println!("Live sync: already running");
    } else {
        println!("Live sync: running");
    }
    Ok(())
}

fn stop_background_sync_for_path(path: &Path) -> Result<(), String> {
    if path.join(".loom").is_dir() {
        loom_daemon::request_stop(path).map_err(product_daemon_error)?;
    }
    Ok(())
}

fn managed_folder_sync_state(folder: &ManagedFolder) -> String {
    if folder.paused {
        return "paused".to_string();
    }
    let path = Path::new(&folder.local_path);
    if !path.join(".loom").is_dir() {
        return "not linked here".to_string();
    }
    match loom_daemon::read_status(path) {
        Ok(status) => match status.state.as_str() {
            "running" | "starting" => "sync running".to_string(),
            "blocked" => "needs attention".to_string(),
            "stopped" => "sync stopped".to_string(),
            other => format!("sync {other}"),
        },
        Err(_) => "sync status unknown".to_string(),
    }
}

fn print_folder_diagnostics(folder: &ManagedFolder) {
    let path = Path::new(&folder.local_path);
    if !path.exists() {
        println!("  Folder check: missing on this machine");
        return;
    }
    if !path.join(".loom").is_dir() {
        println!("  Folder check: not linked here");
        return;
    }

    match loom_daemon::read_status(path) {
        Ok(status) => {
            println!("  Live sync: {}", status.state);
            if let Some(error) = status.last_error.as_deref() {
                println!("  Last issue: {}", product_safe_reason_fragment(error));
            }
        }
        Err(error) => {
            println!(
                "  Live sync: status unavailable ({})",
                product_safe_reason_fragment(&error.to_string())
            );
        }
    }
    println!("  Loom diagnostics:");
    println!("    loom doctor {}", path.display());
    println!("    loom cache status {}", path.display());
    println!("    loom remote check {}", path.display());
}

fn ensure_shared_folder(
    session: &Session,
    folder_id: &str,
    display_name: &str,
) -> Result<(), String> {
    let body = serde_json::json!({ "display_name": display_name });
    let _: SharedFolderResponse = send_json(
        auth_request(
            ureq::put(&api_url(
                &session.api_url,
                &format!("/v1/shared-folders/{}", api_path_segment(folder_id)?),
            )?),
            session,
        ),
        body,
    )?;
    Ok(())
}

fn list_shared_folders(session: &Session) -> Result<Vec<SharedFolderResponse>, String> {
    call_json(auth_request(
        ureq::get(&api_url(&session.api_url, "/v1/shared-folders")?),
        session,
    ))
}

fn print_cloneable_folders(config: &ProductConfig, folders: &[SharedFolderResponse]) {
    let cloneable = folders
        .iter()
        .filter(|folder| {
            !config.shared_folders.iter().any(|managed| {
                managed.folder_id == folder.id && Path::new(&managed.local_path).exists()
            })
        })
        .collect::<Vec<_>>();

    if cloneable.is_empty() {
        if folders.is_empty() {
            println!("No shared folders are available for this account.");
        } else {
            println!("All shared folders for this account are already on this machine.");
        }
        return;
    }

    println!("Shared folders available to clone:");
    for folder in cloneable {
        println!("- {} ({})", folder.display_name, folder.role);
    }
}

fn find_shared_folder<'a>(
    folders: &'a [SharedFolderResponse],
    selector: &str,
) -> Result<&'a SharedFolderResponse, String> {
    let matches = folders
        .iter()
        .filter(|folder| folder.display_name == selector || folder.id == selector)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [folder] => Ok(folder),
        [] => Err(format!(
            "shared folder '{selector}' was not found for this account"
        )),
        _ => Err(format!(
            "shared folder name '{selector}' matches more than one folder; use its id"
        )),
    }
}

fn upsert_managed_folder(
    config: &mut ProductConfig,
    store: &LocalStore,
    paused: bool,
) -> Result<(), String> {
    let folder = ManagedFolder {
        name: store.shared_folder().display_name().to_string(),
        folder_id: store.shared_folder().id().as_str().to_string(),
        local_path: store.folder_root().display().to_string(),
        paused,
    };
    if let Some(existing) = config
        .shared_folders
        .iter_mut()
        .find(|existing| existing.local_path == folder.local_path)
    {
        *existing = folder;
    } else {
        config.shared_folders.push(folder);
    }
    config.shared_folders.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.local_path.cmp(&right.local_path))
    });
    Ok(())
}

fn resolve_managed_folder(config: &ProductConfig, selector: Option<&str>) -> Result<usize, String> {
    if let Some(selector) = selector {
        let selector_path = Path::new(selector);
        let canonical_selector = selector_path
            .exists()
            .then(|| fs::canonicalize(selector_path).ok())
            .flatten();
        let matches = config
            .shared_folders
            .iter()
            .enumerate()
            .filter(|(_, folder)| {
                folder.name == selector
                    || folder.folder_id == selector
                    || folder.local_path == selector
                    || canonical_selector
                        .as_ref()
                        .is_some_and(|path| Path::new(&folder.local_path) == path)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        return match matches.as_slice() {
            [index] => Ok(*index),
            [] => Err(format!(
                "shared folder '{selector}' is not linked on this machine"
            )),
            _ => Err(format!(
                "shared folder '{selector}' matches more than one local folder"
            )),
        };
    }

    if let Ok(store) =
        LocalStore::discover_from(std::env::current_dir().map_err(|error| error.to_string())?)
    {
        if let Some((index, _)) = config
            .shared_folders
            .iter()
            .enumerate()
            .find(|(_, folder)| folder.folder_id == store.shared_folder().id().as_str())
        {
            return Ok(index);
        }
    }

    match config.shared_folders.as_slice() {
        [_] => Ok(0),
        [] => Err("no shared folders are linked on this machine".to_string()),
        _ => Err("choose a shared folder by name, id, or path".to_string()),
    }
}

fn open_store_for_path_or_folder(path: &Path) -> Result<LocalStore, String> {
    let absolute = absolute_path_from_path(path)?;
    let mut current = if absolute.exists() && absolute.is_dir() {
        absolute.clone()
    } else {
        absolute
            .parent()
            .ok_or_else(|| format!("path has no parent: {}", absolute.display()))?
            .to_path_buf()
    };

    while !current.exists() {
        if !current.pop() {
            return Err(format!(
                "could not find an existing ancestor for {}",
                absolute.display()
            ));
        }
    }

    let mut current = fs::canonicalize(&current).map_err(|error| error.to_string())?;
    loop {
        if current
            .join(".loom")
            .join("metadata")
            .join("shared_folder.tsv")
            .is_file()
        {
            return LocalStore::open(&current).map_err(product_store_error);
        }
        if !current.pop() {
            return Err(format!(
                "{} is not inside a bindhub shared folder",
                absolute.display()
            ));
        }
    }
}

fn validate_clone_target_before_mutation(target: &Path) -> Result<(), String> {
    if !target.exists() {
        return Ok(());
    }
    if !target.is_dir() {
        return Err(format!(
            "clone target is not a folder: {}",
            target.display()
        ));
    }
    if target.join(".loom").exists() {
        return Err("clone refused because the target is already linked".to_string());
    }
    if let Some(source_path) = first_clone_source_entry(target, target)? {
        return Err(format!(
            "clone refused because the target already contains source files: {}",
            display_store_path(&source_path)
        ));
    }
    Ok(())
}

fn absolute_path_from_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(std::env::current_dir()
        .map_err(|error| error.to_string())?
        .join(path))
}

fn first_clone_source_entry(root: &Path, path: &Path) -> Result<Option<PathBuf>, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("could not inspect clone target {}: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not inspect clone target {}: {error}", path.display()))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let entry_path = entry.path();
        let relative_path = entry_path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_path_buf();
        let metadata = fs::symlink_metadata(&entry_path).map_err(|error| {
            format!(
                "could not inspect clone target entry {}: {error}",
                entry_path.display()
            )
        })?;

        if metadata.is_dir() {
            match evaluate_directory_policy(&relative_path) {
                DirectoryPolicyDecision::Ignore { .. } => continue,
                DirectoryPolicyDecision::Include => return Ok(Some(relative_path)),
            }
        }

        return Ok(Some(relative_path));
    }

    Ok(None)
}

fn read_config() -> Result<ProductConfig, String> {
    let path = config_path()?;
    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|error| format!("could not read Bindhub config {}: {error}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ProductConfig::default()),
        Err(error) => Err(format!(
            "could not read Bindhub config {}: {error}",
            path.display()
        )),
    }
}

fn write_config(config: &ProductConfig) -> Result<(), String> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?;
    fs::write(&path, bytes).map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join(CONFIG_FILE))
}

fn config_dir() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var(CONFIG_DIR_ENV) {
        return Ok(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("APPDATA") {
        return Ok(PathBuf::from(path).join("Bindhub"));
    }
    if let Ok(path) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        return Ok(PathBuf::from(path).join(".bindhub"));
    }
    Ok(std::env::current_dir()
        .map_err(|error| error.to_string())?
        .join(".bindhub"))
}

impl ProductConfig {
    fn session(&self) -> Result<Session, String> {
        self.session_or_none()
            .ok_or_else(|| "not logged in; run bindhub login first".to_string())
    }

    fn session_or_none(&self) -> Option<Session> {
        Some(Session {
            api_url: self.api_url.clone()?,
            account_id: self.account_id.clone()?,
            session_token: self.session_token.clone()?,
            device_id: self.device_id.clone()?,
            device_name: self
                .device_name
                .clone()
                .unwrap_or_else(|| "This machine".to_string()),
        })
    }
}

fn parse_login_args(args: &[String]) -> Result<LoginArgs, String> {
    let mut api_url = Some(default_api_url());
    let mut web_url = default_web_url();
    let mut account_hint = "local-dev".to_string();
    let mut account_hint_explicit = false;
    let mut device_name = default_device_name();
    let mut open_browser = true;
    let mut local_dev_direct = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--api" => {
                index += 1;
                api_url = Some(required_value(args, index, "--api")?);
            }
            "--web" => {
                index += 1;
                web_url = required_value(args, index, "--web")?;
            }
            "--account" => {
                index += 1;
                account_hint = required_value(args, index, "--account")?;
                account_hint_explicit = true;
            }
            "--device-name" => {
                index += 1;
                device_name = required_value(args, index, "--device-name")?;
            }
            "--no-browser" => open_browser = false,
            "--local-dev-direct" => local_dev_direct = true,
            value => return Err(format!("login unknown option '{value}'")),
        }
        index += 1;
    }
    let api_url = api_url.expect("default API URL is always present");
    validate_api_url(&api_url)?;
    validate_web_url(&web_url)?;
    if local_dev_direct && !account_hint_explicit {
        account_hint = std::env::var("BINDHUB_ACCOUNT").unwrap_or(account_hint);
    }
    if !local_dev_direct && account_hint_explicit {
        return Err(
            "login derives account identity from browser authentication; use --local-dev-direct for local-dev account hints".to_string(),
        );
    }
    Ok(LoginArgs {
        api_url,
        account_hint,
        device_name,
        web_url,
        open_browser,
        local_dev_direct,
    })
}

fn parse_share_args(args: &[String]) -> Result<ShareArgs, String> {
    let mut folder = None;
    let mut start_background_sync = true;
    for arg in args {
        match arg.as_str() {
            "--no-background-sync" => start_background_sync = false,
            value if value.starts_with('-') => {
                return Err(format!("share unknown option '{value}'"))
            }
            value => {
                if folder.replace(PathBuf::from(value)).is_some() {
                    return Err("share accepts exactly one folder".to_string());
                }
            }
        }
    }
    Ok(ShareArgs {
        folder: folder.ok_or_else(|| "share requires a folder".to_string())?,
        start_background_sync,
    })
}

fn parse_clone_args(args: &[String]) -> Result<CloneArgs, String> {
    let mut positionals = Vec::new();
    let mut start_background_sync = true;
    let mut sparse = false;
    for arg in args {
        match arg.as_str() {
            "--no-background-sync" => start_background_sync = false,
            "--sparse" => sparse = true,
            value if value.starts_with('-') => {
                return Err(format!("clone unknown option '{value}'"))
            }
            value => positionals.push(value.to_string()),
        }
    }
    match positionals.as_slice() {
        [] => Ok(CloneArgs {
            name: None,
            target: None,
            start_background_sync,
            sparse,
        }),
        [name] => Ok(CloneArgs {
            name: Some(name.clone()),
            target: None,
            start_background_sync,
            sparse,
        }),
        [name, target] => Ok(CloneArgs {
            name: Some(name.clone()),
            target: Some(PathBuf::from(target)),
            start_background_sync,
            sparse,
        }),
        _ => Err("clone accepts at most a shared folder name and target folder".to_string()),
    }
}

fn parse_sparse_path_args(command: &str, args: &[String]) -> Result<SparsePathArgs, String> {
    let mut target = None;
    let mut max_bytes = DEFAULT_WARM_MAX_BYTES;
    let mut manifest_only = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--max-bytes" => {
                index += 1;
                max_bytes = parse_u64(&required_value(args, index, "--max-bytes")?, "--max-bytes")?;
            }
            value if value.starts_with("--max-bytes=") => {
                let value = value
                    .split_once('=')
                    .expect("--max-bytes= has a delimiter")
                    .1;
                max_bytes = parse_u64(value, "--max-bytes")?;
            }
            "--manifest" => manifest_only = true,
            value if value.starts_with('-') => {
                return Err(format!("{command} unknown option '{value}'"));
            }
            value => {
                if target.replace(PathBuf::from(value)).is_some() {
                    return Err(format!("{command} accepts one path or folder"));
                }
            }
        }
        index += 1;
    }

    Ok(SparsePathArgs {
        target: target.ok_or_else(|| {
            format!("{command} requires a path or folder\nUsage: bindhub {command} <PATH|FOLDER>")
        })?,
        max_bytes,
        manifest_only,
    })
}

fn parse_hydrate_args(args: &[String]) -> Result<HydrateArgs, String> {
    match args {
        [target] => Ok(HydrateArgs {
            target: PathBuf::from(target),
        }),
        [] => Err(
            "hydrate requires a path or folder\nUsage: bindhub hydrate <PATH|FOLDER>".to_string(),
        ),
        _ => Err("hydrate accepts exactly one path or folder".to_string()),
    }
}

fn parse_keep_args(args: &[String]) -> Result<KeepArgs, String> {
    match args {
        [target] => Ok(KeepArgs {
            target: PathBuf::from(target),
        }),
        [] => Err("keep requires a path or folder\nUsage: bindhub keep <PATH|FOLDER>".to_string()),
        _ => Err("keep accepts exactly one path or folder".to_string()),
    }
}

fn parse_free_space_args(args: &[String]) -> Result<FreeSpaceArgs, String> {
    let mut target = None;
    let mut max_bytes = 0;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--max-bytes" => {
                index += 1;
                max_bytes = parse_u64(&required_value(args, index, "--max-bytes")?, "--max-bytes")?;
            }
            value if value.starts_with("--max-bytes=") => {
                let value = value
                    .split_once('=')
                    .expect("--max-bytes= has a delimiter")
                    .1;
                max_bytes = parse_u64(value, "--max-bytes")?;
            }
            value if value.starts_with('-') => {
                return Err(format!("free-space unknown option '{value}'"));
            }
            value => {
                if target.replace(PathBuf::from(value)).is_some() {
                    return Err("free-space accepts one path or folder".to_string());
                }
            }
        }
        index += 1;
    }

    Ok(FreeSpaceArgs {
        target: target.ok_or_else(|| {
            "free-space requires a path or folder\nUsage: bindhub free-space <PATH|FOLDER> [--max-bytes <BYTES>]".to_string()
        })?,
        max_bytes,
    })
}

fn parse_update_args(args: &[String]) -> Result<UpdateArgs, String> {
    let mut version = "latest".to_string();
    let mut repo = std::env::var("BINDHUB_UPDATE_REPO")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_UPDATE_REPO.to_string());
    let mut yes = false;
    let mut positional_version = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--yes" | "-y" => yes = true,
            "--version" => {
                index += 1;
                version = required_value(args, index, "--version")?;
            }
            "--repo" => {
                index += 1;
                repo = required_value(args, index, "--repo")?;
            }
            value if value.starts_with("--version=") => {
                version = value
                    .split_once('=')
                    .expect("--version= has a delimiter")
                    .1
                    .to_string();
            }
            value if value.starts_with("--repo=") => {
                repo = value
                    .split_once('=')
                    .expect("--repo= has a delimiter")
                    .1
                    .to_string();
            }
            value if value.starts_with('-') => {
                return Err(format!("update unknown option '{value}'"));
            }
            value => {
                if positional_version.replace(value.to_string()).is_some() {
                    return Err("update accepts at most one version tag".to_string());
                }
            }
        }
        index += 1;
    }
    if let Some(value) = positional_version {
        version = value;
    }
    validate_update_repo(&repo)?;
    validate_update_version(&version)?;
    Ok(UpdateArgs { version, repo, yes })
}

fn parse_resume_args(args: &[String]) -> Result<ResumeArgs, String> {
    let mut target = None;
    let mut start_background_sync = true;
    for arg in args {
        match arg.as_str() {
            "--no-background-sync" => start_background_sync = false,
            value if value.starts_with('-') => {
                return Err(format!("resume unknown option '{value}'"))
            }
            value => {
                if target.replace(value.to_string()).is_some() {
                    return Err("resume accepts at most one shared folder".to_string());
                }
            }
        }
    }
    Ok(ResumeArgs {
        target,
        start_background_sync,
    })
}

fn parse_status_selector(args: &[String]) -> Result<Option<String>, String> {
    match args {
        [] => Ok(None),
        [selector] => Ok(Some(selector.clone())),
        _ => Err("status accepts at most one shared folder".to_string()),
    }
}

fn parse_optional_selector(args: &[String], command: &str) -> Result<Option<String>, String> {
    match args {
        [] => Ok(None),
        [selector] => Ok(Some(selector.clone())),
        _ => Err(format!("{command} accepts at most one shared folder")),
    }
}

fn parse_daemon_entry_args(args: &[String]) -> Result<DaemonEntryArgs, String> {
    let mut folder = None;
    let mut debounce_ms = loom_daemon::DEFAULT_DEBOUNCE_MS;
    let mut poll_ms = loom_daemon::DEFAULT_POLL_MS;
    let mut max_cycles = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--debounce-ms" => {
                index += 1;
                debounce_ms = parse_u64(
                    &required_value(args, index, "--debounce-ms")?,
                    "--debounce-ms",
                )?;
            }
            "--poll-ms" => {
                index += 1;
                poll_ms = parse_u64(&required_value(args, index, "--poll-ms")?, "--poll-ms")?;
            }
            "--max-cycles" => {
                index += 1;
                max_cycles = Some(parse_usize(
                    &required_value(args, index, "--max-cycles")?,
                    "--max-cycles",
                )?);
            }
            value if value.starts_with('-') => {
                return Err(format!("background sync unknown option '{value}'"))
            }
            value => {
                if folder.replace(PathBuf::from(value)).is_some() {
                    return Err("background sync accepts one folder".to_string());
                }
            }
        }
        index += 1;
    }
    Ok(DaemonEntryArgs {
        folder: folder.ok_or_else(|| "background sync requires a folder".to_string())?,
        debounce_ms,
        poll_ms,
        max_cycles,
    })
}

fn required_value(args: &[String], index: usize, flag: &str) -> Result<String, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn parse_usize(value: &str, flag: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn auth_request(request: ureq::Request, session: &Session) -> ureq::Request {
    request
        .set(
            "authorization",
            &format!("Bearer {}", session.session_token),
        )
        .set("x-bindhub-device-id", &session.device_id)
}

fn send_json<T: DeserializeOwned>(
    request: ureq::Request,
    body: serde_json::Value,
) -> Result<T, String> {
    request
        .set("content-type", "application/json")
        .send_json(body)
        .map_err(product_api_error)?
        .into_json()
        .map_err(|error| error.to_string())
}

fn call_json<T: DeserializeOwned>(request: ureq::Request) -> Result<T, String> {
    request
        .call()
        .map_err(product_api_error)?
        .into_json()
        .map_err(|error| error.to_string())
}

fn product_api_error(error: UreqError) -> String {
    match error {
        UreqError::Status(status, _) if status == 401 => {
            "Bindhub session was rejected; run bindhub login again".to_string()
        }
        UreqError::Status(status, _) if status == 403 => {
            "this account or machine cannot access that shared folder".to_string()
        }
        UreqError::Status(status, _) => format!("Bindhub API request failed with HTTP {status}"),
        UreqError::Transport(_) => {
            "could not reach Bindhub; check your connection and try again".to_string()
        }
    }
}

fn product_sync_error(error: SyncError) -> String {
    match error {
        SyncError::RemoteAuth(_) => {
            "Bindhub session was rejected; run bindhub login again".to_string()
        }
        SyncError::RemoteTransport(_) => {
            "Bindhub could not reach the shared-folder service; check your connection and try again"
                .to_string()
        }
        SyncError::CursorConflict { .. } | SyncError::DivergentState { .. } => {
            "Sync paused because this folder changed in more than one place. Run bindhub status before resuming.".to_string()
        }
        SyncError::MissingRemotePack(_)
        | SyncError::MissingRevision(_)
        | SyncError::MissingObjectPayload(_)
        | SyncError::MissingObjectSize(_)
        | SyncError::Pack(_) => {
            "Bindhub could not read the latest shared folder data; try again".to_string()
        }
        SyncError::NoLocalRevision => {
            "This folder has not been captured yet; run bindhub share again".to_string()
        }
        SyncError::Store(error) => product_store_error(error),
        SyncError::Io { .. } | SyncError::Loom(_) => {
            "Bindhub could not update this local folder; check the folder and try again".to_string()
        }
        SyncError::InvalidCursor(_) | SyncError::CursorLockBusy { .. } => {
            "Sync is already busy for this folder; try again in a moment".to_string()
        }
        SyncError::RemoteConfig(_) => {
            "bindhub sync settings for this folder are invalid; run bindhub login and resume again"
                .to_string()
        }
    }
}

fn product_store_error(error: StoreError) -> String {
    match error {
        StoreError::MissingStore { .. } => {
            "This folder is not linked with Bindhub yet; run bindhub share or bindhub clone"
                .to_string()
        }
        StoreError::MissingObject { .. }
        | StoreError::ObjectHashMismatch { .. }
        | StoreError::MissingRevisionTarget { .. }
        | StoreError::AmbiguousRevisionTarget { .. } => {
            "Bindhub could not read the latest shared folder data; try again".to_string()
        }
        StoreError::CorruptMetadata { .. } => {
            "Bindhub could not read sync settings for this folder; run bindhub resume again"
                .to_string()
        }
        StoreError::Io { .. } | StoreError::Loom(_) => {
            "Bindhub could not update this local folder; check the folder and try again".to_string()
        }
    }
}

fn product_capture_error(error: loom_worktree::CaptureError) -> String {
    product_safe_reason_fragment(&error.to_string())
}

fn product_daemon_error(error: loom_daemon::DaemonError) -> String {
    match error {
        loom_daemon::DaemonError::Sync(error) => product_sync_error(error),
        loom_daemon::DaemonError::DivergentState { .. } => {
            "Sync paused because this folder changed in more than one place. Run bindhub status before resuming.".to_string()
        }
        loom_daemon::DaemonError::BlockedSource { path, reason } => format!(
            "Sync paused because {} {}",
            display_store_path(&path),
            product_blocked_source_reason(&reason)
        ),
        loom_daemon::DaemonError::DeferredSource { path, reason } => format!(
            "Sync paused because {} {}",
            display_store_path(&path),
            product_deferred_source_reason(&reason)
        ),
        loom_daemon::DaemonError::AlreadyRunning { .. } => {
            "Sync is already running for this folder".to_string()
        }
        loom_daemon::DaemonError::NoRemote | loom_daemon::DaemonError::UnsupportedRemote { .. } => {
            "bindhub sync settings for this folder are invalid; run bindhub resume again".to_string()
        }
        loom_daemon::DaemonError::NoLocalRevision => {
            "This folder has not been captured yet; run bindhub share again".to_string()
        }
        loom_daemon::DaemonError::Store(error) => product_store_error(error),
        loom_daemon::DaemonError::Capture(_)
        | loom_daemon::DaemonError::Io { .. }
        | loom_daemon::DaemonError::Notify(_)
        | loom_daemon::DaemonError::Spawn { .. }
        | loom_daemon::DaemonError::InvalidStatus { .. } => {
            "Bindhub could not update sync for this folder; try again".to_string()
        }
    }
}

fn product_blocked_source_reason(reason: &str) -> String {
    let line = value_between(reason, " at line ", ";");
    let evidence = value_after(reason, "evidence: ");
    let mut message = "contains a blocked secret pattern".to_string();
    if let Some(line) = line {
        message.push_str(&format!(" at line {line}"));
    }
    if let Some(evidence) = evidence {
        message.push_str(&format!(
            "; evidence: {}",
            product_safe_reason_fragment(evidence)
        ));
    }
    message.push_str(". Remove it or exclude the file, then try again.");
    message
}

fn product_deferred_source_reason(reason: &str) -> String {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("symlink") {
        return "is a symlink, which Bindhub does not share yet. Replace it with a regular file or exclude it, then try again.".to_string();
    }
    if lower.contains("unsupported") {
        return "has an unsupported file type. Remove it or exclude it, then try again."
            .to_string();
    }
    format!(
        "needs attention before it can be shared: {}. Fix it or exclude it, then try again.",
        product_safe_reason_fragment(reason)
    )
}

fn product_safe_reason_fragment(value: &str) -> String {
    let sanitized = value
        .replace("Loom", "Bindhub")
        .replace("loom", "Bindhub")
        .replace("cursor", "sync state")
        .replace("pack", "shared folder data")
        .replace("remote", "shared-folder service")
        .replace("bindhub://", "Bindhub link");
    sanitized.trim().trim_end_matches('.').to_string()
}

fn value_between<'a>(value: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    value
        .split_once(prefix)
        .and_then(|(_, rest)| rest.split_once(suffix))
        .map(|(matched, _)| matched.trim())
        .filter(|matched| !matched.is_empty())
}

fn value_after<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    value
        .split_once(prefix)
        .map(|(_, matched)| matched.trim())
        .filter(|matched| !matched.is_empty())
}

fn api_url(api: &str, path: &str) -> Result<String, String> {
    validate_api_url(api)?;
    Ok(format!("{}{}", api.trim_end_matches('/'), path))
}

fn validate_api_url(api: &str) -> Result<(), String> {
    MetadataServiceConfig {
        endpoint: api.to_string(),
        auth_mode: MetadataAuthMode::AccountSession,
    }
    .validate()
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn validate_web_url(web: &str) -> Result<(), String> {
    let trimmed = web.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Ok(())
    } else {
        Err("--web requires an http(s) URL".to_string())
    }
}

fn ensure_local_dev_direct_api_allowed(api: &str) -> Result<(), String> {
    if is_loopback_api_url(api) || env_flag_enabled("BINDHUB_ALLOW_NON_LOOPBACK_LOCAL_DEV_LOGIN") {
        return Ok(());
    }

    Err(
        "bindhub login --local-dev-direct is local-dev only; use a loopback --api URL or set BINDHUB_ALLOW_NON_LOOPBACK_LOCAL_DEV_LOGIN=1 for local development".to_string(),
    )
}

fn is_loopback_api_url(api: &str) -> bool {
    let Some((_, rest)) = api.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority)
        .trim();
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or(bracketed)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    }
    .to_ascii_lowercase();

    host.parse::<IpAddr>()
        .map(|address| address.is_loopback())
        .unwrap_or_else(|_| host == "localhost" || host == "localhost.")
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn api_path_segment(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.'))
    {
        return Err("shared folder id must be safe".to_string());
    }
    Ok(trimmed.to_string())
}

fn default_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "This machine".to_string())
}

fn default_api_url() -> String {
    std::env::var(API_URL_ENV)
        .ok()
        .or_else(|| option_env!("BINDHUB_DEFAULT_API_URL").map(ToString::to_string))
        .unwrap_or_else(|| LOCAL_DEV_API_URL.to_string())
}

fn default_web_url() -> String {
    std::env::var(WEB_URL_ENV)
        .ok()
        .or_else(|| option_env!("BINDHUB_DEFAULT_WEB_URL").map(ToString::to_string))
        .unwrap_or_else(|| LOCAL_DEV_WEB_URL.to_string())
}

fn update_installer_url(repo: &str, installer: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/{repo}/{UPDATE_INSTALLER_BRANCH}/scripts/{installer}"
    )
}

fn update_install_command(args: &UpdateArgs) -> String {
    #[cfg(target_os = "windows")]
    {
        format!(
            "irm {} -OutFile install-bindhub.ps1; .\\install-bindhub.ps1 -Version {} -Repo {}",
            update_installer_url(&args.repo, "install-bindhub.ps1"),
            args.version,
            args.repo
        )
    }

    #[cfg(not(target_os = "windows"))]
    {
        format!(
            "BINDHUB_REPO={} curl -fsSL {} | sh -s -- {}",
            sh_single_quote(&args.repo),
            sh_single_quote(&update_installer_url(&args.repo, "install-bindhub.sh")),
            sh_single_quote(&args.version)
        )
    }
}

fn validate_update_repo(repo: &str) -> Result<(), String> {
    let parts = repo.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(is_safe_update_token_char))
    {
        return Err("--repo must look like OWNER/REPO".to_string());
    }
    Ok(())
}

fn validate_update_version(version: &str) -> Result<(), String> {
    if version.is_empty() || !version.chars().all(is_safe_update_token_char) {
        return Err("--version must be a release tag such as latest or v0.1.0-alpha.1".to_string());
    }
    Ok(())
}

fn is_safe_update_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

#[cfg(target_os = "windows")]
fn powershell_single_quote(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(not(target_os = "windows"))]
fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn cli_verification_url(web_url: &str, user_code: &str) -> Option<String> {
    let user_code = api_path_segment(user_code).ok()?;
    Some(format!(
        "{}/auth/cli?code={}",
        web_url.trim_end_matches('/'),
        user_code
    ))
}

fn open_browser_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn stable_device_id(device_name: &str) -> String {
    let basis = format!(
        "{}:{}",
        device_name,
        config_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .display()
    );
    format!(
        "device-b3-{}",
        &blake3::hash(basis.as_bytes()).to_hex()[..16]
    )
}

fn safe_folder_name(value: &str) -> String {
    let name = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if name.is_empty() {
        "shared-folder".to_string()
    } else {
        name
    }
}

fn display_store_path(path: &Path) -> String {
    let parts = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            std::path::Component::CurDir => Some(".".to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn result_to_exit(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bindhub: {error}");
            ExitCode::from(1)
        }
    }
}
