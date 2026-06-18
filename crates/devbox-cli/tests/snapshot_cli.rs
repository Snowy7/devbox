use rusqlite::{params, Connection};
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
fn init_is_idempotent_and_devices_list_current_local_device() {
    let fixture = SnapshotCliFixture::new();

    let first = run_devbox([
        "init",
        "--db",
        fixture.db_path(),
        "--device-name",
        "Current machine",
    ]);
    assert_success(&first);
    let first_stdout = stdout(&first);
    assert!(first_stdout.contains("Local identity initialized"));
    assert!(first_stdout.contains("Current device name: Current machine"));
    assert!(first_stdout.contains("Cloud authentication: not configured"));
    assert!(first_stdout.contains("Key material: stored locally; not printed"));
    assert!(!first_stdout.contains("sync_key"));
    assert!(!first_stdout.contains("device_key"));

    let second = run_devbox([
        "init",
        "--db",
        fixture.db_path(),
        "--device-name",
        "Ignored later name",
    ]);
    assert_success(&second);
    let second_stdout = stdout(&second);
    assert!(second_stdout.contains("Current device name: Current machine"));
    assert_eq!(
        prefixed_value(&first_stdout, "Account id: "),
        prefixed_value(&second_stdout, "Account id: ")
    );
    assert_eq!(
        prefixed_value(&first_stdout, "Current device id: "),
        prefixed_value(&second_stdout, "Current device id: ")
    );

    let devices = run_devbox(["devices", "list", "--db", fixture.db_path()]);
    assert_success(&devices);
    let devices_stdout = stdout(&devices);
    assert!(devices_stdout.contains("Device id\tAccount id\tCurrent local\tName"));
    assert!(devices_stdout.contains("\ttrue\tCurrent machine\t"));
}

#[test]
fn sync_upload_and_download_encrypts_remote_object_bytes() {
    let fixture = SnapshotCliFixture::new();
    let plaintext = "hello encrypted sync foundation\n";
    fixture.write("README.md", plaintext);

    assert_success(&run_devbox(["init", "--db", fixture.db_path()]));
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
    let blob_id = blob_id_for_path(&stdout(&show), "README.md");

    let upload = run_devbox([
        "sync",
        "upload",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        "--remote",
        fixture.remote_path(),
        blob_id.as_str(),
    ]);
    assert_success(&upload);
    let upload_stdout = stdout(&upload);
    assert!(upload_stdout.contains("Sync upload: encrypted local-remote object"));
    assert!(upload_stdout.contains("Cloud authentication: not configured"));
    let object_key = prefixed_value(&upload_stdout, "Object key: ");
    let remote_bytes = fs::read(remote_object_path(fixture.remote_path_buf(), &object_key))
        .expect("remote object reads");
    assert!(!remote_bytes
        .windows(plaintext.len())
        .any(|window| window == plaintext.as_bytes()));

    let download = run_devbox([
        "sync",
        "download",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.download_cache_path(),
        "--remote",
        fixture.remote_path(),
        blob_id.as_str(),
    ]);
    assert_success(&download);
    let restored = fs::read_to_string(cache_blob_path(fixture.download_cache_path_buf(), &blob_id))
        .expect("downloaded cache blob reads");
    assert_eq!(restored, plaintext);
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
fn changes_scan_and_list_smoke_are_stable_and_idempotent() {
    let fixture = SnapshotCliFixture::new();
    fixture.write("Cargo.toml", "[package]\nname = \"demo\"\n");
    fixture.write("src/main.rs", "fn main() {}\n");
    fixture.write("delete-me.txt", "gone soon\n");
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
    let snapshot_id = stdout(&create)
        .lines()
        .find_map(|line| line.strip_prefix("Snapshot id: "))
        .expect("snapshot id prints")
        .to_string();

    fixture.write("src/main.rs", "fn main() { println!(\"changed\"); }\n");
    fixture.write("src/lib.rs", "pub fn added() {}\n");
    fs::remove_file(fixture.project.join("delete-me.txt")).expect("fixture file deletes");
    fixture.write("target/debug/generated", "ignored\n");

    let first_scan = run_devbox([
        "changes",
        "scan",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        fixture.project_path(),
    ]);
    assert_success(&first_scan);
    let first_scan_stdout = stdout(&first_scan);
    assert!(first_scan_stdout.contains(&format!("Base snapshot id: {snapshot_id}")));
    assert!(first_scan_stdout.contains("Created: 1"));
    assert!(first_scan_stdout.contains("Modified: 1"));
    assert!(first_scan_stdout.contains("Deleted: 1"));
    assert!(first_scan_stdout.contains("Unchanged: 1"));
    assert!(first_scan_stdout.contains("Skipped/deferred: 2"));
    assert!(first_scan_stdout.contains("Pending operations: 3"));

    let second_scan = run_devbox([
        "changes",
        "scan",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        fixture.project_path(),
    ]);
    assert_success(&second_scan);
    assert!(stdout(&second_scan).contains("Pending operations: 3"));

    let project_id = first_scan_stdout
        .lines()
        .find_map(|line| line.strip_prefix("Project id: "))
        .expect("project id prints")
        .to_string();
    let list = run_devbox(["changes", "list", "--db", fixture.db_path()]);
    assert_success(&list);
    let list_stdout = stdout(&list);
    assert!(list_stdout.contains(
        "Project id\tBase snapshot id\tChange\tPath\tBytes\tBlob id\tPrevious blob id\tDetected at"
    ));
    assert!(list_stdout.contains(&format!(
        "{project_id}\t{snapshot_id}\tdeleted\tdelete-me.txt"
    )));
    assert!(list_stdout.contains(&format!(
        "{project_id}\t{snapshot_id}\tmodified\tsrc/main.rs"
    )));
    assert!(list_stdout.contains(&format!("{project_id}\t{snapshot_id}\tcreated\tsrc/lib.rs")));
    assert!(!list_stdout.contains("target/debug/generated"));
    assert_eq!(pending_change_row_count(fixture.db_path()), 3);
}

#[test]
fn changes_scan_rejects_in_project_cache_and_db_without_leftovers() {
    let fixture = SnapshotCliFixture::new();
    fixture.write("README.md", "hello\n");
    let in_project_cache = fixture.project.join(".devbox-cache");
    let in_project_db = fixture.project.join("devbox.sqlite3");

    let rejected_cache = run_devbox([
        "changes",
        "scan",
        "--db",
        fixture.db_path(),
        "--cache",
        path_str(&in_project_cache),
        fixture.project_path(),
    ]);
    assert_failure(&rejected_cache);
    assert!(stderr(&rejected_cache).contains("blob cache root"));
    assert!(!in_project_cache.exists());

    let rejected_db = run_devbox([
        "changes",
        "scan",
        "--db",
        path_str(&in_project_db),
        "--cache",
        fixture.cache_path(),
        fixture.project_path(),
    ]);
    assert_failure(&rejected_db);
    assert!(stderr(&rejected_db).contains("metadata database path"));
    assert!(!in_project_db.exists());
}

#[test]
fn snapshot_restore_rejects_tampered_current_dir_manifest_path() {
    let fixture = SnapshotCliFixture::new();
    fixture.write("src/main.rs", "fn main() {}\n");

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

    let conn = Connection::open(fixture.db_path()).expect("metadata database opens");
    let changed = conn
        .execute(
            "UPDATE manifest_entries SET path = ?1 WHERE snapshot_id = ?2 AND path = ?3",
            params!["src/./main.rs", snapshot_id, "src/main.rs"],
        )
        .expect("manifest path tampers");
    assert_eq!(changed, 1);

    let restore = run_devbox([
        "snapshot",
        "restore",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        "--to",
        fixture.target_path(),
        snapshot_id.as_str(),
        "--dry-run",
    ]);
    assert_failure(&restore);

    assert!(stderr(&restore).contains("unsafe manifest path"));
    assert!(!fixture.target_path_buf().exists());
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
    download_cache: std::path::PathBuf,
    db: std::path::PathBuf,
    remote: std::path::PathBuf,
    target: std::path::PathBuf,
}

impl SnapshotCliFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let project = dir.path().join("project");
        let cache = dir.path().join("cache");
        let download_cache = dir.path().join("download-cache");
        let db = dir.path().join("devbox.sqlite3");
        let remote = dir.path().join("remote");
        let target = dir.path().join("target");
        fs::create_dir_all(&project).expect("project dir creates");

        Self {
            _dir: dir,
            project,
            cache,
            download_cache,
            db,
            remote,
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

    fn remote_path(&self) -> &str {
        self.remote.to_str().expect("test paths are UTF-8")
    }

    fn remote_path_buf(&self) -> &std::path::Path {
        &self.remote
    }

    fn download_cache_path(&self) -> &str {
        self.download_cache.to_str().expect("test paths are UTF-8")
    }

    fn download_cache_path_buf(&self) -> &std::path::Path {
        &self.download_cache
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

fn pending_change_row_count(db_path: &str) -> u64 {
    let conn = Connection::open(db_path).expect("metadata database opens");
    conn.query_row("SELECT COUNT(*) FROM pending_local_changes", [], |row| {
        row.get(0)
    })
    .expect("pending change count reads")
}

fn prefixed_value(output: &str, prefix: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .expect("prefixed value prints")
        .to_string()
}

fn blob_id_for_path(output: &str, path: &str) -> String {
    output
        .lines()
        .find_map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
            if fields.len() >= 5 && fields[0] == path {
                Some(fields[4].to_string())
            } else {
                None
            }
        })
        .expect("blob id is present")
}

fn remote_object_path(root: &std::path::Path, object_key: &str) -> std::path::PathBuf {
    object_key
        .split('/')
        .fold(root.join("objects"), |path, segment| path.join(segment))
}

fn cache_blob_path(root: &std::path::Path, blob_id: &str) -> std::path::PathBuf {
    root.join("blobs")
        .join("b3")
        .join(&blob_id[0..2])
        .join(&blob_id[2..4])
        .join(blob_id)
}
