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
