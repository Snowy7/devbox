use std::fs;
use std::process::{Command, Output};

#[test]
fn snapshot_create_list_and_show_smoke() {
    let fixture = SnapshotCliFixture::new();
    fixture.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    );
    fixture.write("src/main.rs", "fn main() {}\n");
    fixture.write("node_modules/left-pad/index.js", "module.exports = true;\n");

    let create = run_devbox([
        "snapshot",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        fixture.project_path(),
    ]);
    assert_success(&create);

    let create_stdout = stdout(&create);
    assert!(create_stdout.contains("Snapshot id: "));
    assert!(create_stdout.contains("Project name: project"));
    assert!(create_stdout.contains("Policy exclusions: 1"));
    assert!(create_stdout.contains("SQLite database: "));
    assert!(create_stdout.contains("Blob cache: "));

    let snapshot_id = create_stdout
        .lines()
        .find_map(|line| line.strip_prefix("Snapshot id: "))
        .expect("snapshot id prints");

    let list = run_devbox(["snapshot", "list", "--db", fixture.db_path()]);
    assert_success(&list);
    let list_stdout = stdout(&list);
    assert!(list_stdout.contains("Snapshot id\tCreated at\tProject\tEntries\tBytes"));
    assert!(list_stdout.contains(snapshot_id));
    assert!(list_stdout.contains(fixture.project_path()));

    let show = run_devbox(["snapshot", "show", "--db", fixture.db_path(), snapshot_id]);
    assert_success(&show);
    let show_stdout = stdout(&show);
    assert!(show_stdout.contains("Path\tKind\tDecision\tBytes\tBlob id\tObject ref\tReason"));
    assert!(show_stdout.contains("src/main.rs\tfile\tinclude"));
    assert!(show_stdout.contains("node_modules\tdirectory\texclude"));
    assert!(show_stdout.contains("blobs/b3/"));

    let restore_dry_run = run_devbox([
        "snapshot",
        "restore",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        "--to",
        fixture.target_path(),
        snapshot_id,
        "--dry-run",
    ]);
    assert_success(&restore_dry_run);
    let restore_dry_run_stdout = stdout(&restore_dry_run);
    assert!(restore_dry_run_stdout.contains("Restore mode: dry-run"));
    assert!(restore_dry_run_stdout.contains("Apply allowed: true"));
    assert!(restore_dry_run_stdout.contains("FILE\tsrc/main.rs"));
    assert!(restore_dry_run_stdout.contains("SKIP\tnode_modules\tdirectory\texclude"));
    assert!(!fixture.target_path_buf().exists());

    let restore_apply = run_devbox([
        "snapshot",
        "restore",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        "--to",
        fixture.target_path(),
        snapshot_id,
        "--apply",
    ]);
    assert_success(&restore_apply);
    let restore_apply_stdout = stdout(&restore_apply);
    assert!(restore_apply_stdout.contains("Restore mode: apply"));
    assert!(restore_apply_stdout.contains("Files written: 2"));
    assert!(restore_apply_stdout.contains("Skipped entries: 1"));
    assert_eq!(
        fs::read_to_string(fixture.target.join("src/main.rs")).expect("restored source reads"),
        "fn main() {}\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.target.join("Cargo.toml")).expect("restored manifest reads"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n"
    );
    assert!(!fixture.target.join("node_modules").exists());
}

#[test]
fn snapshot_dry_run_stays_non_persisting() {
    let fixture = SnapshotCliFixture::new();
    fixture.write("README.md", "hello\n");

    let dry_run = run_devbox([
        "snapshot",
        "--cache",
        fixture.cache_path(),
        "--dry-run",
        fixture.project_path(),
    ]);
    assert_success(&dry_run);

    let output = stdout(&dry_run);
    assert!(output.contains("Draft snapshot id: "));
    assert!(output.contains("SQLite persistence: deferred"));
    assert!(!fixture.db_path_buf().exists());
}

#[test]
fn snapshot_create_rejects_in_project_db_without_metadata_leftovers() {
    let fixture = SnapshotCliFixture::new();
    fixture.write("README.md", "hello\n");
    let in_project_db = fixture.project.join("devbox.sqlite3");

    let rejected = run_devbox([
        "snapshot",
        "--db",
        path_str(&in_project_db),
        "--cache",
        fixture.cache_path(),
        fixture.project_path(),
    ]);
    assert_failure(&rejected);

    assert!(stderr(&rejected).contains("metadata database path"));
    assert!(!in_project_db.exists());

    let create = run_devbox([
        "snapshot",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        fixture.project_path(),
    ]);
    assert_success(&create);
    let snapshot_id = stdout(&create)
        .lines()
        .find_map(|line| line.strip_prefix("Snapshot id: "))
        .expect("snapshot id prints")
        .to_string();

    let show = run_devbox([
        "snapshot",
        "show",
        "--db",
        fixture.db_path(),
        snapshot_id.as_str(),
    ]);
    assert_success(&show);
    let show_stdout = stdout(&show);
    assert!(show_stdout.contains("README.md\tfile\tinclude"));
    assert!(!show_stdout.contains("devbox.sqlite3\tfile\tinclude"));
}

#[test]
fn snapshot_list_missing_db_fails_without_creating_it() {
    let fixture = SnapshotCliFixture::new();

    let list = run_devbox(["snapshot", "list", "--db", fixture.db_path()]);
    assert_failure(&list);

    assert!(stderr(&list).contains("metadata database does not exist"));
    assert!(!fixture.db_path_buf().exists());
}

#[test]
fn snapshot_show_missing_db_fails_without_creating_it() {
    let fixture = SnapshotCliFixture::new();

    let show = run_devbox([
        "snapshot",
        "show",
        "--db",
        fixture.db_path(),
        "snapshot-missing",
    ]);
    assert_failure(&show);

    assert!(stderr(&show).contains("metadata database does not exist"));
    assert!(!fixture.db_path_buf().exists());
}

struct SnapshotCliFixture {
    _dir: tempfile::TempDir,
    project: std::path::PathBuf,
    cache: std::path::PathBuf,
    db: std::path::PathBuf,
    target: std::path::PathBuf,
}

impl SnapshotCliFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let project = dir.path().join("project");
        let cache = dir.path().join("cache");
        let db = dir.path().join("devbox.sqlite3");
        let target = dir.path().join("target");
        fs::create_dir_all(&project).expect("project dir creates");

        Self {
            _dir: dir,
            project,
            cache,
            db,
            target,
        }
    }

    fn write(&self, path: &str, content: &str) {
        let path = self.project.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir creates");
        }
        fs::write(path, content).expect("fixture file writes");
    }

    fn project_path(&self) -> &str {
        self.project.to_str().expect("test paths are UTF-8")
    }

    fn cache_path(&self) -> &str {
        self.cache.to_str().expect("test paths are UTF-8")
    }

    fn db_path(&self) -> &str {
        self.db.to_str().expect("test paths are UTF-8")
    }

    fn db_path_buf(&self) -> &std::path::Path {
        &self.db
    }

    fn target_path(&self) -> &str {
        self.target.to_str().expect("test paths are UTF-8")
    }

    fn target_path_buf(&self) -> &std::path::Path {
        &self.target
    }
}

fn run_devbox<const N: usize>(args: [&str; N]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devbox"))
        .args(args)
        .output()
        .expect("devbox command runs")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn assert_failure(output: &Output) {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

fn path_str(path: &std::path::Path) -> &str {
    path.to_str().expect("test paths are UTF-8")
}
