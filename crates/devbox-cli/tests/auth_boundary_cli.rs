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

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
