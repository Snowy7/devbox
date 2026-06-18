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

struct SnapshotCliFixture {
    _dir: tempfile::TempDir,
    project: std::path::PathBuf,
    cache: std::path::PathBuf,
    db: std::path::PathBuf,
}

impl SnapshotCliFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let project = dir.path().join("project");
        let cache = dir.path().join("cache");
        let db = dir.path().join("devbox.sqlite3");
        fs::create_dir_all(&project).expect("project dir creates");

        Self {
            _dir: dir,
            project,
            cache,
            db,
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

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}
