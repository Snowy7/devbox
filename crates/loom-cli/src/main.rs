use loom_core::RevisionBoundary;
use loom_store::{
    path_to_store_string, revision_boundary_to_store, CoalescedRevision, LocalStore, RemoteConfig,
    ResolvedRevisionTarget,
};
use loom_sync::{
    import_pack, sync_store_to_remote, LocalFilesystemRemote, LoomRemote, DEFAULT_REMOTE_NAME,
    LOCAL_FILESYSTEM_REMOTE_KIND,
};
use loom_worktree::{
    diff_revision_to_capture, evaluate_directory_policy, CaptureEngine, CaptureRequest,
    DirectoryPolicyDecision, RestoreEngine, WorktreeCapture, WorktreeDiff,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy)]
struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    implemented: bool,
    planned_behavior: &'static str,
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "track",
        usage: "loom track <FOLDER>",
        summary: "Start tracking a shared folder",
        implemented: true,
        planned_behavior: "capture file versions and make the folder durable locally",
    },
    CommandSpec {
        name: "status",
        usage: "loom status [FOLDER]",
        summary: "Show local folder state",
        implemented: true,
        planned_behavior:
            "summarize pending file versions, current folder revision, and sync cursor state",
    },
    CommandSpec {
        name: "history",
        usage: "loom history [FOLDER]",
        summary: "List folder revisions and checkpoints",
        implemented: true,
        planned_behavior: "show automatic folder revisions plus human checkpoints",
    },
    CommandSpec {
        name: "diff",
        usage: "loom diff [FOLDER] [REVISION|CHECKPOINT]",
        summary: "Compare the working folder to local history",
        implemented: true,
        planned_behavior: "summarize created, modified, deleted, blocked, and ignored entries",
    },
    CommandSpec {
        name: "checkpoint",
        usage: "loom checkpoint [FOLDER] -m <MESSAGE>",
        summary: "Name the current folder revision",
        implemented: true,
        planned_behavior: "attach a human message to a durable folder revision",
    },
    CommandSpec {
        name: "restore",
        usage: "loom restore [FOLDER] <REVISION|CHECKPOINT>",
        summary: "Restore a folder revision",
        implemented: true,
        planned_behavior: "materialize a previous folder revision with safety checks",
    },
    CommandSpec {
        name: "remote",
        usage: "loom remote add <NAME> <LOCAL_PATH> [FOLDER]",
        summary: "Configure a Loom remote endpoint",
        implemented: true,
        planned_behavior: "remember a named Loom folder-state endpoint",
    },
    CommandSpec {
        name: "sync",
        usage: "loom sync [FOLDER]",
        summary: "Synchronize a shared folder",
        implemented: true,
        planned_behavior: "reconcile local and remote folder revisions through Loom cursors",
    },
    CommandSpec {
        name: "clone",
        usage: "loom clone <REMOTE> <FOLDER>",
        summary: "Materialize a shared folder on this machine",
        implemented: true,
        planned_behavior: "create a local shared folder from a Loom remote without assuming Git",
    },
];

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    run(args)
}

fn run(args: Vec<String>) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") | Some("version") => {
            println!("loom {VERSION}");
            ExitCode::SUCCESS
        }
        Some("--help") | Some("-h") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(command_name) => match COMMANDS.iter().find(|command| command.name == command_name) {
            Some(command)
                if args
                    .get(1)
                    .is_some_and(|arg| arg == "--help" || arg == "-h") =>
            {
                print_command_help(command);
                ExitCode::SUCCESS
            }
            Some(command) => run_command(command, &args[1..]),
            None => {
                eprintln!("loom: unknown command '{command_name}'");
                eprintln!("Run 'loom --help' for usage.");
                ExitCode::from(2)
            }
        },
    }
}

fn run_command(command: &CommandSpec, args: &[String]) -> ExitCode {
    match command.name {
        "track" => result_to_exit(run_track(args)),
        "status" => result_to_exit(run_status(args)),
        "history" => result_to_exit(run_history(args)),
        "diff" => result_to_exit(run_diff(args)),
        "checkpoint" => result_to_exit(run_checkpoint(args)),
        "restore" => result_to_exit(run_restore(args)),
        "remote" => result_to_exit(run_remote(args)),
        "sync" => result_to_exit(run_sync(args)),
        "clone" => result_to_exit(run_clone(args)),
        _ => {
            run_placeholder(command);
            ExitCode::SUCCESS
        }
    }
}

fn run_track(args: &[String]) -> Result<(), String> {
    let folder = required_folder_arg("track", args)?;
    let opened = LocalStore::open_or_init(&folder).map_err(|error| error.to_string())?;
    let initialized = opened.initialized();
    let store = opened.into_store();
    let (capture, coalesced) = capture_and_coalesce(&store)?;

    if initialized {
        println!(
            "Initialized Loom tracking for {}",
            store.folder_root().display()
        );
    } else {
        println!("Opened Loom tracking for {}", store.folder_root().display());
    }
    println!("Store: {}", store.store_root().display());
    print_capture_result(&capture, &coalesced);
    Ok(())
}

fn run_status(args: &[String]) -> Result<(), String> {
    let store = open_existing_store(args)?;
    let (capture, coalesced) = capture_and_coalesce(&store)?;

    println!("Folder: {}", store.folder_root().display());
    if coalesced.created() {
        println!(
            "Captured new folder revision: {}",
            coalesced.revision().id()
        );
    } else {
        println!("Current folder revision: {}", coalesced.revision().id());
        println!("No source changes since the latest folder revision.");
    }
    print_status_summary(&capture, &coalesced);
    Ok(())
}

fn run_history(args: &[String]) -> Result<(), String> {
    let store = open_existing_store(args)?;
    let revisions = store.revisions().map_err(|error| error.to_string())?;
    let checkpoints = store.checkpoints().map_err(|error| error.to_string())?;
    let pins = store.pins().map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    if revisions.is_empty() {
        println!("No folder revisions yet.");
        println!("Run 'loom status' to capture the current folder.");
        return Ok(());
    }

    println!("Folder revision history:");
    for revision in revisions.iter().rev() {
        println!(
            "{}  {}  boundary={}  entries={}  parent={}",
            revision.id(),
            revision.created_at(),
            revision_boundary_to_store(revision.boundary()),
            revision.entries().len(),
            revision
                .parent_id()
                .map(ToString::to_string)
                .unwrap_or_else(|| "-".to_string())
        );
    }

    if checkpoints.is_empty() {
        println!("Checkpoints: none");
    } else {
        println!("Checkpoints:");
        for checkpoint in checkpoints.iter().rev() {
            let pin_count = pins
                .iter()
                .filter(|pin| pin.revision_id() == checkpoint.revision_id())
                .count();
            println!(
                "{}  {}  revision={}  pins={}  {}",
                checkpoint.id(),
                checkpoint.created_at(),
                checkpoint.revision_id(),
                pin_count,
                checkpoint.message()
            );
        }
    }

    Ok(())
}

fn run_diff(args: &[String]) -> Result<(), String> {
    let parsed = parse_diff_args(args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let target = match parsed.target {
        Some(target) => store
            .resolve_revision_target(&target)
            .map_err(|error| error.to_string())?,
        None => ResolvedRevisionTarget::Revision(
            store
                .latest_revision()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "no folder revisions yet; run 'loom status' first".to_string())?,
        ),
    };
    let capture = capture_worktree(&store, RevisionBoundary::LoomCommand)?;
    let diff =
        diff_revision_to_capture(target.revision(), &capture).map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    print_target("Compared to", &target);
    print_worktree_diff(&diff);
    print_policy_summary(&capture);
    print_notices("Blocked", capture.blocked());
    print_notices("Ignored", capture.ignored());
    Ok(())
}

fn run_checkpoint(args: &[String]) -> Result<(), String> {
    let parsed = parse_checkpoint_args(args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let revision = store
        .latest_revision()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "no folder revisions yet; run 'loom status' first".to_string())?;
    let capture = capture_worktree(&store, RevisionBoundary::LoomCommand)?;
    ensure_no_blocked_or_deferred(&capture, "checkpoint")?;
    let diff = diff_revision_to_capture(&revision, &capture).map_err(|error| error.to_string())?;
    if diff.has_changes() {
        return Err(
            "checkpoint refused because the working folder differs from the latest folder revision; run 'loom status' first"
                .to_string(),
        );
    }

    let checkpoint = store
        .create_checkpoint(&revision, parsed.message)
        .map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("Checkpoint: {}", checkpoint.id());
    println!("Revision: {}", checkpoint.revision_id());
    println!("Message: {}", checkpoint.message());
    println!("Pinned: revision kept for checkpoint retention");
    Ok(())
}

fn run_restore(args: &[String]) -> Result<(), String> {
    let parsed = parse_restore_args(args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let target = store
        .resolve_revision_target(&parsed.target)
        .map_err(|error| error.to_string())?;
    let current = capture_worktree(&store, RevisionBoundary::Restore)?;
    ensure_no_blocked_or_deferred(&current, "restore")?;
    let current_diff =
        diff_revision_to_capture(target.revision(), &current).map_err(|error| error.to_string())?;
    let pre_restore = store
        .coalesce_folder_revision(RevisionBoundary::Restore, current.file_versions())
        .map_err(|error| error.to_string())?;
    let report = RestoreEngine::new(&store)
        .restore(target.revision(), &current)
        .map_err(|error| error.to_string())?;
    let restored_capture = capture_worktree(&store, RevisionBoundary::Restore)?;
    let restored = store
        .coalesce_folder_revision(RevisionBoundary::Restore, restored_capture.file_versions())
        .map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    print_target("Restored", &target);
    println!("Pre-restore revision: {}", pre_restore.revision().id());
    println!("Current folder revision: {}", restored.revision().id());
    println!(
        "Restore changes: {} removed, {} reverted, {} restored, {} unchanged",
        report.diff().created().len(),
        report.diff().modified().len(),
        report.diff().deleted().len(),
        current_diff.unchanged()
    );
    Ok(())
}

fn run_remote(args: &[String]) -> Result<(), String> {
    match args {
        [subcommand, name, location] if subcommand == "add" => {
            run_remote_add(name, location, None)
        }
        [subcommand, name, location, folder] if subcommand == "add" => {
            run_remote_add(name, location, Some(PathBuf::from(folder)))
        }
        _ => Err(
            "remote command requires 'add <NAME> <LOCAL_PATH> [FOLDER]'\nUsage: loom remote add <NAME> <LOCAL_PATH> [FOLDER]"
                .to_string(),
        ),
    }
}

fn run_remote_add(name: &str, location: &str, folder: Option<PathBuf>) -> Result<(), String> {
    let store = open_store_for_sync_source(folder)?;
    let location = absolute_path(location)?;
    let location = location.to_string_lossy().into_owned();
    let remote = RemoteConfig::new(name, LOCAL_FILESYSTEM_REMOTE_KIND, location.clone())
        .map_err(|error| error.to_string())?;
    store
        .upsert_remote(remote)
        .map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("Remote: {name}");
    println!("Kind: {LOCAL_FILESYSTEM_REMOTE_KIND}");
    println!("Location: {location}");
    Ok(())
}

fn run_sync(args: &[String]) -> Result<(), String> {
    let store = match args {
        [] => open_store_for_sync_source(None)?,
        [folder] => open_store_for_sync_source(Some(PathBuf::from(folder)))?,
        _ => return Err("sync accepts at most one folder argument".to_string()),
    };
    let remote_config = store
        .remote(DEFAULT_REMOTE_NAME)
        .map_err(|error| error.to_string())?
        .or_else(|| store.remotes().ok().and_then(|mut remotes| remotes.pop()))
        .ok_or_else(|| {
            "no Loom remote configured; run 'loom remote add local <PATH>' first".to_string()
        })?;
    if remote_config.kind() != LOCAL_FILESYSTEM_REMOTE_KIND {
        return Err(format!(
            "remote {} uses unsupported kind {}",
            remote_config.name(),
            remote_config.kind()
        ));
    }

    let (capture, coalesced) = capture_and_coalesce_with_boundary(&store, RevisionBoundary::Sync)?;
    ensure_no_blocked_or_deferred(&capture, "sync")?;
    let remote = LocalFilesystemRemote::new(remote_config.location());
    let report = sync_store_to_remote(&store, &remote).map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("Remote: {}", remote_config.name());
    println!("Location: {}", remote.root().display());
    if coalesced.created() {
        println!("Captured sync revision: {}", coalesced.revision().id());
    } else {
        println!("Current folder revision: {}", coalesced.revision().id());
    }
    println!("Synced revision: {}", report.latest_revision_id);
    println!(
        "Remote cursor advanced from {}",
        report
            .previous_remote_revision_id
            .map(|revision| revision.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!("Pack objects: {}", report.uploaded_objects);
    print_policy_summary(&capture);
    Ok(())
}

fn run_clone(args: &[String]) -> Result<(), String> {
    let (remote_path, target) = match args {
        [remote_path, target] => (PathBuf::from(remote_path), PathBuf::from(target)),
        _ => return Err("clone requires a remote path and target folder".to_string()),
    };
    let remote = LocalFilesystemRemote::new(remote_path);
    let remote_revision_id = remote
        .get_cursor(loom_sync::DEFAULT_CURSOR_ID)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "remote has no shared-folder cursor; run 'loom sync' first".to_string())?;
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
    let import_report = import_pack(&store, &pack).map_err(|error| error.to_string())?;
    let current = capture_worktree(&store, RevisionBoundary::Restore)?;
    ensure_no_blocked_or_deferred(&current, "clone")?;
    if !current.file_versions().is_empty() {
        return Err(
            "clone refused because the target already contains source files; choose an empty folder"
                .to_string(),
        );
    }
    let revision = store
        .revision_by_id(&remote_revision_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("imported pack did not contain revision {remote_revision_id}"))?;
    let report = RestoreEngine::new(&store)
        .restore(&revision, &current)
        .map_err(|error| error.to_string())?;
    let restored_capture = capture_worktree(&store, RevisionBoundary::Sync)?;
    store
        .coalesce_folder_revision(RevisionBoundary::Sync, restored_capture.file_versions())
        .map_err(|error| error.to_string())?;

    println!("Cloned revision: {}", report.revision_id());
    println!("Target: {}", store.folder_root().display());
    println!("Imported objects: {}", import_report.imported_objects);
    println!(
        "Imported metadata: {} file versions, {} revisions, {} checkpoints, {} pins",
        import_report.imported_file_versions,
        import_report.imported_revisions,
        import_report.imported_checkpoints,
        import_report.imported_pins
    );
    println!(
        "Materialized: {} created, {} modified, {} removed",
        report.diff().deleted().len(),
        report.diff().modified().len(),
        report.diff().created().len()
    );
    Ok(())
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
        return Err(
            "clone refused because the target already contains a Loom store; choose an untracked folder"
                .to_string(),
        );
    }
    if let Some(source_path) = first_clone_source_entry(target, target)? {
        return Err(format!(
            "clone refused because the target already contains source files; choose an empty folder: {}",
            path_to_store_string(&source_path)
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

fn capture_and_coalesce(
    store: &LocalStore,
) -> Result<(WorktreeCapture, CoalescedRevision), String> {
    capture_and_coalesce_with_boundary(store, RevisionBoundary::LoomCommand)
}

fn capture_and_coalesce_with_boundary(
    store: &LocalStore,
    boundary: RevisionBoundary,
) -> Result<(WorktreeCapture, CoalescedRevision), String> {
    let capture = capture_worktree(store, boundary)?;
    let coalesced = store
        .coalesce_folder_revision(boundary, capture.file_versions())
        .map_err(|error| error.to_string())?;

    Ok((capture, coalesced))
}

fn capture_worktree(
    store: &LocalStore,
    boundary: RevisionBoundary,
) -> Result<WorktreeCapture, String> {
    let request = CaptureRequest::new(store.shared_folder().clone(), boundary);
    let capture = CaptureEngine::new(store.object_cache())
        .capture(&request)
        .map_err(|error| error.to_string())?;
    Ok(capture)
}

fn print_capture_result(capture: &WorktreeCapture, coalesced: &CoalescedRevision) {
    if coalesced.created() {
        println!("Captured folder revision: {}", coalesced.revision().id());
    } else {
        println!("Current folder revision: {}", coalesced.revision().id());
    }
    print_status_summary(capture, coalesced);
}

fn print_status_summary(capture: &WorktreeCapture, coalesced: &CoalescedRevision) {
    let diff = coalesced.diff();
    println!(
        "Changes: {} created, {} modified, {} deleted, {} unchanged",
        diff.created(),
        diff.modified(),
        diff.deleted(),
        diff.unchanged()
    );
    println!(
        "Captured: {} files, {} folders, {} bytes, {} new file versions",
        capture.summary().captured_files(),
        capture.summary().captured_directories(),
        capture.summary().total_file_bytes(),
        coalesced.new_file_versions()
    );
    println!(
        "Policy: {} ignored, {} secret-blocked, {} deferred",
        capture.summary().ignored_entries(),
        capture.summary().blocked_secret_files(),
        capture.summary().deferred_entries()
    );

    for notice in capture.blocked().iter().take(3) {
        println!(
            "Blocked: {} ({})",
            path_to_store_string(notice.relative_path()),
            notice.reason()
        );
    }
}

fn print_target(prefix: &str, target: &ResolvedRevisionTarget) {
    match target {
        ResolvedRevisionTarget::Revision(revision) => {
            println!("{prefix}: revision {}", revision.id());
        }
        ResolvedRevisionTarget::Checkpoint {
            checkpoint,
            revision,
        } => {
            println!(
                "{prefix}: checkpoint {} ({}) -> revision {}",
                checkpoint.id(),
                checkpoint.message(),
                revision.id()
            );
        }
    }
}

fn print_worktree_diff(diff: &WorktreeDiff) {
    println!(
        "Changes: {} created, {} modified, {} deleted, {} unchanged",
        diff.created().len(),
        diff.modified().len(),
        diff.deleted().len(),
        diff.unchanged()
    );
    print_paths("Created", diff.created());
    print_paths("Modified", diff.modified());
    print_paths("Deleted", diff.deleted());
}

fn print_paths(label: &str, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }

    println!("{label}:");
    for path in paths.iter().take(20) {
        println!("  {}", path_to_store_string(path));
    }
    if paths.len() > 20 {
        println!("  ... {} more", paths.len() - 20);
    }
}

fn print_policy_summary(capture: &WorktreeCapture) {
    println!(
        "Policy: {} ignored, {} secret-blocked, {} deferred",
        capture.summary().ignored_entries(),
        capture.summary().blocked_secret_files(),
        capture.summary().deferred_entries()
    );
}

fn print_notices(label: &str, notices: &[loom_worktree::CaptureNotice]) {
    if notices.is_empty() {
        return;
    }

    println!("{label}:");
    for notice in notices.iter().take(20) {
        println!(
            "  {} ({})",
            path_to_store_string(notice.relative_path()),
            notice.reason()
        );
    }
    if notices.len() > 20 {
        println!("  ... {} more", notices.len() - 20);
    }
}

fn required_folder_arg(command: &str, args: &[String]) -> Result<PathBuf, String> {
    match args {
        [folder] => Ok(PathBuf::from(folder)),
        [] => Err(format!(
            "{command} requires a folder\nUsage: loom {command} <FOLDER>"
        )),
        _ => Err(format!("{command} accepts exactly one folder")),
    }
}

fn open_existing_store(args: &[String]) -> Result<LocalStore, String> {
    match args {
        [] => {
            let current_dir = std::env::current_dir().map_err(|error| error.to_string())?;
            LocalStore::discover_from(current_dir).map_err(|error| error.to_string())
        }
        [folder] => LocalStore::open(folder).map_err(|error| error.to_string()),
        _ => Err("expected at most one folder argument".to_string()),
    }
}

fn open_store_from_optional_folder(folder: Option<PathBuf>) -> Result<LocalStore, String> {
    match folder {
        Some(folder) => LocalStore::open(folder).map_err(|error| error.to_string()),
        None => {
            let current_dir = std::env::current_dir().map_err(|error| error.to_string())?;
            LocalStore::discover_from(current_dir).map_err(|error| error.to_string())
        }
    }
}

fn open_store_for_sync_source(folder: Option<PathBuf>) -> Result<LocalStore, String> {
    if folder.is_some() {
        return open_store_from_optional_folder(folder);
    }

    let current_dir = std::env::current_dir().map_err(|error| error.to_string())?;
    match LocalStore::discover_from(&current_dir) {
        Ok(store) => Ok(store),
        Err(discover_error) => {
            let mut candidates = Vec::new();
            for entry in fs::read_dir(&current_dir).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let path = entry.path();
                if path.is_dir()
                    && path
                        .join(".loom")
                        .join("metadata")
                        .join("shared_folder.tsv")
                        .is_file()
                {
                    candidates.push(path);
                }
            }
            match candidates.as_slice() {
                [candidate] => LocalStore::open(candidate).map_err(|error| error.to_string()),
                [] => Err(discover_error.to_string()),
                _ => Err(
                    "multiple tracked child folders found; pass the folder path explicitly"
                        .to_string(),
                ),
            }
        }
    }
}

fn absolute_path(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        return Ok(path);
    }

    Ok(std::env::current_dir()
        .map_err(|error| error.to_string())?
        .join(path))
}

#[derive(Debug, Clone)]
struct CheckpointArgs {
    folder: Option<PathBuf>,
    message: String,
}

#[derive(Debug, Clone)]
struct DiffArgs {
    folder: Option<PathBuf>,
    target: Option<String>,
}

#[derive(Debug, Clone)]
struct RestoreArgs {
    folder: Option<PathBuf>,
    target: String,
}

fn parse_checkpoint_args(args: &[String]) -> Result<CheckpointArgs, String> {
    let mut folder = None;
    let mut message = None;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "-m" || arg == "--message" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "checkpoint requires a message after -m".to_string())?;
            message = Some(value.clone());
        } else if let Some(value) = arg.strip_prefix("--message=") {
            message = Some(value.to_string());
        } else if folder.is_none() {
            folder = Some(PathBuf::from(arg));
        } else {
            return Err("checkpoint accepts at most one folder".to_string());
        }
        index += 1;
    }

    let message = message.ok_or_else(|| {
        "checkpoint requires a message\nUsage: loom checkpoint [FOLDER] -m <MESSAGE>".to_string()
    })?;
    if message.trim().is_empty() {
        return Err("checkpoint message cannot be empty".to_string());
    }

    Ok(CheckpointArgs { folder, message })
}

fn parse_diff_args(args: &[String]) -> Result<DiffArgs, String> {
    match args {
        [] => Ok(DiffArgs {
            folder: None,
            target: None,
        }),
        [single] if looks_like_folder_arg(single) => Ok(DiffArgs {
            folder: Some(PathBuf::from(single)),
            target: None,
        }),
        [single] => Ok(DiffArgs {
            folder: None,
            target: Some(single.clone()),
        }),
        [folder, target] => Ok(DiffArgs {
            folder: Some(PathBuf::from(folder)),
            target: Some(target.clone()),
        }),
        _ => Err("diff accepts an optional folder and optional revision/checkpoint".to_string()),
    }
}

fn parse_restore_args(args: &[String]) -> Result<RestoreArgs, String> {
    match args {
        [target] => Ok(RestoreArgs {
            folder: None,
            target: target.clone(),
        }),
        [folder, target] => Ok(RestoreArgs {
            folder: Some(PathBuf::from(folder)),
            target: target.clone(),
        }),
        [] => Err("restore requires a revision or checkpoint".to_string()),
        _ => Err("restore accepts an optional folder plus one revision/checkpoint".to_string()),
    }
}

fn looks_like_folder_arg(value: &str) -> bool {
    Path::new(value).is_dir()
}

fn ensure_no_blocked_or_deferred(capture: &WorktreeCapture, command: &str) -> Result<(), String> {
    if let Some(notice) = capture.blocked().first() {
        return Err(format!(
            "{command} refused because {} is secret-blocked: {}",
            path_to_store_string(notice.relative_path()),
            notice.reason()
        ));
    }

    if let Some(notice) = capture.deferred().first() {
        return Err(format!(
            "{command} refused because {} is deferred: {}",
            path_to_store_string(notice.relative_path()),
            notice.reason()
        ));
    }

    Ok(())
}

fn result_to_exit(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("loom: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_placeholder(command: &CommandSpec) {
    println!("loom {}: not implemented yet", command.name);
    println!("Purpose: {}", command.summary);
    println!("Planned behavior: {}", command.planned_behavior);
}

fn print_help() {
    println!("loom {VERSION}");
    println!();
    println!("Usage: loom <COMMAND>");
    println!();
    println!("Commands:");
    for command in COMMANDS {
        println!("  {:<10} {}", command.name, command.summary);
    }
    println!();
    println!("Options:");
    println!("  -h, --help     Print help");
    println!("  -V, --version  Print version");
}

fn print_command_help(command: &CommandSpec) {
    println!("loom {} - {}", command.name, command.summary);
    println!();
    println!("Usage: {}", command.usage);
    println!();
    if command.implemented {
        println!("Status: implemented for the local offline engine");
    } else {
        println!("Status: not implemented yet");
    }
    println!("Planned behavior: {}", command.planned_behavior);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_include_mvp_surface() {
        let names = COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "track",
                "status",
                "history",
                "diff",
                "checkpoint",
                "restore",
                "remote",
                "sync",
                "clone"
            ]
        );
    }
}
