use rusqlite::Connection;
use std::fs;
use std::process::{Command, Output};

#[test]
fn watch_once_scans_pending_changes_and_is_idempotent() {
    let fixture = WatchFixture::new();
    fixture.write("README.md", "hello\n");
    fixture.write("node_modules/left-pad/index.js", "module.exports = true;\n");

    let first = fixture.run_watch_once();
    assert_success(&first);
    let first_stdout = stdout(&first);
    assert!(first_stdout.contains("watch status=start"));
    assert!(first_stdout.contains("watch event=batched reason=once events=0"));
    assert!(first_stdout.contains("watch scan=1"));
    assert!(first_stdout.contains("created=1"));
    assert!(first_stdout.contains("skipped_deferred=1"));
    assert!(first_stdout.contains("pending_operations=1"));
    assert!(first_stdout.contains("watch status=idle scans=1"));

    let second = fixture.run_watch_once();
    assert_success(&second);
    assert!(stdout(&second).contains("pending_operations=1"));
    assert_eq!(pending_change_row_count(fixture.db_path()), 1);
    assert_eq!(pending_change_paths(fixture.db_path()), vec!["README.md"]);
}

#[test]
fn watch_once_rejects_in_project_cache_and_db_without_leftovers() {
    let fixture = WatchFixture::new();
    fixture.write("README.md", "hello\n");
    let in_project_cache = fixture.project.join(".devbox-cache");
    let in_project_db = fixture.project.join("devbox.sqlite3");

    let rejected_cache = run_devbox_daemon([
        "watch",
        "--db",
        fixture.db_path(),
        "--cache",
        path_str(&in_project_cache),
        "--once",
        fixture.project_path(),
    ]);
    assert_failure(&rejected_cache);
    assert!(stderr(&rejected_cache).contains("blob%20cache%20root"));
    assert!(!in_project_cache.exists());

    let rejected_db = run_devbox_daemon([
        "watch",
        "--db",
        path_str(&in_project_db),
        "--cache",
        fixture.cache_path(),
        "--once",
        fixture.project_path(),
    ]);
    assert_failure(&rejected_db);
    assert!(stderr(&rejected_db).contains("metadata%20database%20path"));
    assert!(!in_project_db.exists());
}

struct WatchFixture {
    _dir: tempfile::TempDir,
    project: std::path::PathBuf,
    cache: std::path::PathBuf,
    db: std::path::PathBuf,
}

impl WatchFixture {
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

    fn run_watch_once(&self) -> Output {
        run_devbox_daemon([
            "watch",
            "--db",
            self.db_path(),
            "--cache",
            self.cache_path(),
            "--once",
            self.project_path(),
        ])
    }

    fn project_path(&self) -> &str {
        path_str(&self.project)
    }

    fn cache_path(&self) -> &str {
        path_str(&self.cache)
    }

    fn db_path(&self) -> &str {
        path_str(&self.db)
    }
}

fn run_devbox_daemon<const N: usize>(args: [&str; N]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devbox-daemon"))
        .args(args)
        .output()
        .expect("devbox-daemon command runs")
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

fn pending_change_row_count(db_path: &str) -> u64 {
    let conn = Connection::open(db_path).expect("metadata database opens");
    conn.query_row("SELECT COUNT(*) FROM pending_local_changes", [], |row| {
        row.get(0)
    })
    .expect("pending change count reads")
}

fn pending_change_paths(db_path: &str) -> Vec<String> {
    let conn = Connection::open(db_path).expect("metadata database opens");
    let mut statement = conn
        .prepare("SELECT path FROM pending_local_changes ORDER BY path ASC")
        .expect("pending path statement prepares");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("pending paths query");

    rows.collect::<Result<Vec<_>, _>>()
        .expect("pending paths collect")
}
