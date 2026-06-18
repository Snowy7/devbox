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
fn snapshot_blocks_secret_files_without_printing_raw_values() {
    let fixture = SnapshotCliFixture::new();
    let raw_secret = synthetic_token("sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456");
    fixture.write("README.md", "safe\n");
    fixture.write("secrets.env", &format!("OPENAI_API_KEY={raw_secret}\n"));

    assert_success(&run_devbox([
        "init",
        "--db",
        fixture.db_path(),
        "--device-name",
        "Desk",
    ]));
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
    assert!(create_stdout.contains("Blocked secrets: 1"));
    assert!(
        create_stdout.contains("SECRET\tsecrets.env\tsecret blocked by policy rule openai_api_key")
    );
    assert!(create_stdout.contains("sk-<redacted>"));
    assert!(!create_stdout.contains(&raw_secret));

    let snapshot_id = prefixed_value(&create_stdout, "Snapshot id: ");
    let show = run_devbox([
        "snapshot",
        "show",
        "--db",
        fixture.db_path(),
        snapshot_id.as_str(),
    ]);
    assert_success(&show);
    let show_stdout = stdout(&show);
    assert!(show_stdout.contains("Blocked secrets: 1"));
    assert!(show_stdout.contains("secrets.env\tfile\trequires_user_decision"));
    assert!(show_stdout.contains("sk-<redacted>"));
    assert!(!show_stdout.contains(&raw_secret));
    assert_eq!(
        manifest_entry_blob_refs(fixture.db_path(), snapshot_id.as_str(), "secrets.env"),
        (None, None)
    );

    let publish = run_devbox([
        "sync",
        "publish-snapshot",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        "--remote",
        fixture.remote_path(),
        snapshot_id.as_str(),
    ]);
    assert_success(&publish);
    assert!(stdout(&publish).contains("Included blob count: 1"));
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
fn auth_device_pairing_and_cursor_smoke_use_local_mock_metadata() {
    let fixture = SnapshotCliFixture::new();

    assert_success(&run_devbox([
        "init",
        "--db",
        fixture.db_path(),
        "--device-name",
        "Desk",
    ]));

    let login = run_devbox(["auth", "mock-login", "--db", fixture.db_path()]);
    assert_success(&login);
    let login_stdout = stdout(&login);
    assert!(login_stdout.contains("Mock auth session active"));
    assert!(login_stdout.contains("Provider: local-mock"));
    assert!(login_stdout.contains("Production authentication: not configured"));
    assert!(!login_stdout.contains("sync_key"));
    assert!(!login_stdout.contains("device_key"));

    let status = run_devbox(["auth", "status", "--db", fixture.db_path()]);
    assert_success(&status);
    let status_stdout = stdout(&status);
    assert!(status_stdout.contains("Auth boundary: local/mock"));
    assert!(status_stdout.contains("Session state: active"));

    let invite = run_devbox([
        "devices",
        "invite",
        "--db",
        fixture.db_path(),
        "--ttl-seconds",
        "600",
    ]);
    assert_success(&invite);
    let invite_stdout = stdout(&invite);
    assert!(invite_stdout.contains("Pairing invitation created"));
    assert!(invite_stdout.contains("Provider: local/mock metadata"));
    let token = prefixed_value(&invite_stdout, "Pairing token: ");

    let approve = run_devbox([
        "devices",
        "approve",
        "--db",
        fixture.db_path(),
        "--token",
        &token,
        "--device-name",
        "Travel laptop",
    ]);
    assert_success(&approve);
    let approve_stdout = stdout(&approve);
    assert!(approve_stdout.contains("Device approved"));
    assert!(approve_stdout.contains("Device name: Travel laptop"));
    assert!(approve_stdout.contains("Trust state: approved"));
    assert!(approve_stdout.contains("Key envelope stored: true"));
    assert!(!approve_stdout.contains("sync_key"));
    assert!(!approve_stdout.contains("device_key"));
    let approved_device_id = prefixed_value(&approve_stdout, "Device id: ");

    let devices = run_devbox(["devices", "list", "--db", fixture.db_path()]);
    assert_success(&devices);
    let devices_stdout = stdout(&devices);
    assert!(devices_stdout.contains("Trust state"));
    assert!(devices_stdout.contains("\ttrue\tDesk\tcurrent-local\t"));
    assert!(devices_stdout.contains("\tfalse\tTravel laptop\tapproved\t"));

    let revoke = run_devbox([
        "devices",
        "revoke",
        "--db",
        fixture.db_path(),
        &approved_device_id,
        "--reason",
        "manual smoke",
    ]);
    assert_success(&revoke);
    let revoke_stdout = stdout(&revoke);
    assert!(revoke_stdout.contains("Device revoked"));
    assert!(revoke_stdout.contains("Trust state: revoked"));

    let project_id = "project-smoke";
    let cursor_set = run_devbox([
        "sync",
        "cursor",
        "set",
        "--db",
        fixture.db_path(),
        "--project",
        project_id,
        "--value",
        "snapshot-123",
    ]);
    assert_success(&cursor_set);
    assert!(stdout(&cursor_set).contains("Sync cursor updated"));

    let cursor_get = run_devbox([
        "sync",
        "cursor",
        "get",
        "--db",
        fixture.db_path(),
        "--project",
        project_id,
    ]);
    assert_success(&cursor_get);
    let cursor_get_stdout = stdout(&cursor_get);
    assert!(cursor_get_stdout.contains("Cursor value: snapshot-123"));
    assert!(cursor_get_stdout.contains("Provider: local/mock metadata"));
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
fn sync_snapshot_publish_import_and_materialize_smoke_uses_two_local_contexts() {
    let fixture = SnapshotCliFixture::new();
    let plaintext = "hello from device one\n";
    fixture.write("README.md", plaintext);
    fixture.write("src/main.rs", "fn main() {}\n");
    fixture.write("node_modules/left-pad/index.js", "module.exports = true;\n");
    fixture.write(".git/config", "[core]\nrepositoryformatversion = 0\n");

    assert_success(&run_devbox([
        "init",
        "--db",
        fixture.db_path(),
        "--device-name",
        "Desk",
    ]));
    assert_success(&run_devbox([
        "init",
        "--db",
        fixture.receiver_db_path(),
        "--device-name",
        "Laptop",
    ]));

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

    let publish = run_devbox([
        "sync",
        "publish-snapshot",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        "--remote",
        fixture.remote_path(),
        snapshot_id.as_str(),
    ]);
    assert_success(&publish);
    let publish_stdout = stdout(&publish);
    assert!(publish_stdout.contains("Sync publish snapshot: encrypted local/mock bundle"));
    assert!(publish_stdout.contains("Boundary: local/mock second-device foundation"));
    assert!(publish_stdout.contains("Included blob count: 2"));
    let manifest_object_key = prefixed_value(&publish_stdout, "Manifest object key: ");
    let remote_manifest = fs::read(remote_object_path(
        fixture.remote_path_buf(),
        &manifest_object_key,
    ))
    .expect("remote manifest object reads");
    assert!(!remote_manifest
        .windows("src/main.rs".len())
        .any(|window| window == b"src/main.rs"));
    assert!(!remote_manifest
        .windows(plaintext.len())
        .any(|window| window == plaintext.as_bytes()));

    let import = run_devbox([
        "sync",
        "import-snapshot",
        "--db",
        fixture.receiver_db_path(),
        "--cache",
        fixture.download_cache_path(),
        "--remote",
        fixture.remote_path(),
        "--mock-key-source-db",
        fixture.db_path(),
        snapshot_id.as_str(),
    ]);
    assert_success(&import);
    let import_stdout = stdout(&import);
    assert!(
        import_stdout.contains("Sync import snapshot: decrypted into local/mock second context")
    );
    assert!(import_stdout.contains("Snapshot inserted: true"));
    assert!(import_stdout.contains("Downloaded blob count: 2"));
    assert!(import_stdout.contains("Trust bootstrap: local/mock --mock-key-source-db used"));

    let second_import = run_devbox([
        "sync",
        "import-snapshot",
        "--db",
        fixture.receiver_db_path(),
        "--cache",
        fixture.download_cache_path(),
        "--remote",
        fixture.remote_path(),
        "--mock-key-source-db",
        fixture.db_path(),
        snapshot_id.as_str(),
    ]);
    assert_success(&second_import);
    assert!(stdout(&second_import).contains("Snapshot inserted: false"));

    let materialize = run_devbox([
        "sync",
        "materialize",
        "--db",
        fixture.receiver_db_path(),
        "--cache",
        fixture.download_cache_path(),
        "--remote",
        fixture.remote_path(),
        "--to",
        fixture.target_path(),
        "--mock-key-source-db",
        fixture.db_path(),
        snapshot_id.as_str(),
        "--apply",
    ]);
    assert_success(&materialize);
    let materialize_stdout = stdout(&materialize);
    assert!(materialize_stdout.contains("Sync materialize snapshot"));
    assert!(materialize_stdout.contains("Mode: apply"));
    assert!(materialize_stdout.contains("Apply allowed: true"));
    assert!(materialize_stdout.contains("Applied: true"));
    assert_eq!(
        fs::read_to_string(fixture.target.join("README.md")).expect("readme restored"),
        plaintext
    );
    assert_eq!(
        fs::read_to_string(fixture.target.join("src/main.rs")).expect("source restored"),
        "fn main() {}\n"
    );
    assert!(!fixture.target.join("node_modules").exists());
    assert!(!fixture.target.join(".git").exists());

    let cursor_get = run_devbox([
        "sync",
        "cursor",
        "get",
        "--db",
        fixture.receiver_db_path(),
        "--project",
        &prefixed_value(&materialize_stdout, "Project id: "),
    ]);
    assert_success(&cursor_get);
    assert!(stdout(&cursor_get).contains(&format!("Cursor value: {snapshot_id}")));

    let refused = run_devbox([
        "sync",
        "materialize",
        "--db",
        fixture.receiver_db_path(),
        "--cache",
        fixture.download_cache_path(),
        "--remote",
        fixture.remote_path(),
        "--to",
        fixture.target_path(),
        "--mock-key-source-db",
        fixture.db_path(),
        snapshot_id.as_str(),
        "--apply",
    ]);
    assert_failure(&refused);
    assert!(stderr(&refused).contains("restore target must be missing or empty"));
    assert_eq!(
        fs::read_to_string(fixture.target.join("README.md")).expect("existing readme reads"),
        plaintext
    );
}

#[test]
fn sync_import_refuses_when_receiver_cursor_has_diverged() {
    let fixture = SnapshotCliFixture::new();
    fixture.write("README.md", "incoming\n");

    assert_success(&run_devbox([
        "init",
        "--db",
        fixture.db_path(),
        "--device-name",
        "Desk",
    ]));
    assert_success(&run_devbox([
        "init",
        "--db",
        fixture.receiver_db_path(),
        "--device-name",
        "Laptop",
    ]));

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
    let incoming_id = prefixed_value(&create_stdout, "Snapshot id: ");
    let project_id = prefixed_value(&create_stdout, "Project id: ");

    let publish = run_devbox([
        "sync",
        "publish-snapshot",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        "--remote",
        fixture.remote_path(),
        &incoming_id,
    ]);
    assert_success(&publish);

    insert_manual_snapshot(
        fixture.receiver_db_path(),
        &project_id,
        "snapshot-base",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "base\n",
        "2026-06-18T10:00:00Z",
    );
    insert_manual_snapshot(
        fixture.receiver_db_path(),
        &project_id,
        "snapshot-local",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "local\n",
        "2026-06-18T10:01:00Z",
    );
    assert_success(&run_devbox([
        "sync",
        "cursor",
        "set",
        "--db",
        fixture.receiver_db_path(),
        "--project",
        &project_id,
        "--value",
        "snapshot-base",
    ]));

    let import = run_devbox([
        "sync",
        "import-snapshot",
        "--db",
        fixture.receiver_db_path(),
        "--cache",
        fixture.download_cache_path(),
        "--remote",
        fixture.remote_path(),
        "--mock-key-source-db",
        fixture.db_path(),
        &incoming_id,
    ]);
    assert_failure(&import);
    let import_stdout = stdout(&import);
    assert!(import_stdout.contains("Preflight: blocked"));
    assert!(import_stdout.contains("Local snapshot id: snapshot-local"));
    assert!(import_stdout.contains(&format!("Incoming snapshot id: {incoming_id}")));
    assert!(import_stdout.contains("Both modified different: 1"));
    assert!(stderr(&import).contains("sync import-snapshot refused by local preflight"));
    assert!(!fixture.download_cache_path_buf().join("blobs").exists());

    let cursor_get = run_devbox([
        "sync",
        "cursor",
        "get",
        "--db",
        fixture.receiver_db_path(),
        "--project",
        &project_id,
    ]);
    assert_success(&cursor_get);
    assert!(stdout(&cursor_get).contains("Cursor value: snapshot-base"));
    assert_eq!(conflict_row_count(fixture.receiver_db_path()), 1);
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
fn conflicts_compare_list_show_and_resolve_divergent_snapshots() {
    let fixture = SnapshotCliFixture::new();
    let raw_base_secret = synthetic_token("sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456");
    let raw_local_secret = synthetic_token("sk-", "bcdefghijklmnopqrstuvwxyzABCDEFGH1234567");
    let raw_incoming_secret = synthetic_token("sk-", "cdefghijklmnopqrstuvwxyzABCDEFGH12345678");
    fixture.write("same.txt", "same\n");
    fixture.write("both.txt", "base\n");
    fixture.write("delete-local.txt", "base local delete\n");
    fixture.write("delete-incoming.txt", "base incoming delete\n");
    fixture.write("secret.env", &format!("OPENAI_API_KEY={raw_base_secret}\n"));
    fixture.write("node_modules/left-pad/index.js", "module.exports = true;\n");

    let base = run_devbox([
        "snapshot",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        fixture.project_path(),
    ]);
    assert_success(&base);
    let base_id = prefixed_value(&stdout(&base), "Snapshot id: ");

    fixture.write("both.txt", "local\n");
    fs::remove_file(fixture.project.join("delete-local.txt")).expect("local delete applies");
    fixture.write("local-only.txt", "local only\n");
    fixture.write(
        "secret.env",
        &format!("OPENAI_API_KEY={raw_local_secret}\n"),
    );
    let local = run_devbox([
        "snapshot",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        fixture.project_path(),
    ]);
    assert_success(&local);
    let local_id = prefixed_value(&stdout(&local), "Snapshot id: ");

    fixture.write("both.txt", "incoming\n");
    fixture.write("delete-local.txt", "base local delete\n");
    fs::remove_file(fixture.project.join("delete-incoming.txt")).expect("incoming delete applies");
    fs::remove_file(fixture.project.join("local-only.txt")).expect("local-only removes");
    fixture.write("incoming-only.txt", "incoming only\n");
    fixture.write(
        "secret.env",
        &format!("OPENAI_API_KEY={raw_incoming_secret}\n"),
    );
    let incoming = run_devbox([
        "snapshot",
        "--db",
        fixture.db_path(),
        "--cache",
        fixture.cache_path(),
        fixture.project_path(),
    ]);
    assert_success(&incoming);
    let incoming_id = prefixed_value(&stdout(&incoming), "Snapshot id: ");
    let project_id = prefixed_value(&stdout(&incoming), "Project id: ");

    let preflight = run_devbox([
        "sync",
        "preflight",
        "--db",
        fixture.db_path(),
        "--project",
        &project_id,
        "--base",
        &base_id,
        "--local",
        &local_id,
        "--incoming",
        &incoming_id,
    ]);
    assert_failure(&preflight);
    let preflight_stdout = stdout(&preflight);
    assert!(preflight_stdout.contains("Preflight: blocked"));
    assert!(preflight_stdout.contains(&format!("Project id: {project_id}")));
    assert!(preflight_stdout.contains("Both modified different: 1"));
    assert!(preflight_stdout.contains("Policy blocked: 1"));
    assert!(stderr(&preflight).contains("sync preflight blocked"));
    let conflict_id = prefixed_value(&preflight_stdout, "Conflict id: ");

    let compare = run_devbox([
        "conflicts",
        "compare",
        "--db",
        fixture.db_path(),
        "--base",
        &base_id,
        "--local",
        &local_id,
        "--incoming",
        &incoming_id,
    ]);
    assert_success(&compare);
    let compare_stdout = stdout(&compare);
    assert!(compare_stdout.contains("Conflict compare: divergent snapshots"));
    assert!(compare_stdout.contains("Status: open"));
    assert!(compare_stdout.contains("both.txt\tboth-modified-different\tfile"));
    assert!(compare_stdout.contains("delete-local.txt\tlocal-deleted\tfile"));
    assert!(compare_stdout.contains("delete-incoming.txt\tincoming-deleted\tfile"));
    assert!(compare_stdout.contains("incoming-only.txt\tincoming-only\tfile"));
    assert!(compare_stdout.contains("local-only.txt\tlocal-only\tfile"));
    assert!(compare_stdout.contains("node_modules\tpolicy-excluded\tdirectory"));
    assert!(compare_stdout.contains("secret.env\tpolicy-blocked\tfile"));
    assert!(compare_stdout.contains("sk-<redacted>"));
    assert!(!compare_stdout.contains(&raw_base_secret));
    assert!(!compare_stdout.contains(&raw_local_secret));
    assert!(!compare_stdout.contains(&raw_incoming_secret));
    assert_eq!(
        prefixed_value(&compare_stdout, "Conflict id: "),
        conflict_id
    );

    let second_compare = run_devbox([
        "conflicts",
        "compare",
        "--db",
        fixture.db_path(),
        "--base",
        &base_id,
        "--local",
        &local_id,
        "--incoming",
        &incoming_id,
    ]);
    assert_success(&second_compare);
    assert_eq!(
        prefixed_value(&stdout(&second_compare), "Conflict id: "),
        conflict_id
    );

    let list = run_devbox(["conflicts", "list", "--db", fixture.db_path()]);
    assert_success(&list);
    let list_stdout = stdout(&list);
    assert!(list_stdout.contains(
        "Conflict id\tStatus\tProject id\tBase snapshot id\tLocal snapshot id\tIncoming snapshot id"
    ));
    assert!(list_stdout.contains(&conflict_id));
    assert!(list_stdout.contains(&base_id));
    assert!(list_stdout.contains(&local_id));
    assert!(list_stdout.contains(&incoming_id));

    let show = run_devbox(["conflicts", "show", "--db", fixture.db_path(), &conflict_id]);
    assert_success(&show);
    let show_stdout = stdout(&show);
    assert!(show_stdout.contains("Entries:"));
    assert!(show_stdout.contains("secret.env\tpolicy-blocked\tfile"));
    assert!(!show_stdout.contains(&raw_local_secret));
    assert!(!show_stdout.contains(&raw_incoming_secret));

    let resolve = run_devbox([
        "conflicts",
        "resolve",
        "--db",
        fixture.db_path(),
        &conflict_id,
    ]);
    assert_success(&resolve);
    assert!(stdout(&resolve).contains("Status: resolved"));
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
    receiver_db: std::path::PathBuf,
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
        let receiver_db = dir.path().join("receiver.sqlite3");
        let remote = dir.path().join("remote");
        let target = dir.path().join("target");
        fs::create_dir_all(&project).expect("project dir creates");

        Self {
            _dir: dir,
            project,
            cache,
            download_cache,
            db,
            receiver_db,
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

    fn receiver_db_path(&self) -> &str {
        self.receiver_db.to_str().expect("test paths are UTF-8")
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

fn conflict_row_count(db_path: &str) -> u64 {
    let conn = Connection::open(db_path).expect("metadata database opens");
    conn.query_row("SELECT COUNT(*) FROM conflicts", [], |row| row.get(0))
        .expect("conflict count reads")
}

fn insert_manual_snapshot(
    db_path: &str,
    project_id: &str,
    snapshot_id: &str,
    blob_id: &str,
    content: &str,
    created_at: &str,
) {
    let conn = Connection::open(db_path).expect("metadata database opens");
    conn.execute(
        r#"
        INSERT OR IGNORE INTO projects (id, root_path, kind, display_name, discovered_at)
        VALUES (?1, '/workspace/devbox', 'local', 'devbox', ?2)
        "#,
        params![project_id, created_at],
    )
    .expect("project inserts");
    conn.execute(
        r#"
        INSERT OR IGNORE INTO blobs (id, hash_algorithm, size_bytes, object_ref, created_at)
        VALUES (?1, 'blake3', ?2, ?3, ?4)
        "#,
        params![
            blob_id,
            content.len() as u64,
            format!("blobs/b3/{blob_id}"),
            created_at
        ],
    )
    .expect("blob inserts");
    conn.execute(
        r#"
        INSERT INTO snapshots (
            id,
            project_id,
            parent_snapshot_id,
            created_at,
            reason,
            manifest_entry_count,
            total_size_bytes
        )
        VALUES (?1, ?2, NULL, ?3, 'test', 1, ?4)
        "#,
        params![snapshot_id, project_id, created_at, content.len() as u64],
    )
    .expect("snapshot inserts");
    conn.execute(
        r#"
        INSERT INTO manifest_entries (
            snapshot_id,
            path,
            entry_kind,
            blob_id,
            target_path,
            file_mode,
            size_bytes,
            policy_decision,
            policy_reason
        )
        VALUES (?1, 'README.md', 'file', ?2, NULL, NULL, ?3, 'include', NULL)
        "#,
        params![snapshot_id, blob_id, content.len() as u64],
    )
    .expect("manifest entry inserts");
}

fn manifest_entry_blob_refs(
    db_path: &str,
    snapshot_id: &str,
    path: &str,
) -> (Option<String>, Option<String>) {
    let conn = Connection::open(db_path).expect("metadata database opens");
    conn.query_row(
        r#"
        SELECT me.blob_id, b.object_ref
        FROM manifest_entries me
        LEFT JOIN blobs b ON b.id = me.blob_id
        WHERE me.snapshot_id = ?1 AND me.path = ?2
        "#,
        params![snapshot_id, path],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .expect("manifest entry reads")
}

fn prefixed_value(output: &str, prefix: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .expect("prefixed value prints")
        .to_string()
}

fn synthetic_token(prefix: &str, tail: &str) -> String {
    [prefix, tail].concat()
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
