use std::process::Command;

#[test]
fn mock_verified_bootstrap_and_proof_check_never_print_or_persist_raw_token() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("devbox.sqlite3");
    let raw_token = "raw-dev-session-token-should-not-appear";
    let devbox = env!("CARGO_BIN_EXE_devbox");

    let init = Command::new(devbox)
        .args([
            "init",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--device-name",
            "Test laptop",
        ])
        .output()
        .expect("init runs");
    assert!(init.status.success(), "{}", stderr(&init));

    let bootstrap = Command::new(devbox)
        .args([
            "auth",
            "mock-verified-bootstrap",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--provider-kind",
            "oidc-dev",
            "--provider-issuer",
            "https://issuer.devbox.local",
            "--provider-subject",
            "provider-subject-123",
            "--verified-email",
            "user@example.com",
            "--session-token",
            raw_token,
            "--ttl-seconds",
            "3600",
        ])
        .output()
        .expect("bootstrap runs");
    assert!(bootstrap.status.success(), "{}", stderr(&bootstrap));
    let bootstrap_stdout = stdout(&bootstrap);
    assert!(bootstrap_stdout.contains("Mock verified account boundary bootstrapped"));
    assert!(bootstrap_stdout.contains("Session token: not printed"));
    assert!(bootstrap_stdout.contains("session token hash only"));
    assert!(!bootstrap_stdout.contains(raw_token));

    let proof_check = Command::new(devbox)
        .args([
            "auth",
            "proof-check",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--session-token",
            raw_token,
        ])
        .output()
        .expect("proof check runs");
    assert!(proof_check.status.success(), "{}", stderr(&proof_check));
    let proof_stdout = stdout(&proof_check);
    assert!(proof_stdout.contains("Auth proof check: active"));
    assert!(proof_stdout.contains("Session token: not printed"));
    assert!(!proof_stdout.contains(raw_token));

    let db_bytes = std::fs::read(&db_path).expect("db bytes read");
    let db_text = String::from_utf8_lossy(&db_bytes);
    assert!(!db_text.contains(raw_token));
}

#[test]
fn receiver_pairing_cli_flow_creates_fresh_receiver_db_without_key_leaks() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source_db = dir.path().join("source.sqlite3");
    let receiver_db = dir.path().join("receiver.sqlite3");
    let devbox = env!("CARGO_BIN_EXE_devbox");

    let init = Command::new(devbox)
        .args([
            "init",
            "--db",
            source_db.to_str().expect("source db path is utf8"),
            "--device-name",
            "Desk",
        ])
        .output()
        .expect("source init runs");
    assert!(init.status.success(), "{}", stderr(&init));

    let invite = Command::new(devbox)
        .args([
            "devices",
            "invite",
            "--db",
            source_db.to_str().expect("source db path is utf8"),
        ])
        .output()
        .expect("invite runs");
    assert!(invite.status.success(), "{}", stderr(&invite));
    let invite_stdout = stdout(&invite);
    let token = line_value(&invite_stdout, "Pairing token: ");
    assert!(token.starts_with("devbox-pair-v1:"));

    let join = Command::new(devbox)
        .env("DEVBOX_PAIRING_TOKEN", &token)
        .args([
            "devices",
            "join",
            "--db",
            receiver_db.to_str().expect("receiver db path is utf8"),
            "--token-env",
            "DEVBOX_PAIRING_TOKEN",
            "--device-name",
            "Laptop",
        ])
        .output()
        .expect("join runs");
    assert!(join.status.success(), "{}", stderr(&join));
    assert!(receiver_db.exists());
    let join_stdout = stdout(&join);
    assert!(!join_stdout.contains(&token));
    let join_request = export_value(&join_stdout, "DEVBOX_PAIRING_JOIN_REQUEST");

    let pending_upload = Command::new(devbox)
        .args([
            "sync",
            "upload",
            "--db",
            receiver_db.to_str().expect("receiver db path is utf8"),
            "--cache",
            dir.path()
                .join("receiver-cache")
                .to_str()
                .expect("cache path is utf8"),
            "--remote",
            dir.path()
                .join("remote")
                .to_str()
                .expect("remote path is utf8"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ])
        .output()
        .expect("pending upload runs");
    assert!(
        !pending_upload.status.success(),
        "{}",
        stdout(&pending_upload)
    );
    let pending_upload_stderr = stderr(&pending_upload);
    assert!(
        pending_upload_stderr.contains("local identity is pending pairing completion"),
        "{pending_upload_stderr}"
    );

    let pending_download = Command::new(devbox)
        .args([
            "sync",
            "download",
            "--db",
            receiver_db.to_str().expect("receiver db path is utf8"),
            "--cache",
            dir.path()
                .join("receiver-cache")
                .to_str()
                .expect("cache path is utf8"),
            "--remote",
            dir.path()
                .join("remote")
                .to_str()
                .expect("remote path is utf8"),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ])
        .output()
        .expect("pending download runs");
    assert!(
        !pending_download.status.success(),
        "{}",
        stdout(&pending_download)
    );
    let pending_download_stderr = stderr(&pending_download);
    assert!(
        pending_download_stderr.contains("local identity is pending pairing completion"),
        "{pending_download_stderr}"
    );

    for command in ["publish-snapshot", "import-snapshot", "materialize"] {
        let mut pending = Command::new(devbox);
        pending.args([
            "sync",
            command,
            "--db",
            receiver_db.to_str().expect("receiver db path is utf8"),
            "--cache",
            dir.path()
                .join("receiver-cache")
                .to_str()
                .expect("cache path is utf8"),
            "--remote",
            dir.path()
                .join("remote")
                .to_str()
                .expect("remote path is utf8"),
        ]);
        if command == "materialize" {
            pending.args([
                "--to",
                dir.path()
                    .join("target")
                    .to_str()
                    .expect("target path is utf8"),
            ]);
        }
        let pending = pending
            .arg("snapshot-pending")
            .output()
            .expect("pending snapshot sync runs");
        assert!(!pending.status.success(), "{}", stdout(&pending));
        let pending_stderr = stderr(&pending);
        assert!(
            pending_stderr.contains("local identity is pending pairing completion"),
            "{command}: {pending_stderr}"
        );
    }

    let approve = Command::new(devbox)
        .env("DEVBOX_PAIRING_TOKEN", &token)
        .env("DEVBOX_PAIRING_JOIN_REQUEST", &join_request)
        .args([
            "devices",
            "approve-join",
            "--db",
            source_db.to_str().expect("source db path is utf8"),
            "--token-env",
            "DEVBOX_PAIRING_TOKEN",
            "--join-request-env",
            "DEVBOX_PAIRING_JOIN_REQUEST",
            "--device-name",
            "Laptop",
        ])
        .output()
        .expect("approve join runs");
    assert!(approve.status.success(), "{}", stderr(&approve));
    let approve_stdout = stdout(&approve);
    assert!(!approve_stdout.contains(&token));
    assert!(!approve_stdout.contains(&join_request));
    let completion = export_value(&approve_stdout, "DEVBOX_PAIRING_COMPLETION");

    let complete = Command::new(devbox)
        .env("DEVBOX_PAIRING_COMPLETION", &completion)
        .args([
            "devices",
            "complete",
            "--db",
            receiver_db.to_str().expect("receiver db path is utf8"),
            "--completion-env",
            "DEVBOX_PAIRING_COMPLETION",
        ])
        .output()
        .expect("complete runs");
    assert!(complete.status.success(), "{}", stderr(&complete));
    let complete_stdout = stdout(&complete);
    assert!(complete_stdout.contains("Pairing completed"));
    assert!(
        complete_stdout.contains("Receiver can import/materialize without --mock-key-source-db")
    );
    assert!(!complete_stdout.contains(&completion));
}

#[test]
fn managed_object_credential_lease_cli_never_prints_or_persists_raw_cloud_material() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("metadata.sqlite3");
    let session_token = "raw-managed-session-token-should-not-appear";
    let raw_access_key = "aws_access_key_id_should_not_appear";
    let raw_secret_key = "aws_secret_access_key_should_not_appear";
    let raw_provider_token = "cloudflare_api_token_should_not_appear";
    let raw_credential_hash = "credential_hash_should_not_appear";
    let devbox = env!("CARGO_BIN_EXE_devbox");

    let create = Command::new(devbox)
        .args([
            "metadata",
            "credential-lease",
            "mock-create",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--session-token",
            session_token,
            "--verified-email",
            "user@example.com",
            "--project",
            "project-devbox",
            "--lease",
            "lease-alpha",
            "--provider-kind",
            "r2",
            "--endpoint",
            "https://account.r2.cloudflarestorage.com",
            "--bucket",
            "devbox-alpha",
            "--prefix",
            "accounts/account-managed-user-example-com/projects/project-devbox",
            "--ttl-seconds",
            "3600",
        ])
        .output()
        .expect("lease create runs");
    assert!(create.status.success(), "{}", stderr(&create));
    let create_stdout = stdout(&create);
    assert!(create_stdout.contains("Managed object credential lease: mock-created"));
    assert!(create_stdout
        .contains("Credential reference: mock-managed-object-ref:lease-alpha:generation-0"));
    assert!(create_stdout.contains("Boundary: no live Cloudflare/AWS provisioning"));

    let check = Command::new(devbox)
        .args([
            "metadata",
            "credential-lease",
            "check",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--session-token",
            session_token,
            "--project",
            "project-devbox",
            "--lease",
            "lease-alpha",
            "--require-capabilities",
            "read,head",
        ])
        .output()
        .expect("lease check runs");
    assert!(check.status.success(), "{}", stderr(&check));
    assert!(stdout(&check).contains("Managed object credential lease: active"));

    let rotate = Command::new(devbox)
        .args([
            "metadata",
            "credential-lease",
            "rotate",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--session-token",
            session_token,
            "--project",
            "project-devbox",
            "--lease",
            "lease-alpha",
        ])
        .output()
        .expect("lease rotate runs");
    assert!(rotate.status.success(), "{}", stderr(&rotate));
    assert!(stdout(&rotate).contains("Generation: 1"));

    let revoke = Command::new(devbox)
        .args([
            "metadata",
            "credential-lease",
            "revoke",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--session-token",
            session_token,
            "--project",
            "project-devbox",
            "--lease",
            "lease-alpha",
        ])
        .output()
        .expect("lease revoke runs");
    assert!(revoke.status.success(), "{}", stderr(&revoke));
    assert!(stdout(&revoke).contains("Managed object credential lease: revoked"));

    let combined_output = [
        stdout(&create),
        stderr(&create),
        stdout(&check),
        stderr(&check),
        stdout(&rotate),
        stderr(&rotate),
        stdout(&revoke),
        stderr(&revoke),
    ]
    .join("\n");
    for forbidden in [
        session_token,
        raw_access_key,
        raw_secret_key,
        raw_provider_token,
        raw_credential_hash,
    ] {
        assert!(!combined_output.contains(forbidden));
    }

    let db_bytes = std::fs::read(&db_path).expect("db bytes read");
    let db_text = String::from_utf8_lossy(&db_bytes);
    for forbidden in [
        session_token,
        raw_access_key,
        raw_secret_key,
        raw_provider_token,
        raw_credential_hash,
    ] {
        assert!(!db_text.contains(forbidden));
    }
}

#[test]
fn managed_object_credential_lease_cli_rejects_project_scope_sentinel() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("metadata.sqlite3");
    let session_token = "raw-managed-session-token-should-not-appear";
    let devbox = env!("CARGO_BIN_EXE_devbox");

    let account_wide = Command::new(devbox)
        .args([
            "metadata",
            "credential-lease",
            "mock-create",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--session-token",
            session_token,
            "--verified-email",
            "user@example.com",
            "--lease",
            "lease-account-wide",
            "--provider-kind",
            "r2",
            "--endpoint",
            "https://account.r2.cloudflarestorage.com",
            "--bucket",
            "devbox-alpha",
            "--ttl-seconds",
            "3600",
        ])
        .output()
        .expect("account-wide lease create runs");
    assert!(account_wide.status.success(), "{}", stderr(&account_wide));
    assert!(stdout(&account_wide).contains("Project id: -"));

    let sentinel = Command::new(devbox)
        .args([
            "metadata",
            "credential-lease",
            "mock-create",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--session-token",
            session_token,
            "--verified-email",
            "user@example.com",
            "--project",
            "*",
            "--lease",
            "lease-account-wide",
            "--provider-kind",
            "r2",
            "--endpoint",
            "https://account.r2.cloudflarestorage.com",
            "--bucket",
            "devbox-alpha",
        ])
        .output()
        .expect("sentinel lease create runs");
    assert!(!sentinel.status.success(), "{}", stdout(&sentinel));
    assert!(stderr(&sentinel)
        .contains("project id '*' is reserved for account-wide managed object credential leases"));

    let check = Command::new(devbox)
        .args([
            "metadata",
            "credential-lease",
            "check",
            "--db",
            db_path.to_str().expect("db path is utf8"),
            "--session-token",
            session_token,
            "--lease",
            "lease-account-wide",
            "--require-capabilities",
            "read",
        ])
        .output()
        .expect("account-wide lease check runs");
    assert!(check.status.success(), "{}", stderr(&check));
    let check_stdout = stdout(&check);
    assert!(check_stdout.contains("Managed object credential lease: active"));
    assert!(check_stdout.contains("Project id: -"));
    assert!(!check_stdout.contains(session_token));
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn line_value(output: &str, prefix: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .expect("line with prefix exists")
        .to_string()
}

fn export_value(output: &str, name: &str) -> String {
    let prefix = format!("export {name}='");
    output
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|value| value.strip_suffix('\''))
        .expect("export line exists")
        .to_string()
}
