use devbox_auth::{
    approve_pairing_join_request, create_pairing_invitation, create_pairing_join_request,
    pairing_completion_from_approval, LocalIdentityView,
};
use devbox_materialize::{
    import_snapshot_with_metadata, HostedMetadataImportOptions, ImportSnapshotRequest,
};
use devbox_metadata::{MetadataStore, SqliteMetadataStore};
use devbox_snapshot::SnapshotManifestBuilder;
use devbox_store::{
    local_project_id, BlobCache, EnsureLocalIdentityOptions, NewProject, NewSnapshot,
    NewSnapshotDraft, NewSnapshotManifestEntry, Store,
};
use devbox_sync::LocalFilesystemBlobProvider;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
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

#[test]
fn watch_rejects_in_project_cache_and_db_before_idle_without_leftovers() {
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
        "--exit-after-idle-ms",
        "1",
        fixture.project_path(),
    ]);
    assert_failure(&rejected_cache);
    assert!(stderr(&rejected_cache).contains("blob%20cache%20root"));
    assert!(!stdout(&rejected_cache).contains("watch status=start"));
    assert!(!in_project_cache.exists());

    let rejected_db = run_devbox_daemon([
        "watch",
        "--db",
        path_str(&in_project_db),
        "--cache",
        fixture.cache_path(),
        "--exit-after-idle-ms",
        "1",
        fixture.project_path(),
    ]);
    assert_failure(&rejected_db);
    assert!(stderr(&rejected_db).contains("metadata%20database%20path"));
    assert!(!stdout(&rejected_db).contains("watch status=start"));
    assert!(!in_project_db.exists());
}

#[test]
fn sync_once_push_publishes_snapshot_registers_latest_and_clears_pending() {
    let fixture = LiveFixture::new();
    let identity = fixture.init_identity(&fixture.source_db, "Desk");
    fixture.write("README.md", "hello live sync\n");

    let output = fixture.run_sync_push_once(&fixture.source_db, &fixture.source_cache);

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("sync status=start"));
    assert!(stdout.contains("sync scan=1"));
    assert!(stdout.contains("action=publish status=ok"));
    assert!(stdout.contains("metadata mode=mock-dev-sqlite"));
    assert!(stdout.contains("credentials=not_used"));
    assert_eq!(pending_change_row_count(path_str(&fixture.source_db)), 0);

    let metadata = SqliteMetadataStore::open_file(&fixture.metadata_db).expect("metadata opens");
    let latest = metadata
        .latest_snapshot(&identity.account_id, &fixture.project_id())
        .expect("latest query succeeds")
        .expect("latest snapshot exists");
    assert_eq!(latest.project_id, fixture.project_id());
    assert_eq!(latest.published_by_device_id, identity.device_id);
}

#[test]
fn sync_pull_discovers_latest_snapshot_and_materializes_to_empty_target() {
    let fixture = LiveFixture::new();
    let source_identity = fixture.init_identity(&fixture.source_db, "Desk");
    fixture.write("README.md", "materialize me\n");
    assert_success(&fixture.run_sync_push_once(&fixture.source_db, &fixture.source_cache));
    fixture.pair_receiver_with_source();
    fs::create_dir_all(&fixture.target).expect("target creates");

    let output = run_devbox_daemon_vec(vec![
        "sync".to_string(),
        "--db".to_string(),
        path_string(&fixture.receiver_db),
        "--cache".to_string(),
        path_string(&fixture.receiver_cache),
        "--remote".to_string(),
        path_string(&fixture.remote),
        "--metadata-mode".to_string(),
        "mock-dev-sqlite".to_string(),
        "--metadata-db".to_string(),
        path_string(&fixture.metadata_db),
        "--metadata-account".to_string(),
        source_identity.account_id,
        "--metadata-project".to_string(),
        fixture.project_id(),
        "--pull".to_string(),
        "--to".to_string(),
        path_string(&fixture.target),
        "--apply".to_string(),
        "--once".to_string(),
        path_string(&fixture.target),
    ]);

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("sync discovery=latest"));
    assert!(stdout.contains("action=materialize status=ok"));
    assert!(stdout.contains("applied=true"));
    assert_eq!(
        fs::read_to_string(fixture.target.join("README.md")).expect("target file reads"),
        "materialize me\n"
    );
}

#[test]
fn sync_refuses_pending_receiver_identity_before_remote_work() {
    let fixture = LiveFixture::new();
    fixture.init_identity(&fixture.source_db, "Desk");
    let pending = fixture.prepare_pending_receiver();
    fixture.write("README.md", "pending receiver must not sync\n");

    let output = run_devbox_daemon_vec(vec![
        "sync".to_string(),
        "--db".to_string(),
        path_string(&pending),
        "--cache".to_string(),
        path_string(&fixture.receiver_cache),
        "--remote".to_string(),
        path_string(&fixture.remote),
        "--once".to_string(),
        path_string(&fixture.project),
    ]);

    assert_failure(&output);
    assert!(stderr(&output).contains("pending%20pairing%20completion"));
    assert!(!stdout(&output).contains("action=publish"));
}

#[test]
fn sync_pull_refuses_divergent_cursor_before_downloading_blobs() {
    let fixture = LiveFixture::new();
    let source_identity = fixture.init_identity(&fixture.source_db, "Desk");
    fixture.write("README.md", "base\n");
    assert_success(&fixture.run_sync_push_once(&fixture.source_db, &fixture.source_cache));
    fixture.pair_receiver_with_source();
    fixture.import_latest_into_receiver(&source_identity.account_id);

    fixture.write("README.md", "receiver local\n");
    fixture.persist_receiver_snapshot_for_project();
    fixture.write("README.md", "incoming remote\n");
    assert_success(&fixture.run_sync_push_once(&fixture.source_db, &fixture.source_cache));
    fixture.write("README.md", "receiver local\n");

    let blocked = run_devbox_daemon_vec(vec![
        "sync".to_string(),
        "--db".to_string(),
        path_string(&fixture.receiver_db),
        "--cache".to_string(),
        path_string(&fixture.receiver_cache),
        "--remote".to_string(),
        path_string(&fixture.remote),
        "--metadata-mode".to_string(),
        "mock-dev-sqlite".to_string(),
        "--metadata-db".to_string(),
        path_string(&fixture.metadata_db),
        "--metadata-account".to_string(),
        source_identity.account_id,
        "--metadata-project".to_string(),
        fixture.project_id(),
        "--pull".to_string(),
        "--once".to_string(),
        path_string(&fixture.project),
    ]);

    assert_failure(&blocked);
    assert!(stdout(&blocked).contains("sync preflight=blocked"));
    assert!(stderr(&blocked).contains("live%20sync%20pull%20refused%20by%20local%20preflight"));
}

#[test]
fn sync_s3_live_mode_requires_object_access_and_env_names() {
    let missing_object_access = run_devbox_daemon([
        "sync",
        "--db",
        "devbox.sqlite3",
        "--cache",
        "cache",
        "--remote-kind",
        "s3",
        "--s3-endpoint",
        "https://account.r2.cloudflarestorage.com",
        "--s3-bucket",
        "devbox",
        "--s3-prefix",
        "accounts/account-alpha/projects/project-devbox",
        "project",
    ]);
    assert_failure(&missing_object_access);
    assert!(stderr(&missing_object_access).contains("--object-access-api"));

    let raw_env_name = run_devbox_daemon([
        "sync",
        "--db",
        "devbox.sqlite3",
        "--cache",
        "cache",
        "--remote-kind",
        "s3",
        "--s3-endpoint",
        "https://account.r2.cloudflarestorage.com",
        "--s3-bucket",
        "devbox",
        "--s3-prefix",
        "accounts/account-alpha/projects/project-devbox",
        "--s3-access-key-env",
        "AKIA-raw-token",
        "--s3-secret-key-env",
        "DEVBOX_R2_SECRET_ACCESS_KEY",
        "--object-access-api",
        "https://metadata.example",
        "--object-access-lease",
        "lease-alpha",
        "--metadata-mode",
        "mock-dev-sqlite",
        "--metadata-db",
        "metadata.sqlite3",
        "--metadata-project",
        "project-devbox",
        "project",
    ]);
    assert_failure(&raw_env_name);
    assert!(stderr(&raw_env_name).contains("--s3-access-key-env"));
    assert!(!stderr(&raw_env_name).contains("DEVBOX_R2_SECRET_ACCESS_KEY"));
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

struct LiveFixture {
    _dir: tempfile::TempDir,
    project: PathBuf,
    target: PathBuf,
    source_db: PathBuf,
    source_cache: PathBuf,
    receiver_db: PathBuf,
    receiver_cache: PathBuf,
    metadata_db: PathBuf,
    remote: PathBuf,
}

impl LiveFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let fixture = Self {
            project: dir.path().join("project"),
            target: dir.path().join("target"),
            source_db: dir.path().join("source.sqlite3"),
            source_cache: dir.path().join("source-cache"),
            receiver_db: dir.path().join("receiver.sqlite3"),
            receiver_cache: dir.path().join("receiver-cache"),
            metadata_db: dir.path().join("metadata.sqlite3"),
            remote: dir.path().join("remote"),
            _dir: dir,
        };
        fs::create_dir_all(&fixture.project).expect("project creates");
        fixture
    }

    fn write(&self, path: &str, content: &str) {
        let path = self.project.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent creates");
        }
        fs::write(path, content).expect("fixture file writes");
    }

    fn init_identity(&self, db_path: &Path, name: &str) -> devbox_store::LocalIdentityRecord {
        let mut store = Store::open_file(db_path).expect("store opens");
        store.apply_migrations().expect("migrations apply");
        store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some(name),
            })
            .expect("identity initializes")
    }

    fn run_sync_push_once(&self, db_path: &Path, cache_root: &Path) -> Output {
        run_devbox_daemon_vec(vec![
            "sync".to_string(),
            "--db".to_string(),
            path_string(db_path),
            "--cache".to_string(),
            path_string(cache_root),
            "--remote".to_string(),
            path_string(&self.remote),
            "--metadata-mode".to_string(),
            "mock-dev-sqlite".to_string(),
            "--metadata-db".to_string(),
            path_string(&self.metadata_db),
            "--once".to_string(),
            path_string(&self.project),
        ])
    }

    fn project_id(&self) -> String {
        local_project_id(fs::canonicalize(&self.project).expect("project canonicalizes"))
            .to_string()
    }

    fn prepare_pending_receiver(&self) -> PathBuf {
        let source = Store::open_file(&self.source_db).expect("source opens");
        source.apply_migrations().expect("source migrates");
        let identity = source
            .local_identity()
            .expect("source identity reads")
            .expect("source identity exists");
        let view = identity_view(&identity);
        let draft = create_pairing_invitation(&view, "2026-06-19T10:00:00Z", 100, 600)
            .expect("invitation creates");
        source
            .insert_pairing_invitation(&draft.invitation)
            .expect("invitation persists");

        let mut receiver = Store::open_file(&self.receiver_db).expect("receiver opens");
        receiver.apply_migrations().expect("receiver migrates");
        receiver
            .prepare_pairing_receiver_identity(&draft.token, "Laptop")
            .expect("pending receiver prepares");
        self.receiver_db.clone()
    }

    fn pair_receiver_with_source(&self) {
        let mut source = Store::open_file(&self.source_db).expect("source opens");
        source.apply_migrations().expect("source migrates");
        let identity = source
            .local_identity()
            .expect("source identity reads")
            .expect("source identity exists");
        let view = identity_view(&identity);
        let draft = create_pairing_invitation(&view, "2026-06-19T10:00:00Z", 100, 600)
            .expect("invitation creates");
        source
            .insert_pairing_invitation(&draft.invitation)
            .expect("invitation persists");

        let mut receiver = Store::open_file(&self.receiver_db).expect("receiver opens");
        receiver.apply_migrations().expect("receiver migrates");
        let receiver_identity = receiver
            .prepare_pairing_receiver_identity(&draft.token, "Laptop")
            .expect("receiver identity prepares");
        let join = create_pairing_join_request(&draft.token, &receiver_identity.device_id)
            .expect("join creates");
        let approval = approve_pairing_join_request(
            &view,
            &draft.invitation,
            &draft.token,
            &join,
            "Laptop",
            "2026-06-19T10:01:00Z",
            101,
        )
        .expect("join approves");
        source
            .persist_pairing_approval(&approval)
            .expect("approval persists");
        let completion = pairing_completion_from_approval(&approval);
        receiver
            .complete_pairing_for_local_device(&completion)
            .expect("pairing completes");
    }

    fn persist_receiver_snapshot_for_project(&self) {
        persist_snapshot_for_project(
            &self.receiver_db,
            &self.receiver_cache,
            &self.project,
            &self.project_id(),
        );
    }

    fn import_latest_into_receiver(&self, account_id: &str) {
        let mut metadata =
            SqliteMetadataStore::open_file(&self.metadata_db).expect("metadata opens");
        let latest = metadata
            .latest_snapshot(account_id, &self.project_id())
            .expect("latest query succeeds")
            .expect("latest snapshot exists");
        let provider = LocalFilesystemBlobProvider::open(&self.remote).expect("remote opens");
        import_snapshot_with_metadata(
            &ImportSnapshotRequest {
                db_path: self.receiver_db.clone(),
                cache_root: self.receiver_cache.clone(),
                key_source_db_path: None,
                snapshot_id: latest.snapshot_id,
            },
            &provider,
            &mut metadata,
            &HostedMetadataImportOptions {
                account_id: account_id.to_string(),
                project_id: self.project_id(),
            },
        )
        .expect("latest imports into receiver");
    }
}

fn run_devbox_daemon<const N: usize>(args: [&str; N]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devbox-daemon"))
        .args(args)
        .output()
        .expect("devbox-daemon command runs")
}

fn run_devbox_daemon_vec(args: Vec<String>) -> Output {
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

fn path_string(path: &Path) -> String {
    path_str(path).to_string()
}

fn identity_view(identity: &devbox_store::LocalIdentityRecord) -> LocalIdentityView {
    LocalIdentityView {
        account_id: identity.account_id.clone(),
        device_id: identity.device_id.clone(),
        device_name: identity.device_name.clone(),
        sync_key_hex: identity.sync_key_hex.clone(),
    }
}

fn persist_snapshot_for_project(db_path: &Path, cache_root: &Path, root: &Path, project_id: &str) {
    let cache = BlobCache::open(cache_root).expect("cache opens");
    let snapshot = SnapshotManifestBuilder::new(cache)
        .build_draft(root)
        .expect("snapshot builds");
    let mut store = Store::open_file(db_path).expect("store opens");
    store.apply_migrations().expect("migrations apply");
    let created_at = store.current_timestamp().expect("timestamp reads");
    let root_path = snapshot.root().display().to_string();
    let snapshot_id = snapshot.id().to_string();
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
    store
        .persist_draft_snapshot(&NewSnapshotDraft {
            project: NewProject {
                id: project_id,
                root_path: &root_path,
                kind: "local",
                display_name: "project",
                discovered_at: &created_at,
            },
            snapshot: NewSnapshot {
                id: &snapshot_id,
                project_id,
                parent_snapshot_id: None,
                created_at: &created_at,
                reason: "test-local",
                manifest_entry_count: snapshot.summary().total_entries() as u64,
                total_size_bytes: snapshot.summary().total_file_bytes(),
            },
            entries,
        })
        .expect("snapshot persists");
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
