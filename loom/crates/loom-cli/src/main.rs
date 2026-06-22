use devbox_remote::{
    provision_devbox_hosted_remote, DevboxHostedRemote, DevboxHostedRemoteConfig,
    DEVBOX_HOSTED_REMOTE_KIND,
};
use loom_core::{
    FileKind, FileVersion, RevisionBoundary, WorkspaceKind, WorkspaceSessionId,
    WorkspaceSessionState,
};
use loom_daemon::{DaemonLoopOptions, DaemonStartOptions};
use loom_store::{
    path_to_store_string, revision_boundary_to_store, CoalescedRevision, LocalStore, RemoteConfig,
    ResolvedRevisionTarget, StoreVerificationReport, VerificationLevel,
};
use loom_sync::{
    check_remote_availability, hydrate_object_from_remote, import_pack_from_remote,
    import_pack_metadata_only_from_remote, sync_store_to_remote, LocalFilesystemRemote, LoomRemote,
    RemoteCheckReport, DEFAULT_REMOTE_NAME, LOCAL_FILESYSTEM_REMOTE_KIND,
};
use loom_workspace::{
    AgentWorkspaceAdapter, WorkspaceEntryMetadata, WorkspaceEntrySource, WorkspaceOverlayDiff,
    WorkspaceSessionRequest, WorkspaceView,
};
use loom_worktree::{
    cache_policy_presets, cache_status_for_scope, diff_revision_to_capture,
    evaluate_directory_policy, evict_versions, hydrate_versions, prefetch_versions_for_scope,
    prune_cache_to_limit, relative_scope_path, tracked_versions_for_scope, warm_versions_for_scope,
    CaptureEngine, CaptureRequest, DirectoryPolicyDecision, RestoreEngine, WorktreeCapture,
    WorktreeDiff, DEFAULT_PREFETCH_MAX_BYTES, DEFAULT_WARM_MAX_BYTES,
};
use std::collections::BTreeSet;
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
        name: "doctor",
        usage: "loom doctor [FOLDER]",
        summary: "Inspect local and remote Loom health",
        implemented: true,
        planned_behavior: "run report-only metadata, object, cache, policy, and remote checks",
    },
    CommandSpec {
        name: "fsck",
        usage: "loom fsck [FOLDER]",
        summary: "Verify local Loom metadata and object integrity",
        implemented: true,
        planned_behavior: "report invalid local metadata references, object bytes, and cache state",
    },
    CommandSpec {
        name: "object",
        usage: "loom object verify [FOLDER]",
        summary: "Verify local Loom object bytes",
        implemented: true,
        planned_behavior: "hash local object bytes and compare them to content object ids",
    },
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
        usage: "loom remote add <NAME> <LOCAL_PATH|DEVBOX_API_URL> [FOLDER]\n       loom remote check [FOLDER]",
        summary: "Configure a Loom remote endpoint",
        implemented: true,
        planned_behavior: "remember and inspect a named Loom folder-state endpoint",
    },
    CommandSpec {
        name: "sync",
        usage: "loom sync [FOLDER]\n       loom sync start [FOLDER]\n       loom sync stop [FOLDER]\n       loom sync status [FOLDER]",
        summary: "Synchronize a shared folder",
        implemented: true,
        planned_behavior:
            "reconcile local and remote folder revisions through Loom cursors, once or in the background",
    },
    CommandSpec {
        name: "clone",
        usage: "loom clone <REMOTE> <FOLDER> [--sparse]",
        summary: "Materialize a shared folder on this machine",
        implemented: true,
        planned_behavior: "create a local shared folder from a Loom remote without assuming Git",
    },
    CommandSpec {
        name: "hydrate",
        usage: "loom hydrate <PATH|FOLDER>",
        summary: "Fetch and materialize bytes for a path or subtree",
        implemented: true,
        planned_behavior: "turn remote-only folder metadata into local files on demand",
    },
    CommandSpec {
        name: "evict",
        usage: "loom evict <PATH|FOLDER>",
        summary: "Remove clean materialized bytes for a path or subtree",
        implemented: true,
        planned_behavior: "free local bytes while keeping folder history intact",
    },
    CommandSpec {
        name: "pin",
        usage: "loom pin <PATH|FOLDER>",
        summary: "Keep a path or subtree available locally",
        implemented: true,
        planned_behavior: "record local offline intent that prevents explicit eviction",
    },
    CommandSpec {
        name: "cache",
        usage: "loom cache status [FOLDER]\n       loom cache warm <PATH|FOLDER> [--manifest] [--max-bytes <BYTES>]\n       loom cache free-space --max-bytes <BYTES> [FOLDER]\n       loom cache prune --max-bytes <BYTES> [FOLDER]\n       loom cache policy show",
        summary: "Show local cache hydration state",
        implemented: true,
        planned_behavior: "summarize hydration, warm useful files, free clean bytes over a limit, and show internal policy presets",
    },
    CommandSpec {
        name: "workspace",
        usage: "loom workspace open [FOLDER] [--session <ID>] [--revision <REVISION|CHECKPOINT>]\n       loom workspace sessions [FOLDER]\n       loom workspace list [FOLDER] --session <ID> [PATH]\n       loom workspace read [FOLDER] --session <ID> <PATH>\n       loom workspace write [FOLDER] --session <ID> <PATH> --text <TEXT>\n       loom workspace hydrate [FOLDER] --session <ID> <PATH>\n       loom workspace dehydrate [FOLDER] --session <ID> <PATH>\n       loom workspace pin [FOLDER] --session <ID> <PATH>\n       loom workspace diff [FOLDER] --session <ID>\n       loom workspace checkpoint [FOLDER] --session <ID> -m <MESSAGE>\n       loom workspace close [FOLDER] --session <ID>\n       loom workspace discard [FOLDER] --session <ID>",
        summary: "Use virtual workspace sessions over Loom revisions",
        implemented: true,
        planned_behavior: "open agent virtual sessions, read lazily, write isolated overlays, diff, checkpoint, and discard without OS filesystem mounting",
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
        "doctor" => result_to_exit(run_doctor(args)),
        "fsck" => result_to_exit(run_fsck(args)),
        "object" => result_to_exit(run_object(args)),
        "track" => result_to_exit(run_track(args)),
        "status" => result_to_exit(run_status(args)),
        "history" => result_to_exit(run_history(args)),
        "diff" => result_to_exit(run_diff(args)),
        "checkpoint" => result_to_exit(run_checkpoint(args)),
        "restore" => result_to_exit(run_restore(args)),
        "remote" => result_to_exit(run_remote(args)),
        "sync" => result_to_exit(run_sync(args)),
        "clone" => result_to_exit(run_clone(args)),
        "hydrate" => result_to_exit(run_hydrate(args)),
        "evict" => result_to_exit(run_evict(args)),
        "pin" => result_to_exit(run_pin(args)),
        "cache" => result_to_exit(run_cache(args)),
        "workspace" => result_to_exit(run_workspace(args)),
        _ => {
            run_placeholder(command);
            ExitCode::SUCCESS
        }
    }
}

fn run_doctor(args: &[String]) -> Result<(), String> {
    let store = open_existing_store(args)?;
    let local_report = store
        .verify_integrity()
        .map_err(|error| error.to_string())?;
    let mut failed = local_report.has_errors();

    println!("Folder: {}", store.folder_root().display());
    println!("Doctor:");
    print_store_verification_report(&local_report);

    let capture = capture_worktree(&store, RevisionBoundary::LoomCommand)?;
    println!(
        "Worktree policy: {} ignored, {} secret-blocked, {} deferred",
        capture.summary().ignored_entries(),
        capture.summary().blocked_secret_files(),
        capture.summary().deferred_entries()
    );
    if !capture.blocked().is_empty() || !capture.deferred().is_empty() {
        failed = true;
        print_notices("Blocked", capture.blocked());
        print_notices("Deferred", capture.deferred());
    }

    match preferred_remote_for_check(&store) {
        Ok(remote_config) => {
            let remote = remote_from_config(&remote_config)?;
            let remote_report = check_remote_availability(&store, remote.as_ref())
                .map_err(|error| error.to_string())?;
            print_remote_check_report(&remote_config, &remote_report);
            failed |= remote_report.has_errors();
        }
        Err(_) => {
            println!("Remote: not configured");
            println!("Remote check: skipped");
        }
    }

    if failed {
        println!("Status: failed");
        return Err("doctor found Loom integrity problems".to_string());
    }
    if local_report.warning_count() > 0 {
        println!("Status: warnings");
    } else {
        println!("Status: healthy");
    }
    Ok(())
}

fn run_fsck(args: &[String]) -> Result<(), String> {
    let store = open_existing_store(args)?;
    let report = store
        .verify_integrity()
        .map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("FSCK:");
    print_store_verification_report(&report);
    if report.has_errors() {
        return Err("fsck found Loom integrity problems".to_string());
    }
    Ok(())
}

fn run_object(args: &[String]) -> Result<(), String> {
    match args {
        [subcommand] if subcommand == "verify" => run_object_verify(None),
        [subcommand, folder] if subcommand == "verify" => {
            run_object_verify(Some(PathBuf::from(folder)))
        }
        _ => {
            Err("object command requires 'verify'\nUsage: loom object verify [FOLDER]".to_string())
        }
    }
}

fn run_object_verify(folder: Option<PathBuf>) -> Result<(), String> {
    let store = open_store_from_optional_folder(folder)?;
    let report = store
        .verify_integrity()
        .map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("Object verification:");
    println!(
        "  checked local objects: {}",
        report.checked_local_objects()
    );
    print_store_issues(&report);
    if report.has_errors() {
        return Err("object verification found Loom integrity problems".to_string());
    }
    Ok(())
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
        [subcommand] if subcommand == "check" => run_remote_check(None),
        [subcommand, folder] if subcommand == "check" => run_remote_check(Some(PathBuf::from(folder))),
        _ => Err(
            "remote command requires 'add <NAME> <LOCAL_PATH> [FOLDER]' or 'check [FOLDER]'\nUsage: loom remote add <NAME> <LOCAL_PATH> [FOLDER]\n       loom remote check [FOLDER]"
                .to_string(),
        ),
    }
}

fn run_remote_add(name: &str, location: &str, folder: Option<PathBuf>) -> Result<(), String> {
    let store = open_store_for_sync_source(folder)?;
    let (kind, stored_location, display_location) = if looks_like_devbox_api_url(location) {
        let provisioned = provision_devbox_hosted_remote(
            location,
            store.shared_folder().id(),
            store.shared_folder().display_name(),
        )
        .map_err(|error| error.to_string())?;
        let clone_url = provisioned.config.clone_url();
        (
            DEVBOX_HOSTED_REMOTE_KIND,
            clone_url.clone(),
            format!(
                "{}\nClone URL: {}\nAccount: {}\nSession: {}",
                provisioned.config.api(),
                clone_url,
                provisioned.account_id,
                provisioned.session_id
            ),
        )
    } else {
        let location = absolute_path(location)?;
        let location = location.to_string_lossy().into_owned();
        (LOCAL_FILESYSTEM_REMOTE_KIND, location.clone(), location)
    };
    let remote = RemoteConfig::new(name, kind, stored_location.clone())
        .map_err(|error| error.to_string())?;
    store
        .upsert_remote(remote)
        .map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("Remote: {name}");
    println!("Kind: {kind}");
    println!("Location: {display_location}");
    Ok(())
}

fn run_remote_check(folder: Option<PathBuf>) -> Result<(), String> {
    let store = open_store_from_optional_folder(folder)?;
    let remote_config = preferred_remote_for_check(&store)?;
    let remote = remote_from_config(&remote_config)?;
    let report =
        check_remote_availability(&store, remote.as_ref()).map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    print_remote_check_report(&remote_config, &report);
    if report.has_errors() {
        return Err("remote check found Loom availability problems".to_string());
    }
    Ok(())
}

fn run_sync(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("start") => return run_sync_start(&args[1..]),
        Some("stop") => return run_sync_stop(&args[1..]),
        Some("status") => return run_sync_daemon_status(&args[1..]),
        Some("run-loop") => return run_sync_run_loop(&args[1..]),
        _ => {}
    }

    run_sync_once(args)
}

fn run_sync_once(args: &[String]) -> Result<(), String> {
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
    let (capture, coalesced) = capture_and_coalesce_with_boundary(&store, RevisionBoundary::Sync)?;
    ensure_no_blocked_or_deferred(&capture, "sync")?;
    let remote = remote_from_config(&remote_config)?;
    let report =
        sync_store_to_remote(&store, remote.as_ref()).map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("Remote: {}", remote_config.name());
    println!("Location: {}", remote_config.location());
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

fn run_sync_start(args: &[String]) -> Result<(), String> {
    let parsed = parse_sync_daemon_args(args, "start")?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let mut options = DaemonStartOptions::new(store.folder_root());
    options.debounce_ms = parsed.debounce_ms;
    options.poll_ms = parsed.poll_ms;
    let report = loom_daemon::start_background(&options).map_err(|error| error.to_string())?;

    println!("Folder: {}", report.folder.display());
    if report.already_running {
        println!("Background sync: already running");
    } else {
        println!("Background sync: starting");
    }
    println!("Daemon pid: {}", report.pid);
    println!("Status: {}", report.status_path.display());
    println!("Log: {}", report.log_path.display());
    Ok(())
}

fn run_sync_stop(args: &[String]) -> Result<(), String> {
    let folder = parse_optional_folder(args, "stop")?;
    let store = open_store_from_optional_folder(folder)?;
    let report =
        loom_daemon::request_stop(store.folder_root()).map_err(|error| error.to_string())?;

    println!("Folder: {}", report.folder.display());
    println!("Background sync: {}", report.status.state);
    println!("Stop request: {}", report.stop_path.display());
    print_daemon_status(&report.status);
    Ok(())
}

fn run_sync_daemon_status(args: &[String]) -> Result<(), String> {
    let folder = parse_optional_folder(args, "status")?;
    let store = open_store_from_optional_folder(folder)?;
    let status =
        loom_daemon::read_status(store.folder_root()).map_err(|error| error.to_string())?;

    println!("Folder: {}", status.folder.display());
    print_daemon_status(&status);
    Ok(())
}

fn run_sync_run_loop(args: &[String]) -> Result<(), String> {
    let parsed = parse_sync_daemon_args(args, "run-loop")?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let mut options = DaemonLoopOptions::new(store.folder_root());
    options.debounce_ms = parsed.debounce_ms;
    options.poll_ms = parsed.poll_ms;
    options.max_cycles = parsed.max_cycles;
    loom_daemon::run_loop(&options).map_err(|error| error.to_string())
}

fn run_clone(args: &[String]) -> Result<(), String> {
    let parsed = parse_clone_args(args)?;
    let remote_location = parsed.remote_location;
    let target = parsed.target;
    let (remote, remote_kind, stored_location) = clone_remote_from_location(&remote_location)?;
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
    let import_report = if parsed.sparse {
        import_pack_metadata_only_from_remote(&store, &pack, remote.as_ref())
            .map_err(|error| error.to_string())?
    } else {
        import_pack_from_remote(&store, &pack, remote.as_ref())
            .map_err(|error| error.to_string())?
    };
    let mut materialized_report = None;
    let revision = store
        .revision_by_id(&remote_revision_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("imported pack did not contain revision {remote_revision_id}"))?;
    if !parsed.sparse {
        let current = capture_worktree(&store, RevisionBoundary::Restore)?;
        ensure_no_blocked_or_deferred(&current, "clone")?;
        if !current.file_versions().is_empty() {
            return Err(
                "clone refused because the target already contains source files; choose an empty folder"
                    .to_string(),
            );
        }
        let report = RestoreEngine::new(&store)
            .restore(&revision, &current)
            .map_err(|error| error.to_string())?;
        let restored_capture = capture_worktree(&store, RevisionBoundary::Sync)?;
        store
            .coalesce_folder_revision(RevisionBoundary::Sync, restored_capture.file_versions())
            .map_err(|error| error.to_string())?;
        materialized_report = Some(report);
    }
    store
        .upsert_remote(
            RemoteConfig::new(DEFAULT_REMOTE_NAME, remote_kind, stored_location.clone())
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;

    println!("Cloned revision: {}", revision.id());
    println!("Target: {}", store.folder_root().display());
    println!("Remote: {}", stored_location);
    println!(
        "Mode: {}",
        if parsed.sparse {
            "sparse metadata-only"
        } else {
            "eager materialized"
        }
    );
    println!("Imported objects: {}", import_report.imported_objects);
    println!(
        "Imported metadata: {} file versions, {} revisions, {} checkpoints, {} pins",
        import_report.imported_file_versions,
        import_report.imported_revisions,
        import_report.imported_checkpoints,
        import_report.imported_pins
    );
    if let Some(report) = materialized_report {
        println!(
            "Materialized: {} created, {} modified, {} removed",
            report.diff().deleted().len(),
            report.diff().modified().len(),
            report.diff().created().len()
        );
    } else {
        println!("Materialized: 0 files (run 'loom hydrate <path>' when needed)");
    }
    Ok(())
}

fn run_hydrate(args: &[String]) -> Result<(), String> {
    let target = required_path_arg("hydrate", args)?;
    let store = open_store_for_path_or_folder(&target)?;
    let scope = relative_scope_path(&store, &absolute_path_from_path(&target)?)
        .map_err(|error| error.to_string())?;
    let versions = tracked_versions_for_scope(&store, &scope).map_err(|error| error.to_string())?;
    if versions.is_empty() {
        return Err(format!(
            "hydrate found no tracked files under {}",
            path_to_store_string(&scope)
        ));
    }

    let fetched_objects = fetch_missing_objects(&store, &versions)?;

    let report = hydrate_versions(&store, &versions).map_err(|error| error.to_string())?;
    println!("Folder: {}", store.folder_root().display());
    println!("Hydrated: {}", path_to_store_string(&scope));
    println!("Fetched objects: {fetched_objects}");
    println!(
        "Materialized: {} files, {} folders, {} already present",
        report.materialized_files(),
        report.materialized_directories(),
        report.already_materialized_files()
    );
    Ok(())
}

fn run_evict(args: &[String]) -> Result<(), String> {
    let target = required_path_arg("evict", args)?;
    let store = open_store_for_path_or_folder(&target)?;
    let scope = relative_scope_path(&store, &absolute_path_from_path(&target)?)
        .map_err(|error| error.to_string())?;
    let versions = tracked_versions_for_scope(&store, &scope).map_err(|error| error.to_string())?;
    if versions.is_empty() {
        return Err(format!(
            "evict found no tracked files under {}",
            path_to_store_string(&scope)
        ));
    }
    let pinned_scopes = local_pinned_scopes(&store)?;
    let remote_available_objects =
        remote_available_objects_for_versions(&store, &versions, "evict")?;
    let report = evict_versions(&store, &versions, &pinned_scopes, &remote_available_objects)
        .map_err(|error| error.to_string())?;
    println!("Folder: {}", store.folder_root().display());
    println!("Evicted: {}", path_to_store_string(&scope));
    println!(
        "Removed: {} files, {} objects; already remote-only: {} files",
        report.evicted_files(),
        report.evicted_objects(),
        report.already_remote_files()
    );
    Ok(())
}

fn run_pin(args: &[String]) -> Result<(), String> {
    let target = required_path_arg("pin", args)?;
    let store = open_store_for_path_or_folder(&target)?;
    let scope = relative_scope_path(&store, &absolute_path_from_path(&target)?)
        .map_err(|error| error.to_string())?;
    let revision = store
        .latest_revision()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "no folder revisions yet; run 'loom status' first".to_string())?;
    let pin = store
        .pin_revision(
            revision.id(),
            format!("materialization-pin path={}", path_to_store_string(&scope)),
        )
        .map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("Pinned: {}", path_to_store_string(&scope));
    println!("Revision: {}", revision.id());
    println!("Pin: {}", pin.id());
    Ok(())
}

fn run_cache(args: &[String]) -> Result<(), String> {
    match args {
        [subcommand] if subcommand == "status" => run_cache_status(None),
        [subcommand, folder] if subcommand == "status" => {
            run_cache_status(Some(PathBuf::from(folder)))
        }
        [subcommand, rest @ ..] if subcommand == "warm" => run_cache_warm(rest),
        [subcommand, rest @ ..] if subcommand == "free-space" => {
            run_cache_free_space(rest, "cache free-space")
        }
        [subcommand, rest @ ..] if subcommand == "prune" => run_cache_free_space(rest, "cache prune"),
        [subcommand, rest @ ..] if subcommand == "prefetch" => run_cache_prefetch(rest),
        [subcommand, action] if subcommand == "policy" && action == "show" => run_cache_policy_show(),
        _ => Err(
            "cache command requires 'status', 'warm', 'free-space', 'prune', 'prefetch', or 'policy show'\nUsage: loom cache status [FOLDER]\n       loom cache warm <PATH|FOLDER> [--manifest] [--max-bytes <BYTES>]\n       loom cache free-space --max-bytes <BYTES> [FOLDER]\n       loom cache prune --max-bytes <BYTES> [FOLDER]\n       loom cache policy show"
                .to_string(),
        ),
    }
}

fn run_cache_status(folder: Option<PathBuf>) -> Result<(), String> {
    let store = open_store_from_optional_folder(folder)?;
    let versions =
        tracked_versions_for_scope(&store, Path::new("")).map_err(|error| error.to_string())?;
    let remote_metrics = remote_metrics_for_versions(&store, &versions);
    let remote_available_objects = match &remote_metrics {
        RemoteMetrics::Known {
            available_objects, ..
        } => available_objects.clone(),
        RemoteMetrics::Unknown { .. } => BTreeSet::new(),
    };
    let report = cache_status_for_scope(&store, Path::new(""), &remote_available_objects)
        .map_err(|error| error.to_string())?;
    println!("Folder: {}", store.folder_root().display());
    println!("Cache status:");
    println!("  hydrated: {}", report.hydrated_files());
    println!("  remote-only: {}", report.remote_only_files());
    println!("  partial: {}", report.partial_files());
    println!("  total files: {}", report.total_files());
    println!("  hydrated bytes: {}", report.hydrated_bytes());
    println!("  remote-only bytes: {}", report.remote_only_bytes());
    println!(
        "  pinned: {} files, {} bytes",
        report.pinned_files(),
        report.pinned_bytes()
    );
    println!(
        "  evictable: {} files, {} bytes",
        report.evictable_files(),
        report.evictable_bytes()
    );
    println!(
        "  would avoid downloading: {} bytes already local",
        report.hydrated_bytes()
    );
    match remote_metrics {
        RemoteMetrics::Known {
            pending_upload_files,
            pending_upload_bytes,
            ..
        } => println!(
            "  pending uploads: {} files, {} bytes",
            pending_upload_files, pending_upload_bytes
        ),
        RemoteMetrics::Unknown { reason } => println!("  pending uploads: unknown ({reason})"),
    }
    println!("  cache hits/misses: not measured yet");
    Ok(())
}

fn run_cache_free_space(args: &[String], command: &str) -> Result<(), String> {
    let parsed = parse_cache_prune_args(args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let versions =
        tracked_versions_for_scope(&store, Path::new("")).map_err(|error| error.to_string())?;
    let no_remote_proof = BTreeSet::new();
    let current_status = cache_status_for_scope(&store, Path::new(""), &no_remote_proof)
        .map_err(|error| error.to_string())?;
    let remote_available_objects = if current_status.hydrated_bytes() > parsed.max_bytes {
        remote_available_objects_for_versions(&store, &versions, "cache prune")?
    } else {
        no_remote_proof
    };
    let report = prune_cache_to_limit(
        &store,
        Path::new(""),
        parsed.max_bytes,
        &remote_available_objects,
    )
    .map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    if command == "cache prune" {
        println!("Cache limit: {} bytes", report.limit_bytes());
        println!("Intent: free space by evicting only clean, unpinned files with remote proof");
    } else {
        println!("Free-space target: {} hydrated bytes", report.limit_bytes());
        println!("Intent: free space by evicting only clean, unpinned files with remote proof");
    }
    println!(
        "Hydrated bytes: {} -> {}",
        report.hydrated_bytes_before(),
        report.hydrated_bytes_after()
    );
    println!(
        "Evicted: {} files, {} objects; already remote-only: {} files",
        report.evicted_files(),
        report.evicted_objects(),
        report.already_remote_files()
    );
    println!(
        "Skipped: {} pinned, {} dirty, {} unsupported",
        report.skipped_pinned_files(),
        report.skipped_dirty_files(),
        report.skipped_unsupported_files()
    );
    Ok(())
}

fn run_cache_warm(args: &[String]) -> Result<(), String> {
    let parsed = parse_cache_warm_args(args)?;
    let store = open_store_for_path_or_folder(&parsed.target)?;
    let scope = relative_scope_path(&store, &absolute_path_from_path(&parsed.target)?)
        .map_err(|error| error.to_string())?;
    let selection = warm_versions_for_scope(&store, &scope, parsed.max_bytes, parsed.manifest_only)
        .map_err(|error| error.to_string())?;
    let avoided_download_bytes = selection
        .versions()
        .iter()
        .filter_map(|version| {
            let object_id = version.object_id()?;
            store
                .object_cache()
                .exists(object_id)
                .then(|| version.size_bytes().unwrap_or(0))
        })
        .sum::<u64>();
    let fetched_objects = fetch_missing_objects(&store, selection.versions())?;
    let report =
        hydrate_versions(&store, selection.versions()).map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("Warmed: {}", path_to_store_string(&scope));
    println!("Warm limit: {} bytes per file", parsed.max_bytes);
    if parsed.manifest_only {
        println!("Filter: manifest/config files only");
    }
    println!(
        "Selected: {} files ({} manifest/config, {} source, {} other small)",
        selection.selected_files(),
        selection.selected_manifest_files(),
        selection.selected_source_files(),
        selection.selected_small_files()
    );
    println!(
        "Skipped: {} large, {} outside manifest filter",
        selection.skipped_large_files(),
        selection.skipped_non_manifest_files()
    );
    println!("Fetched objects: {fetched_objects}");
    println!("Avoided download bytes: {avoided_download_bytes}");
    println!(
        "Materialized: {} files, {} folders, {} already present",
        report.materialized_files(),
        report.materialized_directories(),
        report.already_materialized_files()
    );
    Ok(())
}

fn run_cache_prefetch(args: &[String]) -> Result<(), String> {
    let parsed = parse_cache_prefetch_args(args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let selection = prefetch_versions_for_scope(&store, Path::new(""), parsed.max_bytes)
        .map_err(|error| error.to_string())?;
    let fetched_objects = fetch_missing_objects(&store, selection.versions())?;
    let report =
        hydrate_versions(&store, selection.versions()).map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    println!("Prefetch limit: {} bytes per file", parsed.max_bytes);
    println!(
        "Selected: {} files; skipped large: {} files",
        selection.selected_files(),
        selection.skipped_large_files()
    );
    println!("Fetched objects: {fetched_objects}");
    println!(
        "Materialized: {} files, {} folders, {} already present",
        report.materialized_files(),
        report.materialized_directories(),
        report.already_materialized_files()
    );
    Ok(())
}

fn run_cache_policy_show() -> Result<(), String> {
    println!("Cache policy presets:");
    println!("  These are internal presets for Loom commands and diagnostics.");
    println!("  Normal use stays intent-based: pin paths, warm paths, free space, check status.");
    for preset in cache_policy_presets() {
        println!(
            "  {}: warm_max_bytes={}, prune_target={}, pins_required={} - {}",
            preset.name(),
            preset.warm_max_bytes(),
            preset
                .prune_target()
                .map(|bytes| bytes.to_string())
                .unwrap_or_else(|| "-".to_string()),
            preset.pins_required(),
            preset.intent()
        );
    }
    Ok(())
}

fn run_workspace(args: &[String]) -> Result<(), String> {
    match args.split_first() {
        Some((subcommand, rest)) if subcommand == "open" => run_workspace_open(rest),
        Some((subcommand, rest)) if subcommand == "sessions" => run_workspace_sessions(rest),
        Some((subcommand, rest)) if subcommand == "list" => run_workspace_list(rest),
        Some((subcommand, rest)) if subcommand == "read" => run_workspace_read(rest),
        Some((subcommand, rest)) if subcommand == "write" => run_workspace_write(rest),
        Some((subcommand, rest)) if subcommand == "hydrate" => run_workspace_hydrate(rest),
        Some((subcommand, rest)) if subcommand == "dehydrate" => run_workspace_dehydrate(rest),
        Some((subcommand, rest)) if subcommand == "pin" => run_workspace_pin(rest),
        Some((subcommand, rest)) if subcommand == "diff" => run_workspace_diff(rest),
        Some((subcommand, rest)) if subcommand == "checkpoint" => run_workspace_checkpoint(rest),
        Some((subcommand, rest)) if subcommand == "close" => run_workspace_close(rest),
        Some((subcommand, rest)) if subcommand == "discard" => run_workspace_discard(rest),
        Some((subcommand, _)) => Err(format!(
            "workspace unknown subcommand '{subcommand}'\nUsage: {}",
            COMMANDS
                .iter()
                .find(|command| command.name == "workspace")
                .expect("workspace command exists")
                .usage
        )),
        None => Err(format!(
            "workspace requires a subcommand\nUsage: {}",
            COMMANDS
                .iter()
                .find(|command| command.name == "workspace")
                .expect("workspace command exists")
                .usage
        )),
    }
}

fn run_workspace_open(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_open_args(args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let base_revision_id = match parsed.revision {
        Some(target) => Some(
            store
                .resolve_revision_target(&target)
                .map_err(|error| error.to_string())?
                .revision()
                .id()
                .clone(),
        ),
        None => None,
    };
    let request = WorkspaceSessionRequest {
        session_id: parsed.session_id,
        base_revision_id,
    };
    let adapter = AgentWorkspaceAdapter::new(store.clone());
    let session = adapter
        .create_session(request)
        .map_err(|error| error.to_string())?;

    println!("Workspace session: {}", session.session().id());
    println!("Folder: {}", store.folder_root().display());
    println!("Adapter: agent virtual");
    println!("Base revision: {}", session.session().base_revision_id());
    println!(
        "Overlay: {}",
        store
            .store_root()
            .join("workspaces")
            .join("sessions")
            .join(session.session().id().as_str())
            .display()
    );
    Ok(())
}

fn run_workspace_sessions(args: &[String]) -> Result<(), String> {
    let folder = parse_workspace_sessions_args(args)?;
    let store = open_store_from_optional_folder(folder)?;
    let adapter = AgentWorkspaceAdapter::new(store.clone());
    let sessions = adapter.list_sessions().map_err(|error| error.to_string())?;

    println!("Folder: {}", store.folder_root().display());
    if sessions.is_empty() {
        println!("Workspace sessions: none");
        return Ok(());
    }
    println!("Workspace sessions:");
    for session in sessions {
        println!(
            "{}  kind={}  state={}  base={}  created={}",
            session.id(),
            workspace_kind_label(session.kind()),
            workspace_state_label(session.state()),
            session.base_revision_id(),
            session.created_at()
        );
    }
    Ok(())
}

fn run_workspace_list(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_path_args("list", args, false)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let session_id = parsed
        .session_id
        .ok_or_else(|| "workspace list requires --session <ID>".to_string())?;
    let scope = parsed.path.unwrap_or_default();
    let adapter = AgentWorkspaceAdapter::new(store.clone());
    let session = adapter
        .open_session(&session_id)
        .map_err(|error| error.to_string())?;
    let entries = session
        .list_metadata(&scope)
        .map_err(|error| error.to_string())?;

    println!("Workspace session: {}", session.session().id());
    println!("Base revision: {}", session.session().base_revision_id());
    println!("Path: {}", path_to_store_string(&scope));
    if entries.is_empty() {
        println!("Entries: none");
        return Ok(());
    }
    println!("Entries:");
    for entry in entries {
        print_workspace_entry(&entry);
    }
    Ok(())
}

fn run_workspace_read(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_path_args("read", args, true)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let session_id = parsed
        .session_id
        .ok_or_else(|| "workspace read requires --session <ID>".to_string())?;
    let path = parsed
        .path
        .ok_or_else(|| "workspace read requires a path".to_string())?;
    let remote = optional_workspace_remote(&store)?;
    let bytes = if let Some(remote) = remote.as_deref() {
        let adapter = AgentWorkspaceAdapter::with_remote(store, remote);
        let session = adapter
            .open_session(&session_id)
            .map_err(|error| error.to_string())?;
        session
            .read_file(&path)
            .map_err(|error| error.to_string())?
    } else {
        let adapter = AgentWorkspaceAdapter::new(store);
        let session = adapter
            .open_session(&session_id)
            .map_err(|error| error.to_string())?;
        session
            .read_file(&path)
            .map_err(|error| error.to_string())?
    };

    print!("{}", String::from_utf8_lossy(&bytes));
    Ok(())
}

fn run_workspace_write(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_write_args(args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let adapter = AgentWorkspaceAdapter::new(store);
    let mut session = adapter
        .open_session(&parsed.session_id)
        .map_err(|error| error.to_string())?;
    session
        .write_file(&parsed.path, parsed.text.as_bytes())
        .map_err(|error| error.to_string())?;

    println!("Workspace session: {}", session.session().id());
    println!("Wrote overlay: {}", path_to_store_string(&parsed.path));
    println!("Bytes: {}", parsed.text.len());
    Ok(())
}

fn run_workspace_hydrate(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_path_args("hydrate", args, true)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let session_id = parsed
        .session_id
        .ok_or_else(|| "workspace hydrate requires --session <ID>".to_string())?;
    let path = parsed
        .path
        .ok_or_else(|| "workspace hydrate requires a path".to_string())?;
    let remote = optional_workspace_remote(&store)?;
    let report = if let Some(remote) = remote.as_deref() {
        let adapter = AgentWorkspaceAdapter::with_remote(store, remote);
        let session = adapter
            .open_session(&session_id)
            .map_err(|error| error.to_string())?;
        session
            .hydrate_path(&path)
            .map_err(|error| error.to_string())?
    } else {
        let adapter = AgentWorkspaceAdapter::new(store);
        let session = adapter
            .open_session(&session_id)
            .map_err(|error| error.to_string())?;
        session
            .hydrate_path(&path)
            .map_err(|error| error.to_string())?
    };

    println!("Workspace session: {}", session_id);
    println!("Hydrated cache for: {}", path_to_store_string(&path));
    println!("Fetched objects: {}", report.fetched_objects());
    println!(
        "Already cached objects: {}",
        report.already_cached_objects()
    );
    println!("Overlay files: {}", report.overlay_files());
    Ok(())
}

fn run_workspace_dehydrate(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_path_args("dehydrate", args, true)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let session_id = parsed
        .session_id
        .ok_or_else(|| "workspace dehydrate requires --session <ID>".to_string())?;
    let path = parsed
        .path
        .ok_or_else(|| "workspace dehydrate requires a path".to_string())?;
    let adapter = AgentWorkspaceAdapter::new(store);
    let session = adapter
        .open_session(&session_id)
        .map_err(|error| error.to_string())?;
    session
        .dehydrate_path(&path)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn run_workspace_pin(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_path_args("pin", args, true)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let session_id = parsed
        .session_id
        .ok_or_else(|| "workspace pin requires --session <ID>".to_string())?;
    let path = parsed
        .path
        .ok_or_else(|| "workspace pin requires a path".to_string())?;
    let adapter = AgentWorkspaceAdapter::new(store);
    let session = adapter
        .open_session(&session_id)
        .map_err(|error| error.to_string())?;
    session
        .pin_path(&path)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn run_workspace_diff(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_path_args("diff", args, false)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let session_id = parsed
        .session_id
        .ok_or_else(|| "workspace diff requires --session <ID>".to_string())?;
    let adapter = AgentWorkspaceAdapter::new(store);
    let session = adapter
        .open_session(&session_id)
        .map_err(|error| error.to_string())?;
    let diff = session.diff_overlay().map_err(|error| error.to_string())?;

    println!("Workspace session: {}", session.session().id());
    println!("Base revision: {}", session.session().base_revision_id());
    print_workspace_diff(&diff);
    Ok(())
}

fn run_workspace_checkpoint(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_checkpoint_args(args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let adapter = AgentWorkspaceAdapter::new(store);
    let mut session = adapter
        .open_session(&parsed.session_id)
        .map_err(|error| error.to_string())?;
    let checkpoint = session
        .checkpoint_overlay(&parsed.message)
        .map_err(|error| error.to_string())?;

    println!("Workspace session: {}", session.session().id());
    println!("Checkpoint: {}", checkpoint.checkpoint().id());
    println!("Revision: {}", checkpoint.checkpoint().revision_id());
    println!("Boundary: sandbox-merge");
    println!("Overlay files: {}", checkpoint.overlay_files());
    println!(
        "Changes: {} created, {} modified, {} deleted, {} unchanged",
        checkpoint.coalesced().diff().created(),
        checkpoint.coalesced().diff().modified(),
        checkpoint.coalesced().diff().deleted(),
        checkpoint.coalesced().diff().unchanged()
    );
    Ok(())
}

fn run_workspace_close(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_session_only_args("close", args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let adapter = AgentWorkspaceAdapter::new(store);
    let session = adapter
        .open_session(&parsed.session_id)
        .map_err(|error| error.to_string())?;
    let report = session.close().map_err(|error| error.to_string())?;

    println!("Workspace session: {}", report.session_id());
    println!("State: closed");
    println!(
        "Discarded overlay files: {}",
        report.discarded_overlay_files()
    );
    Ok(())
}

fn run_workspace_discard(args: &[String]) -> Result<(), String> {
    let parsed = parse_workspace_session_only_args("discard", args)?;
    let store = open_store_from_optional_folder(parsed.folder)?;
    let adapter = AgentWorkspaceAdapter::new(store);
    let session = adapter
        .open_session(&parsed.session_id)
        .map_err(|error| error.to_string())?;
    let report = session.discard().map_err(|error| error.to_string())?;

    println!("Workspace session: {}", report.session_id());
    println!("State: discarded");
    println!(
        "Discarded overlay files: {}",
        report.discarded_overlay_files()
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
    let capture = CaptureEngine::new(store)
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

fn print_workspace_entry(entry: &WorkspaceEntryMetadata) {
    println!(
        "  {}  kind={}  hydration={}  source={}  bytes={}",
        path_to_store_string(entry.path()),
        file_kind_label(entry.kind()),
        hydration_label(entry.hydration_state()),
        workspace_entry_source_label(entry.source()),
        entry
            .size_bytes()
            .map(|bytes| bytes.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
}

fn print_workspace_diff(diff: &WorkspaceOverlayDiff) {
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

fn file_kind_label(kind: &FileKind) -> &'static str {
    match kind {
        FileKind::File => "file",
        FileKind::Directory => "directory",
        FileKind::Symlink => "symlink",
        FileKind::Unsupported => "unsupported",
    }
}

fn hydration_label(state: loom_core::HydrationState) -> &'static str {
    match state {
        loom_core::HydrationState::RemoteOnly => "remote-only",
        loom_core::HydrationState::Partial => "partial",
        loom_core::HydrationState::Hydrated => "hydrated",
    }
}

fn workspace_entry_source_label(source: WorkspaceEntrySource) -> &'static str {
    match source {
        WorkspaceEntrySource::BaseRevision => "base",
        WorkspaceEntrySource::Overlay => "overlay",
    }
}

fn workspace_kind_label(kind: WorkspaceKind) -> &'static str {
    match kind {
        WorkspaceKind::AgentVirtual => "agent-virtual",
        WorkspaceKind::MaterializedSandbox => "materialized-sandbox",
        WorkspaceKind::OsFilesystemMount => "os-filesystem-mount",
    }
}

fn workspace_state_label(state: WorkspaceSessionState) -> &'static str {
    match state {
        WorkspaceSessionState::Open => "open",
        WorkspaceSessionState::Closed => "closed",
        WorkspaceSessionState::Discarded => "discarded",
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

fn print_store_verification_report(report: &StoreVerificationReport) {
    println!("  file versions: {}", report.checked_file_versions());
    println!("  folder revisions: {}", report.checked_revisions());
    println!("  cache entries: {}", report.checked_cache_entries());
    println!("  local objects: {}", report.checked_local_objects());
    println!(
        "  issues: {} errors, {} warnings",
        report.error_count(),
        report.warning_count()
    );
    print_store_issues(report);
}

fn print_store_issues(report: &StoreVerificationReport) {
    for issue in report.issues() {
        let level = match issue.level() {
            VerificationLevel::Error => "ERROR",
            VerificationLevel::Warning => "WARN",
        };
        println!("  {level} {}: {}", issue.code(), issue.message());
    }
}

fn print_remote_check_report(config: &RemoteConfig, report: &RemoteCheckReport) {
    println!("Remote: {}", config.name());
    println!("  kind: {}", config.kind());
    println!("  location: {}", config.location());
    println!(
        "  cursor: {}",
        report
            .remote_cursor
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "-".to_string())
    );
    println!("  cursor known locally: {}", report.cursor_known_locally);
    println!("  cursor pack present: {}", report.cursor_pack_present);
    println!(
        "  cursor pack matches cursor: {}",
        report.cursor_pack_matches_cursor
    );
    println!("  checked objects: {}", report.checked_objects);
    println!("  missing objects: {}", report.missing_objects.len());
    for object_id in report.missing_objects.iter().take(20) {
        println!("  ERROR missing-remote-object: object {object_id} is not available remotely");
    }
    if !report.cursor_pack_present {
        println!("  ERROR missing-remote-pack: remote cursor pack is not readable");
    }
    if !report.cursor_pack_matches_cursor {
        println!(
            "  ERROR remote-cursor-pack-mismatch: remote cursor does not match decoded pack manifest"
        );
    }
    if !report.cursor_known_locally {
        println!("  ERROR remote-cursor-unknown-locally: remote cursor is not in local history");
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

fn required_path_arg(command: &str, args: &[String]) -> Result<PathBuf, String> {
    match args {
        [path] => Ok(PathBuf::from(path)),
        [] => Err(format!(
            "{command} requires a path or folder\nUsage: loom {command} <PATH|FOLDER>"
        )),
        _ => Err(format!("{command} accepts exactly one path or folder")),
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
            return LocalStore::open(&current).map_err(|error| error.to_string());
        }
        if !current.pop() {
            return Err(format!(
                "{} is not inside a tracked Loom folder",
                absolute.display()
            ));
        }
    }
}

fn preferred_remote(store: &LocalStore) -> Result<RemoteConfig, String> {
    store
        .remote(DEFAULT_REMOTE_NAME)
        .map_err(|error| error.to_string())?
        .or_else(|| store.remotes().ok().and_then(|mut remotes| remotes.pop()))
        .ok_or_else(|| {
            "no Loom remote configured; sparse hydration needs a remote with object bytes"
                .to_string()
        })
}

fn optional_workspace_remote(store: &LocalStore) -> Result<Option<Box<dyn LoomRemote>>, String> {
    match preferred_remote(store) {
        Ok(config) => remote_from_config(&config).map(Some),
        Err(_) => Ok(None),
    }
}

fn preferred_remote_for_check(store: &LocalStore) -> Result<RemoteConfig, String> {
    store
        .remote(DEFAULT_REMOTE_NAME)
        .map_err(|error| error.to_string())?
        .or_else(|| store.remotes().ok().and_then(|mut remotes| remotes.pop()))
        .ok_or_else(|| "no Loom remote configured; run 'loom remote add' first".to_string())
}

fn fetch_missing_objects(store: &LocalStore, versions: &[FileVersion]) -> Result<usize, String> {
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

    let remote_config = preferred_remote(store)?;
    let remote = remote_from_config(&remote_config)?;
    let mut fetched_objects = 0;
    for (object_id, size_bytes) in missing {
        if hydrate_object_from_remote(store, remote.as_ref(), &object_id, size_bytes)
            .map_err(|error| error.to_string())?
        {
            fetched_objects += 1;
        }
    }
    Ok(fetched_objects)
}

fn remote_available_objects_for_versions(
    store: &LocalStore,
    versions: &[FileVersion],
    command: &str,
) -> Result<BTreeSet<loom_core::ObjectId>, String> {
    let object_ids = unique_file_object_ids(versions);
    if object_ids.is_empty() {
        return Ok(BTreeSet::new());
    }

    let remote_config = preferred_remote_for_eviction(store, command)?;
    let remote = remote_from_config(&remote_config)?;
    let mut available = BTreeSet::new();
    for object_id in object_ids {
        let exists = remote.has_object(&object_id).map_err(|error| {
            format!(
                "{command} refused because remote object availability could not be proven: {error}"
            )
        })?;
        if !exists {
            return Err(format!(
                "{command} refused because object {object_id} is not available on remote {}; run 'loom sync' first",
                remote_config.name()
            ));
        }
        available.insert(object_id);
    }

    Ok(available)
}

fn remote_metrics_for_versions(store: &LocalStore, versions: &[FileVersion]) -> RemoteMetrics {
    let remote_config = match preferred_remote(store) {
        Ok(remote_config) => remote_config,
        Err(_) => {
            return RemoteMetrics::Unknown {
                reason: "no remote configured".to_string(),
            }
        }
    };
    let remote = match remote_from_config(&remote_config) {
        Ok(remote) => remote,
        Err(error) => return RemoteMetrics::Unknown { reason: error },
    };

    let mut known_remote_objects = BTreeSet::new();
    let mut remote_checks = BTreeSet::new();
    for object_id in unique_file_object_ids(versions) {
        match remote.has_object(&object_id) {
            Ok(true) => {
                known_remote_objects.insert(object_id.clone());
                remote_checks.insert(object_id);
            }
            Ok(false) => {
                remote_checks.insert(object_id);
            }
            Err(error) => {
                return RemoteMetrics::Unknown {
                    reason: format!(
                        "remote {} object check failed: {error}",
                        remote_config.name()
                    ),
                }
            }
        }
    }

    let mut pending_upload_files = 0;
    let mut pending_upload_bytes = 0;
    for version in versions {
        if version.kind() != &FileKind::File {
            continue;
        }
        let Some(object_id) = version.object_id() else {
            continue;
        };
        if remote_checks.contains(object_id) && !known_remote_objects.contains(object_id) {
            pending_upload_files += 1;
            pending_upload_bytes += version.size_bytes().unwrap_or(0);
        }
    }

    RemoteMetrics::Known {
        available_objects: known_remote_objects,
        pending_upload_files,
        pending_upload_bytes,
    }
}

fn unique_file_object_ids(versions: &[FileVersion]) -> BTreeSet<loom_core::ObjectId> {
    versions
        .iter()
        .filter(|version| version.kind() == &FileKind::File)
        .filter_map(|version| version.object_id().cloned())
        .collect()
}

fn preferred_remote_for_eviction(
    store: &LocalStore,
    command: &str,
) -> Result<RemoteConfig, String> {
    preferred_remote(store).map_err(|_| {
        format!(
            "{command} refused because no Loom remote is configured; run 'loom remote add' and 'loom sync' before evicting local bytes"
        )
    })
}

fn local_pinned_scopes(store: &LocalStore) -> Result<Vec<PathBuf>, String> {
    let mut scopes = Vec::new();
    for pin in store.pins().map_err(|error| error.to_string())? {
        let Some(path) = pin.reason().strip_prefix("materialization-pin path=") else {
            continue;
        };
        scopes.push(if path == "." {
            PathBuf::new()
        } else {
            path.split('/').collect()
        });
    }
    Ok(scopes)
}

fn absolute_path(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    absolute_path_from_path(&path)
}

fn absolute_path_from_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(std::env::current_dir()
        .map_err(|error| error.to_string())?
        .join(path))
}

fn looks_like_devbox_api_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn remote_from_config(config: &RemoteConfig) -> Result<Box<dyn LoomRemote>, String> {
    match config.kind() {
        LOCAL_FILESYSTEM_REMOTE_KIND => Ok(Box::new(LocalFilesystemRemote::new(config.location()))),
        DEVBOX_HOSTED_REMOTE_KIND => {
            let config = DevboxHostedRemoteConfig::from_clone_url(config.location())
                .map_err(|error| error.to_string())?;
            Ok(Box::new(DevboxHostedRemote::new(config)))
        }
        kind => Err(format!(
            "remote {} uses unsupported kind {}",
            config.name(),
            kind
        )),
    }
}

fn clone_remote_from_location(
    location: &str,
) -> Result<(Box<dyn LoomRemote>, &'static str, String), String> {
    if location.starts_with("devbox://") {
        let config = DevboxHostedRemoteConfig::from_clone_url(location)
            .map_err(|error| error.to_string())?;
        let stored_location = config.clone_url();
        return Ok((
            Box::new(DevboxHostedRemote::new(config)),
            DEVBOX_HOSTED_REMOTE_KIND,
            stored_location,
        ));
    }

    let remote_path = absolute_path_from_path(Path::new(location))?;
    let stored_location = remote_path.to_string_lossy().into_owned();
    Ok((
        Box::new(LocalFilesystemRemote::new(remote_path)),
        LOCAL_FILESYSTEM_REMOTE_KIND,
        stored_location,
    ))
}

#[derive(Debug, Clone)]
struct CloneArgs {
    remote_location: String,
    target: PathBuf,
    sparse: bool,
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

#[derive(Debug, Clone)]
struct SyncDaemonArgs {
    folder: Option<PathBuf>,
    debounce_ms: u64,
    poll_ms: u64,
    max_cycles: Option<usize>,
}

#[derive(Debug, Clone)]
struct CachePruneArgs {
    folder: Option<PathBuf>,
    max_bytes: u64,
}

#[derive(Debug, Clone)]
struct CacheWarmArgs {
    target: PathBuf,
    max_bytes: u64,
    manifest_only: bool,
}

#[derive(Debug, Clone)]
struct CachePrefetchArgs {
    folder: Option<PathBuf>,
    max_bytes: u64,
}

#[derive(Debug, Clone)]
struct WorkspaceOpenArgs {
    folder: Option<PathBuf>,
    session_id: Option<WorkspaceSessionId>,
    revision: Option<String>,
}

#[derive(Debug, Clone)]
struct WorkspacePathArgs {
    folder: Option<PathBuf>,
    session_id: Option<WorkspaceSessionId>,
    path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct WorkspaceWriteArgs {
    folder: Option<PathBuf>,
    session_id: WorkspaceSessionId,
    path: PathBuf,
    text: String,
}

#[derive(Debug, Clone)]
struct WorkspaceCheckpointArgs {
    folder: Option<PathBuf>,
    session_id: WorkspaceSessionId,
    message: String,
}

#[derive(Debug, Clone)]
struct WorkspaceSessionOnlyArgs {
    folder: Option<PathBuf>,
    session_id: WorkspaceSessionId,
}

#[derive(Debug, Clone)]
enum RemoteMetrics {
    Known {
        available_objects: BTreeSet<loom_core::ObjectId>,
        pending_upload_files: usize,
        pending_upload_bytes: u64,
    },
    Unknown {
        reason: String,
    },
}

fn parse_clone_args(args: &[String]) -> Result<CloneArgs, String> {
    let mut sparse = false;
    let mut positional = Vec::new();

    for arg in args {
        if arg == "--sparse" {
            sparse = true;
        } else if arg.starts_with('-') {
            return Err(format!("clone unknown option '{arg}'"));
        } else {
            positional.push(arg.clone());
        }
    }

    match positional.as_slice() {
        [remote_location, target] => Ok(CloneArgs {
            remote_location: remote_location.clone(),
            target: PathBuf::from(target),
            sparse,
        }),
        _ => Err("clone requires a remote path and target folder\nUsage: loom clone <REMOTE> <FOLDER> [--sparse]".to_string()),
    }
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

fn parse_sync_daemon_args(args: &[String], command: &str) -> Result<SyncDaemonArgs, String> {
    let mut folder = None;
    let mut debounce_ms = loom_daemon::DEFAULT_DEBOUNCE_MS;
    let mut poll_ms = loom_daemon::DEFAULT_POLL_MS;
    let mut max_cycles = None;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--debounce-ms" => {
                index += 1;
                debounce_ms = parse_u64_flag(
                    "--debounce-ms",
                    args.get(index)
                        .ok_or_else(|| "--debounce-ms requires a value".to_string())?,
                )?;
            }
            "--poll-ms" => {
                index += 1;
                poll_ms = parse_u64_flag(
                    "--poll-ms",
                    args.get(index)
                        .ok_or_else(|| "--poll-ms requires a value".to_string())?,
                )?;
            }
            "--max-cycles" if command == "run-loop" => {
                index += 1;
                max_cycles = Some(parse_usize_flag(
                    "--max-cycles",
                    args.get(index)
                        .ok_or_else(|| "--max-cycles requires a value".to_string())?,
                )?);
            }
            value if value.starts_with('-') => {
                return Err(format!("sync {command} unknown option '{value}'"));
            }
            value => {
                if folder.replace(PathBuf::from(value)).is_some() {
                    return Err(format!("sync {command} accepts at most one folder"));
                }
            }
        }

        index += 1;
    }

    Ok(SyncDaemonArgs {
        folder,
        debounce_ms,
        poll_ms,
        max_cycles,
    })
}

fn parse_cache_prune_args(args: &[String]) -> Result<CachePruneArgs, String> {
    let mut folder = None;
    let mut max_bytes = None;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--max-bytes" {
            index += 1;
            max_bytes = Some(parse_u64_flag(
                "--max-bytes",
                args.get(index)
                    .ok_or_else(|| "--max-bytes requires a value".to_string())?,
            )?);
        } else if let Some(value) = arg.strip_prefix("--max-bytes=") {
            max_bytes = Some(parse_u64_flag("--max-bytes", value)?);
        } else if arg.starts_with('-') {
            return Err(format!("cache prune unknown option '{arg}'"));
        } else if folder.replace(PathBuf::from(arg)).is_some() {
            return Err("cache prune accepts at most one folder".to_string());
        }
        index += 1;
    }

    let max_bytes = max_bytes.ok_or_else(|| {
        "cache prune requires --max-bytes <BYTES>\nUsage: loom cache prune --max-bytes <BYTES> [FOLDER]"
            .to_string()
    })?;

    Ok(CachePruneArgs { folder, max_bytes })
}

fn parse_cache_warm_args(args: &[String]) -> Result<CacheWarmArgs, String> {
    let mut target = None;
    let mut max_bytes = DEFAULT_WARM_MAX_BYTES;
    let mut manifest_only = false;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--max-bytes" {
            index += 1;
            max_bytes = parse_u64_flag(
                "--max-bytes",
                args.get(index)
                    .ok_or_else(|| "--max-bytes requires a value".to_string())?,
            )?;
        } else if let Some(value) = arg.strip_prefix("--max-bytes=") {
            max_bytes = parse_u64_flag("--max-bytes", value)?;
        } else if arg == "--manifest" {
            manifest_only = true;
        } else if arg.starts_with('-') {
            return Err(format!("cache warm unknown option '{arg}'"));
        } else if target.replace(PathBuf::from(arg)).is_some() {
            return Err("cache warm accepts one path or folder".to_string());
        }
        index += 1;
    }

    let target = target.ok_or_else(|| {
        "cache warm requires a path or folder\nUsage: loom cache warm <PATH|FOLDER> [--manifest] [--max-bytes <BYTES>]".to_string()
    })?;

    Ok(CacheWarmArgs {
        target,
        max_bytes,
        manifest_only,
    })
}

fn parse_cache_prefetch_args(args: &[String]) -> Result<CachePrefetchArgs, String> {
    let mut folder = None;
    let mut max_bytes = DEFAULT_PREFETCH_MAX_BYTES;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--max-bytes" {
            index += 1;
            max_bytes = parse_u64_flag(
                "--max-bytes",
                args.get(index)
                    .ok_or_else(|| "--max-bytes requires a value".to_string())?,
            )?;
        } else if let Some(value) = arg.strip_prefix("--max-bytes=") {
            max_bytes = parse_u64_flag("--max-bytes", value)?;
        } else if arg.starts_with('-') {
            return Err(format!("cache prefetch unknown option '{arg}'"));
        } else if folder.replace(PathBuf::from(arg)).is_some() {
            return Err("cache prefetch accepts at most one folder".to_string());
        }
        index += 1;
    }

    Ok(CachePrefetchArgs { folder, max_bytes })
}

fn parse_workspace_open_args(args: &[String]) -> Result<WorkspaceOpenArgs, String> {
    let mut folder = None;
    let mut session_id = None;
    let mut revision = None;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--session" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "--session requires a value".to_string())?;
            session_id =
                Some(WorkspaceSessionId::new(value.clone()).map_err(|error| error.to_string())?);
        } else if let Some(value) = arg.strip_prefix("--session=") {
            session_id = Some(
                WorkspaceSessionId::new(value.to_string()).map_err(|error| error.to_string())?,
            );
        } else if arg == "--revision" {
            index += 1;
            revision = Some(
                args.get(index)
                    .ok_or_else(|| "--revision requires a value".to_string())?
                    .clone(),
            );
        } else if let Some(value) = arg.strip_prefix("--revision=") {
            revision = Some(value.to_string());
        } else if arg.starts_with('-') {
            return Err(format!("workspace open unknown option '{arg}'"));
        } else if folder.replace(PathBuf::from(arg)).is_some() {
            return Err("workspace open accepts at most one folder".to_string());
        }
        index += 1;
    }

    Ok(WorkspaceOpenArgs {
        folder,
        session_id,
        revision,
    })
}

fn parse_workspace_sessions_args(args: &[String]) -> Result<Option<PathBuf>, String> {
    match args {
        [] => Ok(None),
        [folder] => Ok(Some(PathBuf::from(folder))),
        _ => Err("workspace sessions accepts at most one folder".to_string()),
    }
}

fn parse_workspace_path_args(
    command: &str,
    args: &[String],
    path_required: bool,
) -> Result<WorkspacePathArgs, String> {
    let mut session_id = None;
    let mut positional = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--session" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "--session requires a value".to_string())?;
            session_id =
                Some(WorkspaceSessionId::new(value.clone()).map_err(|error| error.to_string())?);
        } else if let Some(value) = arg.strip_prefix("--session=") {
            session_id = Some(
                WorkspaceSessionId::new(value.to_string()).map_err(|error| error.to_string())?,
            );
        } else if arg.starts_with('-') {
            return Err(format!("workspace {command} unknown option '{arg}'"));
        } else {
            positional.push(arg.clone());
        }
        index += 1;
    }

    let (folder, path) = if path_required {
        match positional.as_slice() {
            [path] => (None, Some(PathBuf::from(path))),
            [folder, path] => (Some(PathBuf::from(folder)), Some(PathBuf::from(path))),
            [] => {
                return Err(format!(
                    "workspace {command} requires a path\nUsage: loom workspace {command} [FOLDER] --session <ID> <PATH>"
                ));
            }
            _ => {
                return Err(format!(
                    "workspace {command} accepts an optional folder and one path"
                ));
            }
        }
    } else {
        match positional.as_slice() {
            [] => (None, None),
            [single] if looks_like_tracked_folder(single) => (Some(PathBuf::from(single)), None),
            [path] => (None, Some(PathBuf::from(path))),
            [folder, path] => (Some(PathBuf::from(folder)), Some(PathBuf::from(path))),
            _ => {
                return Err(format!(
                    "workspace {command} accepts an optional folder and optional path"
                ));
            }
        }
    };

    Ok(WorkspacePathArgs {
        folder,
        session_id,
        path,
    })
}

fn parse_workspace_write_args(args: &[String]) -> Result<WorkspaceWriteArgs, String> {
    let mut session_id = None;
    let mut text = None;
    let mut positional = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--session" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "--session requires a value".to_string())?;
            session_id =
                Some(WorkspaceSessionId::new(value.clone()).map_err(|error| error.to_string())?);
        } else if let Some(value) = arg.strip_prefix("--session=") {
            session_id = Some(
                WorkspaceSessionId::new(value.to_string()).map_err(|error| error.to_string())?,
            );
        } else if arg == "--text" {
            index += 1;
            text = Some(
                args.get(index)
                    .ok_or_else(|| "--text requires a value".to_string())?
                    .clone(),
            );
        } else if let Some(value) = arg.strip_prefix("--text=") {
            text = Some(value.to_string());
        } else if arg.starts_with('-') {
            return Err(format!("workspace write unknown option '{arg}'"));
        } else {
            positional.push(arg.clone());
        }
        index += 1;
    }

    let session_id =
        session_id.ok_or_else(|| "workspace write requires --session <ID>".to_string())?;
    let (folder, path, text) = match (text, positional.as_slice()) {
        (Some(text), [path]) => (None, PathBuf::from(path), text),
        (Some(text), [folder, path]) => (Some(PathBuf::from(folder)), PathBuf::from(path), text),
        (None, [path, text]) => (None, PathBuf::from(path), text.clone()),
        (None, [folder, path, text]) => (
            Some(PathBuf::from(folder)),
            PathBuf::from(path),
            text.clone(),
        ),
        _ => {
            return Err(
                "workspace write requires [FOLDER] --session <ID> <PATH> --text <TEXT>".to_string(),
            );
        }
    };

    Ok(WorkspaceWriteArgs {
        folder,
        session_id,
        path,
        text,
    })
}

fn parse_workspace_checkpoint_args(args: &[String]) -> Result<WorkspaceCheckpointArgs, String> {
    let mut session_id = None;
    let mut message = None;
    let mut positional = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--session" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "--session requires a value".to_string())?;
            session_id =
                Some(WorkspaceSessionId::new(value.clone()).map_err(|error| error.to_string())?);
        } else if let Some(value) = arg.strip_prefix("--session=") {
            session_id = Some(
                WorkspaceSessionId::new(value.to_string()).map_err(|error| error.to_string())?,
            );
        } else if arg == "-m" || arg == "--message" {
            index += 1;
            message = Some(
                args.get(index)
                    .ok_or_else(|| "workspace checkpoint requires a message after -m".to_string())?
                    .clone(),
            );
        } else if let Some(value) = arg.strip_prefix("--message=") {
            message = Some(value.to_string());
        } else if arg.starts_with('-') {
            return Err(format!("workspace checkpoint unknown option '{arg}'"));
        } else {
            positional.push(arg.clone());
        }
        index += 1;
    }

    let folder = match positional.as_slice() {
        [] => None,
        [folder] => Some(PathBuf::from(folder)),
        _ => return Err("workspace checkpoint accepts at most one folder".to_string()),
    };
    let session_id =
        session_id.ok_or_else(|| "workspace checkpoint requires --session <ID>".to_string())?;
    let message = message.ok_or_else(|| {
        "workspace checkpoint requires a message\nUsage: loom workspace checkpoint [FOLDER] --session <ID> -m <MESSAGE>".to_string()
    })?;
    if message.trim().is_empty() {
        return Err("workspace checkpoint message cannot be empty".to_string());
    }

    Ok(WorkspaceCheckpointArgs {
        folder,
        session_id,
        message,
    })
}

fn parse_workspace_session_only_args(
    command: &str,
    args: &[String],
) -> Result<WorkspaceSessionOnlyArgs, String> {
    let parsed = parse_workspace_path_args(command, args, false)?;
    if parsed.path.is_some() {
        return Err(format!("workspace {command} does not accept a path"));
    }
    let session_id = parsed
        .session_id
        .ok_or_else(|| format!("workspace {command} requires --session <ID>"))?;
    Ok(WorkspaceSessionOnlyArgs {
        folder: parsed.folder,
        session_id,
    })
}

fn parse_optional_folder(args: &[String], command: &str) -> Result<Option<PathBuf>, String> {
    match args {
        [] => Ok(None),
        [folder] => Ok(Some(PathBuf::from(folder))),
        _ => Err(format!("sync {command} accepts at most one folder")),
    }
}

fn parse_u64_flag(flag: &str, value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn parse_usize_flag(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} requires a non-negative integer"))
}

fn looks_like_folder_arg(value: &str) -> bool {
    Path::new(value).is_dir()
}

fn looks_like_tracked_folder(value: &str) -> bool {
    Path::new(value)
        .join(".loom")
        .join("metadata")
        .join("shared_folder.tsv")
        .is_file()
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

fn print_daemon_status(status: &loom_daemon::DaemonStatus) {
    println!("Daemon state: {}", status.state);
    println!(
        "Daemon pid: {}",
        status
            .pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "-".to_string())
    );
    println!("Remote: {}", status.remote_name.as_deref().unwrap_or("-"));
    println!(
        "Remote location: {}",
        status.remote_location.as_deref().unwrap_or("-")
    );
    println!(
        "Local revision: {}",
        status.last_local_revision.as_deref().unwrap_or("-")
    );
    println!(
        "Remote revision: {}",
        status.last_remote_revision.as_deref().unwrap_or("-")
    );
    println!("Cycles: {}", status.cycles);
    println!("Stop requested: {}", status.stop_requested);
    println!(
        "Last error: {}",
        status.last_error.as_deref().unwrap_or("-")
    );
    println!("Updated: {}", status.updated_at);
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
                "doctor",
                "fsck",
                "object",
                "track",
                "status",
                "history",
                "diff",
                "checkpoint",
                "restore",
                "remote",
                "sync",
                "clone",
                "hydrate",
                "evict",
                "pin",
                "cache",
                "workspace"
            ]
        );
    }
}
