use loom_core::RevisionBoundary;
use loom_store::{path_to_store_string, revision_boundary_to_store, CoalescedRevision, LocalStore};
use loom_worktree::{CaptureEngine, CaptureRequest, WorktreeCapture};
use std::path::PathBuf;
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
        name: "checkpoint",
        usage: "loom checkpoint [FOLDER] -m <MESSAGE>",
        summary: "Name the current folder revision",
        implemented: false,
        planned_behavior: "attach a human message to a durable folder revision",
    },
    CommandSpec {
        name: "restore",
        usage: "loom restore <REVISION>",
        summary: "Restore a folder revision",
        implemented: false,
        planned_behavior: "materialize a previous folder revision with safety checks",
    },
    CommandSpec {
        name: "sync",
        usage: "loom sync [FOLDER]",
        summary: "Synchronize a shared folder",
        implemented: false,
        planned_behavior: "reconcile local and remote folder revisions through Loom cursors",
    },
    CommandSpec {
        name: "clone",
        usage: "loom clone <REMOTE> [FOLDER]",
        summary: "Materialize a shared folder on this machine",
        implemented: false,
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

    Ok(())
}

fn capture_and_coalesce(
    store: &LocalStore,
) -> Result<(WorktreeCapture, CoalescedRevision), String> {
    let request = CaptureRequest::new(store.shared_folder().clone(), RevisionBoundary::LoomCommand);
    let capture = CaptureEngine::new(store.object_cache())
        .capture(&request)
        .map_err(|error| error.to_string())?;
    let coalesced = store
        .coalesce_folder_revision(RevisionBoundary::LoomCommand, capture.file_versions())
        .map_err(|error| error.to_string())?;

    Ok((capture, coalesced))
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
                "checkpoint",
                "restore",
                "sync",
                "clone"
            ]
        );
    }
}
