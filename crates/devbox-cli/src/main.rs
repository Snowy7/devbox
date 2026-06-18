use devbox_core::scanner::ProjectScanner;
use devbox_core::PolicyDecision;
use devbox_snapshot::SnapshotManifestBuilder;
use devbox_store::{
    local_project_id, path_to_store_string, BlobCache, ManifestEntryRecord, NewProject,
    NewSnapshot, NewSnapshotDraft, NewSnapshotManifestEntry, PersistedSnapshot, Store,
};
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
    match args.first().map(String::as_str) {
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
    match entry.kind {
        devbox_core::ManifestEntryKind::File => "file",
        devbox_core::ManifestEntryKind::Directory => "directory",
        devbox_core::ManifestEntryKind::Symlink => "symlink",
        devbox_core::ManifestEntryKind::Unsupported => "unsupported",
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
}

fn open_existing_metadata_store(db_path: &str) -> Result<Store, Box<dyn std::error::Error>> {
    let path = Path::new(db_path);
    if !path.is_file() {
        return Err(format!("metadata database does not exist: {}", path.display()).into());
    }

    Ok(Store::open_file(path)?)
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
    DatabaseInsideSnapshotRoot {
        db_path: PathBuf,
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
            Self::DatabaseInsideSnapshotRoot {
                db_path,
                snapshot_root,
            } => write!(
                f,
                "metadata database path {} is inside snapshot root {}; choose a database outside the project",
                db_path.display(),
                snapshot_root.display()
            ),
        }
    }
}

impl std::error::Error for SnapshotCliPreflightError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::CacheInsideSnapshotRoot { .. } | Self::DatabaseInsideSnapshotRoot { .. } => None,
        }
    }
}

fn preflight_cache_root(
    cache_root: &Path,
    snapshot_root: &Path,
) -> Result<(), SnapshotCliPreflightError> {
    let snapshot_root = canonicalize_snapshot_root(snapshot_root)?;
    let cache_root = resolve_without_creating(cache_root)?;

    if cache_root == snapshot_root || cache_root.starts_with(&snapshot_root) {
        return Err(SnapshotCliPreflightError::CacheInsideSnapshotRoot {
            cache_root,
            snapshot_root,
        });
    }

    Ok(())
}

fn preflight_db_path(
    db_path: &Path,
    snapshot_root: &Path,
) -> Result<(), SnapshotCliPreflightError> {
    let snapshot_root = canonicalize_snapshot_root(snapshot_root)?;
    let db_path = resolve_without_creating(db_path)?;

    if db_path == snapshot_root || db_path.starts_with(&snapshot_root) {
        return Err(SnapshotCliPreflightError::DatabaseInsideSnapshotRoot {
            db_path,
            snapshot_root,
        });
    }

    Ok(())
}

fn canonicalize_snapshot_root(snapshot_root: &Path) -> Result<PathBuf, SnapshotCliPreflightError> {
    fs::canonicalize(snapshot_root).map_err(|source| SnapshotCliPreflightError::Io {
        path: snapshot_root.to_path_buf(),
        source,
    })
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
    println!("  snapshot   Build, persist, list, and show local snapshot manifests");
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

    #[test]
    fn preflight_rejects_in_tree_db_without_creating_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join("project");
        fs::create_dir_all(&root).expect("project dir creates");
        let db_path = root.join("devbox.sqlite3");

        let error = preflight_db_path(&db_path, &root).expect_err("in-tree db path is rejected");

        assert!(matches!(
            error,
            SnapshotCliPreflightError::DatabaseInsideSnapshotRoot { .. }
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
