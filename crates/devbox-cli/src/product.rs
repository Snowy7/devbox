use devbox_metadata::{MetadataAuthMode, MetadataServiceConfig};
use loom_core::{RevisionBoundary, SharedFolderId};
use loom_daemon::{DaemonLoopOptions, DaemonStartOptions};
use loom_store::{LocalStore, RemoteConfig};
use loom_sync::{
    import_pack, sync_store_to_remote, DevboxHostedRemote, DevboxHostedRemoteConfig, LoomRemote,
    DEFAULT_CURSOR_ID, DEFAULT_REMOTE_NAME, DEVBOX_HOSTED_REMOTE_KIND,
};
use loom_worktree::{
    evaluate_directory_policy, CaptureEngine, CaptureRequest, DirectoryPolicyDecision,
    RestoreEngine, WorktreeCapture,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use ureq::Error as UreqError;

const CONFIG_DIR_ENV: &str = "DEVBOX_CONFIG_DIR";
const API_URL_ENV: &str = "DEVBOX_API_URL";
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
}

#[derive(Debug, Clone)]
struct ResumeArgs {
    target: Option<String>,
    start_background_sync: bool,
}

#[derive(Debug, Clone)]
struct DaemonEntryArgs {
    folder: PathBuf,
    debounce_ms: u64,
    poll_ms: u64,
    max_cycles: Option<usize>,
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
        "manage" => {
            println!("devbox manage: shared-folder management will move here after the MVP CLI.");
            println!("For now, use devbox status, pause, resume, or unlink.");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("devbox: unknown product command '{command}'");
            ExitCode::from(2)
        }
    }
}

pub fn run_status() -> ExitCode {
    match product_status() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("devbox: {error}");
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
        loom_daemon::run_loop(&options).map_err(|error| error.to_string())
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("devbox: background sync failed: {error}");
            ExitCode::from(1)
        }
    }
}

pub fn print_product_command_help(command: &str) {
    let usage = match command {
        "login" => "devbox login [--api <URL>] [--account <NAME>] [--device-name <NAME>]",
        "share" => "devbox share <FOLDER> [--no-background-sync]",
        "clone" => {
            "devbox clone\n       devbox clone <SHARED_FOLDER> [TARGET] [--no-background-sync]"
        }
        "manage" => "devbox manage <SHARED_FOLDER>",
        "pause" => "devbox pause [SHARED_FOLDER]",
        "resume" => "devbox resume [SHARED_FOLDER] [--no-background-sync]",
        "unlink" => "devbox unlink [SHARED_FOLDER]",
        _ => "devbox <COMMAND>",
    };

    println!("devbox {command}");
    println!();
    println!("Usage: {usage}");
    println!();
    println!("Devbox keeps folders continuous across machines and uses Loom under the hood.");
}

fn run_login(args: LoginArgs) -> Result<(), String> {
    let mut config = read_config()?;
    let request = serde_json::json!({
        "account_hint": args.account_hint,
        "device_id": stable_device_id(&args.device_name),
        "device_display_name": args.device_name,
    });
    let response: DevSessionResponse = send_json(
        ureq::post(&api_url(&args.api_url, "/v1/auth/dev-session")?),
        request,
    )?;

    config.api_url = Some(args.api_url);
    config.account_id = Some(response.account_id.clone());
    config.session_id = Some(response.session_id.clone());
    config.session_token = Some(response.session_token);
    config.device_id = Some(response.device_id.clone());
    config.device_name = Some(args.device_name);
    write_config(&config)?;

    println!("Logged in to Devbox");
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

fn run_share(args: ShareArgs) -> Result<(), String> {
    let mut config = read_config()?;
    let session = config.session()?;
    let opened = LocalStore::open_or_init(&args.folder).map_err(|error| error.to_string())?;
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
    let remote_config = DevboxHostedRemoteConfig::new(
        &session.api_url,
        folder_id,
        &session.session_token,
        &session.device_id,
    )
    .map_err(|error| error.to_string())?;
    let remote = DevboxHostedRemote::new(remote_config.clone());
    let remote_revision_id = remote
        .get_cursor(DEFAULT_CURSOR_ID)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "that shared folder has not synced yet".to_string())?;
    let pack = remote
        .get_pack(&remote_revision_id)
        .map_err(|error| error.to_string())?;

    validate_clone_target_before_mutation(&target)?;
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    let store = LocalStore::init_clone(
        &target,
        pack.manifest.shared_folder_id.clone(),
        pack.manifest.display_name.clone(),
    )
    .map_err(|error| error.to_string())?;
    import_pack(&store, &pack).map_err(|error| error.to_string())?;
    let current = capture_worktree(&store, RevisionBoundary::Restore)?;
    ensure_no_blocked_or_deferred(&current, "clone")?;
    if !current.file_versions().is_empty() {
        return Err("clone refused because the target already contains source files".to_string());
    }
    let revision = store
        .revision_by_id(&remote_revision_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "shared folder data was incomplete".to_string())?;
    RestoreEngine::new(&store)
        .restore(&revision, &current)
        .map_err(|error| error.to_string())?;
    let restored = capture_worktree(&store, RevisionBoundary::Sync)?;
    store
        .coalesce_folder_revision(RevisionBoundary::Sync, restored.file_versions())
        .map_err(|error| error.to_string())?;
    store
        .upsert_remote(
            RemoteConfig::new(
                DEFAULT_REMOTE_NAME,
                DEVBOX_HOSTED_REMOTE_KIND,
                remote_config.clone_url(),
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    upsert_managed_folder(&mut config, &store, false)?;
    write_config(&config)?;

    println!("Cloned shared folder: {}", folder.display_name);
    println!("Folder: {}", store.folder_root().display());
    println!("Sync: ready");
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
    let store = LocalStore::open(&folder.local_path).map_err(|error| error.to_string())?;
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

fn product_status() -> Result<(), String> {
    let config = read_config()?;
    let Some(session) = config.session_or_none() else {
        println!("Logged in: no");
        println!("Run devbox login to connect this machine.");
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
    for folder in &config.shared_folders {
        let sync_state = managed_folder_sync_state(folder);
        println!("- {}: {} ({})", folder.name, folder.local_path, sync_state);
    }
    Ok(())
}

fn sync_once(store: &LocalStore, session: &Session) -> Result<WorktreeCapture, String> {
    let capture = capture_worktree(store, RevisionBoundary::Sync)?;
    ensure_no_blocked_or_deferred(&capture, "share")?;
    store
        .coalesce_folder_revision(RevisionBoundary::Sync, capture.file_versions())
        .map_err(|error| error.to_string())?;
    let remote_config = DevboxHostedRemoteConfig::new(
        &session.api_url,
        store.shared_folder().id().clone(),
        &session.session_token,
        &session.device_id,
    )
    .map_err(|error| error.to_string())?;
    let remote = DevboxHostedRemote::new(remote_config);
    sync_store_to_remote(store, &remote).map_err(|error| error.to_string())?;
    Ok(capture)
}

fn configure_hosted_remote(store: &LocalStore, session: &Session) -> Result<(), String> {
    let config = DevboxHostedRemoteConfig::new(
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
                DEVBOX_HOSTED_REMOTE_KIND,
                config.clone_url(),
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())
}

fn capture_worktree(
    store: &LocalStore,
    boundary: RevisionBoundary,
) -> Result<WorktreeCapture, String> {
    CaptureEngine::new(store.object_cache())
        .capture(&CaptureRequest::new(
            store.shared_folder().clone(),
            boundary,
        ))
        .map_err(|error| error.to_string())
}

fn ensure_no_blocked_or_deferred(capture: &WorktreeCapture, action: &str) -> Result<(), String> {
    if let Some(notice) = capture.blocked().first() {
        return Err(format!(
            "{action} refused because {} is secret-blocked: {}",
            display_store_path(notice.relative_path()),
            notice.reason()
        ));
    }
    if let Some(notice) = capture.deferred().first() {
        return Err(format!(
            "{action} refused because {} is deferred: {}",
            display_store_path(notice.relative_path()),
            notice.reason()
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
        .map_err(|error| error.to_string())?;
    if report.already_running {
        println!("Live sync: already running");
    } else {
        println!("Live sync: running");
    }
    Ok(())
}

fn stop_background_sync_for_path(path: &Path) -> Result<(), String> {
    if path.join(".loom").is_dir() {
        loom_daemon::request_stop(path).map_err(|error| error.to_string())?;
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
            .map_err(|error| format!("could not read Devbox config {}: {error}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ProductConfig::default()),
        Err(error) => Err(format!(
            "could not read Devbox config {}: {error}",
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
        return Ok(PathBuf::from(path).join("Devbox"));
    }
    if let Ok(path) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        return Ok(PathBuf::from(path).join(".devbox"));
    }
    Ok(std::env::current_dir()
        .map_err(|error| error.to_string())?
        .join(".devbox"))
}

impl ProductConfig {
    fn session(&self) -> Result<Session, String> {
        self.session_or_none()
            .ok_or_else(|| "not logged in; run devbox login first".to_string())
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
    let mut api_url = std::env::var(API_URL_ENV).ok();
    let mut account_hint =
        std::env::var("DEVBOX_ACCOUNT").unwrap_or_else(|_| "local-dev".to_string());
    let mut device_name = default_device_name();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--api" => {
                index += 1;
                api_url = Some(required_value(args, index, "--api")?);
            }
            "--account" => {
                index += 1;
                account_hint = required_value(args, index, "--account")?;
            }
            "--device-name" => {
                index += 1;
                device_name = required_value(args, index, "--device-name")?;
            }
            value => return Err(format!("login unknown option '{value}'")),
        }
        index += 1;
    }
    let api_url = api_url.ok_or_else(|| {
        "login needs a Devbox API URL; pass --api <URL> or set DEVBOX_API_URL".to_string()
    })?;
    validate_api_url(&api_url)?;
    Ok(LoginArgs {
        api_url,
        account_hint,
        device_name,
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
    for arg in args {
        match arg.as_str() {
            "--no-background-sync" => start_background_sync = false,
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
        }),
        [name] => Ok(CloneArgs {
            name: Some(name.clone()),
            target: None,
            start_background_sync,
        }),
        [name, target] => Ok(CloneArgs {
            name: Some(name.clone()),
            target: Some(PathBuf::from(target)),
            start_background_sync,
        }),
        _ => Err("clone accepts at most a shared folder name and target folder".to_string()),
    }
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
        .set("x-devbox-device-id", &session.device_id)
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
            "Devbox session was rejected; run devbox login again".to_string()
        }
        UreqError::Status(status, _) if status == 403 => {
            "this account or machine cannot access that shared folder".to_string()
        }
        UreqError::Status(status, _) => format!("Devbox API request failed with HTTP {status}"),
        UreqError::Transport(error) => format!("could not reach Devbox API: {error}"),
    }
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
            eprintln!("devbox: {error}");
            ExitCode::from(1)
        }
    }
}
