use devbox_auth::{
    approve_pairing_join_request, create_account_ownership_proof, create_account_session,
    create_pairing_invitation, create_pairing_join_request, pairing_completion_from_approval,
    AccountOwnershipProofInput, LocalIdentityView,
};
use devbox_materialize::{
    import_snapshot_with_metadata, HostedMetadataImportOptions, ImportSnapshotRequest,
};
use devbox_metadata::{
    app_with_config, HostedApiConfig, ManagedObjectAccessBrokerConfig, ManagedObjectCapability,
    ManagedObjectCredentialLeaseRequest, ManagedObjectProviderKind, MetadataStore,
    SqliteMetadataStore, UpsertProjectRequest,
};
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
fn sync_two_way_refuses_remote_latest_before_publish_when_local_has_pending_changes() {
    let fixture = LiveFixture::new();
    let source_identity = fixture.init_identity(&fixture.source_db, "Desk");
    fixture.write("README.md", "base\n");
    assert_success(&fixture.run_sync_push_once(&fixture.source_db, &fixture.source_cache));
    fixture.pair_receiver_with_source();
    fixture.import_latest_into_receiver(&source_identity.account_id);

    fixture.write("README.md", "other device edit\n");
    assert_success(&fixture.run_sync_push_once(&fixture.receiver_db, &fixture.receiver_cache));
    let remote_latest_before = fixture
        .latest_snapshot_id(&source_identity.account_id)
        .expect("remote latest exists");

    fixture.write("README.md", "local device edit\n");
    let blocked = run_devbox_daemon_vec(vec![
        "sync".to_string(),
        "--db".to_string(),
        path_string(&fixture.source_db),
        "--cache".to_string(),
        path_string(&fixture.source_cache),
        "--remote".to_string(),
        path_string(&fixture.remote),
        "--metadata-mode".to_string(),
        "mock-dev-sqlite".to_string(),
        "--metadata-db".to_string(),
        path_string(&fixture.metadata_db),
        "--metadata-account".to_string(),
        source_identity.account_id.clone(),
        "--metadata-project".to_string(),
        fixture.project_id(),
        "--two-way".to_string(),
        "--once".to_string(),
        path_string(&fixture.project),
    ]);

    assert_failure(&blocked);
    let stdout = stdout(&blocked);
    assert!(stdout.contains("sync discovery=latest"));
    assert!(!stdout.contains("action=publish status=ok"));
    assert!(stderr(&blocked).contains("two-way%20refused%20before%20publish"));
    assert_eq!(
        fixture.latest_snapshot_id(&source_identity.account_id),
        Some(remote_latest_before)
    );
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

#[test]
fn live_sync_script_documents_hosted_api_without_shared_metadata_db() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root exists");
    let env_example = fs::read_to_string(repo.join(".env.example")).expect("env example reads");
    let script = fs::read_to_string(repo.join("scripts/devbox-live-sync-alpha.sh"))
        .expect("live sync script reads");

    assert!(env_example.contains("DEVBOX_METADATA_PROJECT="));
    assert!(script.contains("DEVBOX_METADATA_PROJECT:?set DEVBOX_METADATA_PROJECT"));
    assert!(script.contains("--metadata-mode hosted-api"));
    assert!(script.contains("--metadata-api"));
    assert!(script.contains("--metadata-session-token-env DEVBOX_SESSION_TOKEN"));
    assert!(!script.contains("set DEVBOX_METADATA_DB for hosted live sync metadata"));
}

#[test]
fn sync_hosted_object_transfer_push_and_materialize_without_client_r2_keys() {
    let fixture = LiveFixture::new();
    let source_identity = fixture.init_identity(&fixture.source_db, "Desk");
    let session_token = "raw-live-hosted-session-token";
    fixture.seed_hosted_object_access(&source_identity.account_id, session_token);
    let server = fixture.start_hosted_object_server();
    fixture.write("README.md", "hosted transfer path\n");

    let push = run_devbox_daemon_vec_with_env(
        fixture.hosted_sync_args(
            &fixture.source_db,
            &fixture.source_cache,
            &server.url,
            &source_identity.account_id,
            true,
        ),
        &[("DEVBOX_TEST_SESSION_TOKEN", session_token)],
    );
    assert_success(&push);
    let push_stdout = stdout(&push);
    assert!(push_stdout.contains("remote_kind=hosted"));
    assert!(push_stdout.contains("client_bucket_credentials=false"));
    assert!(!push_stdout.contains("DEVBOX_R2_ACCESS_KEY_ID"));
    assert!(!push_stdout.contains("DEVBOX_R2_SECRET_ACCESS_KEY"));
    assert!(fixture
        .hosted_object_root
        .join("objects")
        .join("accounts")
        .join(&source_identity.account_id)
        .join("projects")
        .join(fixture.project_id())
        .exists());

    fixture.pair_receiver_with_source();
    fs::create_dir_all(&fixture.target).expect("target creates");
    let pull = run_devbox_daemon_vec_with_env(
        fixture.hosted_pull_args(&server.url, &source_identity.account_id),
        &[("DEVBOX_TEST_SESSION_TOKEN", session_token)],
    );
    assert_success(&pull);
    let pull_stdout = stdout(&pull);
    assert!(pull_stdout.contains("action=materialize status=ok"));
    assert!(pull_stdout.contains("remote provider=hosted-object-transfer"));
    assert_eq!(
        fs::read_to_string(fixture.target.join("README.md")).expect("target file reads"),
        "hosted transfer path\n"
    );
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
    hosted_object_root: PathBuf,
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
            hosted_object_root: dir.path().join("hosted-objects"),
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

    fn latest_snapshot_id(&self, account_id: &str) -> Option<String> {
        SqliteMetadataStore::open_file(&self.metadata_db)
            .expect("metadata opens")
            .latest_snapshot(account_id, &self.project_id())
            .expect("latest query succeeds")
            .map(|record| record.snapshot_id)
    }

    fn seed_hosted_object_access(&self, account_id: &str, raw_session_token: &str) {
        let mut metadata =
            SqliteMetadataStore::open_file(&self.metadata_db).expect("metadata opens");
        let proof = create_account_ownership_proof(AccountOwnershipProofInput {
            account_id,
            provider_kind: "oidc-dev",
            provider_issuer: "https://issuer.devbox.local",
            provider_subject: "provider-subject-live",
            verified_email: Some("user@example.com"),
            verified_domain: Some("example.com"),
            proof_issued_at: "2026-06-19T10:00:00Z",
            proof_expires_at_unix: 4_000_000_000,
        })
        .expect("proof creates");
        metadata
            .upsert_account_ownership_proof(proof.clone())
            .expect("proof upserts");
        metadata
            .upsert_account_session(
                create_account_session(
                    &proof,
                    raw_session_token,
                    "2026-06-19T10:01:00Z",
                    101,
                    4_000_000_000,
                )
                .expect("session creates"),
            )
            .expect("session upserts");
        metadata
            .upsert_project(UpsertProjectRequest {
                account_id: account_id.to_string(),
                project_id: self.project_id(),
                display_name: "project".to_string(),
                root_hint: path_string(&self.project),
                project_kind: "local".to_string(),
                updated_at: "2026-06-19T10:02:00Z".to_string(),
            })
            .expect("project upserts");
        metadata
            .upsert_managed_object_credential_lease(ManagedObjectCredentialLeaseRequest {
                account_id: account_id.to_string(),
                project_id: Some(self.project_id()),
                lease_id: "lease-alpha".to_string(),
                provider_kind: ManagedObjectProviderKind::R2,
                endpoint: "https://account.r2.cloudflarestorage.com".to_string(),
                bucket: "devbox-alpha".to_string(),
                region: "auto".to_string(),
                prefix: Some(format!(
                    "accounts/{}/projects/{}",
                    account_id,
                    self.project_id()
                )),
                credential_reference: "mock-managed-object-ref:lease-alpha:generation-0"
                    .to_string(),
                credential_fingerprint: None,
                capabilities: vec![
                    ManagedObjectCapability::Read,
                    ManagedObjectCapability::Write,
                    ManagedObjectCapability::Head,
                    ManagedObjectCapability::List,
                ],
                issued_at: "2026-06-19T10:03:00Z".to_string(),
                expires_at_unix: 4_000_000_000,
                rotation_generation: 0,
            })
            .expect("lease upserts");
    }

    fn start_hosted_object_server(&self) -> HostedServer {
        fs::create_dir_all(&self.hosted_object_root).expect("hosted object root creates");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("hosted listener binds");
        listener
            .set_nonblocking(true)
            .expect("listener set nonblocking");
        let addr = listener.local_addr().expect("listener addr reads");
        let metadata_db = self.metadata_db.clone();
        let object_root = self.hosted_object_root.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime creates");
            runtime.block_on(async move {
                let mut config = HostedApiConfig::local_dev();
                config.object_access_broker =
                    ManagedObjectAccessBrokerConfig::server_managed_local(
                        object_root.display().to_string(),
                    )
                    .expect("broker config validates");
                let store = SqliteMetadataStore::open_file(metadata_db).expect("metadata opens");
                let listener =
                    tokio::net::TcpListener::from_std(listener).expect("tokio listener wraps");
                axum::serve(listener, app_with_config(store, config))
                    .await
                    .expect("hosted object server runs");
            });
        });
        let server = HostedServer {
            url: format!("http://{addr}"),
        };
        for _ in 0..50 {
            if ureq::get(&format!("{}/health", server.url)).call().is_ok() {
                return server;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("hosted object server did not become ready");
    }

    fn hosted_sync_args(
        &self,
        db_path: &Path,
        cache_root: &Path,
        api: &str,
        account_id: &str,
        push: bool,
    ) -> Vec<String> {
        let mut args = vec![
            "sync".to_string(),
            "--db".to_string(),
            path_string(db_path),
            "--cache".to_string(),
            path_string(cache_root),
            "--remote-kind".to_string(),
            "hosted".to_string(),
            "--object-access-api".to_string(),
            api.to_string(),
            "--object-access-lease".to_string(),
            "lease-alpha".to_string(),
            "--object-access-session-token-env".to_string(),
            "DEVBOX_TEST_SESSION_TOKEN".to_string(),
            "--metadata-mode".to_string(),
            "mock-dev-sqlite".to_string(),
            "--metadata-db".to_string(),
            path_string(&self.metadata_db),
            "--metadata-account".to_string(),
            account_id.to_string(),
            "--metadata-project".to_string(),
            self.project_id(),
        ];
        if push {
            args.push("--push".to_string());
        } else {
            args.push("--pull".to_string());
        }
        args.push("--once".to_string());
        args.push(path_string(&self.project));
        args
    }

    fn hosted_pull_args(&self, api: &str, account_id: &str) -> Vec<String> {
        let mut args = self.hosted_sync_args(
            &self.receiver_db,
            &self.receiver_cache,
            api,
            account_id,
            false,
        );
        let last = args.len() - 1;
        args[last] = path_string(&self.target);
        let insert_at = args.len() - 1;
        args.insert(insert_at, "--to".to_string());
        args.insert(insert_at + 1, path_string(&self.target));
        args.insert(insert_at + 2, "--apply".to_string());
        args
    }
}

struct HostedServer {
    url: String,
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

fn run_devbox_daemon_vec_with_env(args: Vec<String>, envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devbox-daemon"));
    command.args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    command
        .env_remove("DEVBOX_R2_ACCESS_KEY_ID")
        .env_remove("DEVBOX_R2_SECRET_ACCESS_KEY")
        .env_remove("AWS_ACCESS_KEY_ID")
        .env_remove("AWS_SECRET_ACCESS_KEY")
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
