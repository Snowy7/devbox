use devbox_core::scanner::ProjectScanner;
use devbox_core::PolicyDecision;
use devbox_snapshot::SnapshotManifestBuilder;
use devbox_store::BlobCache;
use devbox_store::Store;
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
        Some("status") => run_status(&args[1..]),
        Some("snapshot") => run_snapshot(&args[1..]),
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

fn run_snapshot(args: &[String]) -> ExitCode {
    match args {
        [cache_flag, cache_root, dry_run_flag, path]
            if cache_flag == "--cache" && dry_run_flag == "--dry-run" =>
        {
            match snapshot_dry_run(cache_root, path) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("devbox: {error}");
                    ExitCode::from(1)
                }
            }
        }
        _ => {
            eprintln!("devbox: snapshot currently supports only dry-run manifest creation");
            eprintln!("Usage: devbox snapshot --cache <CACHE_ROOT> --dry-run <PATH>");
            ExitCode::from(2)
        }
    }
}

fn snapshot_dry_run(cache_root: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
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
    println!("  snapshot   Build a dry-run snapshot manifest and local blob-cache objects");
    println!("  status     Placeholder status, or inspect local metadata with --db <PATH>");
    println!("  restore    Placeholder for snapshot restore");
    println!("  explain    Placeholder for policy and sync explanations");
    println!();
    println!("Options:");
    println!("  -h, --help     Print help");
    println!("  -V, --version  Print version");
}
