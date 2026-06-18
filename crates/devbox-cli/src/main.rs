use devbox_core::scanner::ProjectScanner;
use devbox_core::PolicyDecision;
use devbox_snapshot::SnapshotManifestBuilder;
use devbox_store::BlobCache;
use devbox_store::Store;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
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

#[derive(Debug)]
enum SnapshotCliPreflightError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    CacheInsideSnapshotRoot {
        cache_root: PathBuf,
        snapshot_root: PathBuf,
    },
}

impl fmt::Display for SnapshotCliPreflightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "could not validate {}: {source}", path.display())
            }
            Self::CacheInsideSnapshotRoot {
                cache_root,
                snapshot_root,
            } => write!(
                f,
                "blob cache root {} is inside snapshot root {}; choose a cache outside the project",
                cache_root.display(),
                snapshot_root.display()
            ),
        }
    }
}

impl std::error::Error for SnapshotCliPreflightError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::CacheInsideSnapshotRoot { .. } => None,
        }
    }
}

fn preflight_cache_root(
    cache_root: &Path,
    snapshot_root: &Path,
) -> Result<(), SnapshotCliPreflightError> {
    let snapshot_root =
        fs::canonicalize(snapshot_root).map_err(|source| SnapshotCliPreflightError::Io {
            path: snapshot_root.to_path_buf(),
            source,
        })?;
    let cache_root = resolve_without_creating(cache_root)?;

    if cache_root == snapshot_root || cache_root.starts_with(&snapshot_root) {
        return Err(SnapshotCliPreflightError::CacheInsideSnapshotRoot {
            cache_root,
            snapshot_root,
        });
    }

    Ok(())
}

fn resolve_without_creating(path: &Path) -> Result<PathBuf, SnapshotCliPreflightError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| SnapshotCliPreflightError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .join(path)
    };
    let absolute = lexical_normalize(&absolute);

    if absolute.exists() {
        return fs::canonicalize(&absolute).map_err(|source| SnapshotCliPreflightError::Io {
            path: absolute,
            source,
        });
    }

    let mut existing = absolute.clone();
    let mut missing = Vec::<OsString>::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            break;
        };
        missing.push(name.to_os_string());
        if !existing.pop() {
            break;
        }
    }

    let mut resolved =
        fs::canonicalize(&existing).map_err(|source| SnapshotCliPreflightError::Io {
            path: absolute,
            source,
        })?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }

    Ok(resolved)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
        }
    }
    normalized
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

#[cfg(test)]
mod tests {
    use super::*;

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
            SnapshotCliPreflightError::CacheInsideSnapshotRoot { .. }
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
}
