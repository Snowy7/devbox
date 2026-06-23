use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[test]
fn help_separates_product_commands_from_alpha_compatibility() {
    let output = run_devbox_with_env([], ["--help"]);

    assert_success(&output);
    let help_stdout = stdout(&output);
    assert!(help_stdout.contains("Product commands:"));
    assert!(help_stdout.contains("  login"));
    assert!(help_stdout.contains("  share"));
    assert!(help_stdout.contains("  doctor"));
    assert!(help_stdout.contains("Advanced compatibility commands:"));
    assert!(help_stdout.contains("  snapshot"));
    assert_product_output_is_clean(&help_stdout);

    for args in [
        ["share", "--help"],
        ["clone", "--help"],
        ["doctor", "--help"],
    ] {
        let output = run_devbox_with_env([], args);
        assert_success(&output);
        let stdout = stdout(&output);
        assert!(stdout.contains("Devbox keeps folders continuous across machines."));
        assert_product_output_is_clean(&stdout);
    }
}

#[test]
fn product_login_share_clone_status_pause_resume_and_unlink_flow() {
    let fixture = ProductCliFixture::new("alice");
    fixture.write_source("README.md", "one\n");
    fixture.write_source(".git/config", "[core]\nrepositoryformatversion = 0\n");
    fixture.write_source("node_modules/left-pad/index.js", "module.exports = true;\n");

    let login = fixture.devbox([
        "login",
        "--local-dev-direct",
        "--api",
        &fixture.api.base_url(),
        "--device-name",
        "Desk",
    ]);
    assert_success(&login);
    let login_stdout = stdout(&login);
    assert!(login_stdout.contains("Logged in to Devbox"));
    assert!(login_stdout.contains("Token: not printed"));
    assert!(!login_stdout.contains("devbox-local-session"));

    let share = fixture.devbox(["share", path_str(&fixture.source), "--no-background-sync"]);
    assert_success(&share);
    let share_stdout = stdout(&share);
    assert!(share_stdout.contains("Shared folder: source"));
    assert!(share_stdout.contains("Sync: up to date"));
    assert!(share_stdout.contains("Live sync: not started"));
    assert_product_output_is_clean(&share_stdout);

    let clone_list = fixture.devbox(["clone"]);
    assert_success(&clone_list);
    assert!(stdout(&clone_list).contains("already on this machine"));

    let clone = fixture.devbox([
        "clone",
        "source",
        path_str(&fixture.target),
        "--no-background-sync",
    ]);
    assert_success(&clone);
    let clone_stdout = stdout(&clone);
    assert!(clone_stdout.contains("Cloned shared folder: source"));
    assert!(clone_stdout.contains("Sync: ready"));
    assert_eq!(
        fs::read_to_string(fixture.target.join("README.md")).expect("target readme reads"),
        "one\n"
    );
    assert!(!fixture.target.join(".git").exists());
    assert!(!fixture.target.join("node_modules").exists());
    assert_product_output_is_clean(&clone_stdout);

    fixture.write_source("README.md", "two\n");
    let push_source = fixture.devbox(["resume", path_str(&fixture.source), "--no-background-sync"]);
    assert_success(&push_source);
    let pull_target = fixture.devbox([
        "sync",
        "run-loop",
        path_str(&fixture.target),
        "--max-cycles",
        "1",
    ]);
    assert_success(&pull_target);
    assert_eq!(
        fs::read_to_string(fixture.target.join("README.md")).expect("target readme updates"),
        "two\n"
    );

    let status = fixture.devbox(["status"]);
    assert_success(&status);
    let status_stdout = stdout(&status);
    assert!(status_stdout.contains("Logged in: yes"));
    assert!(status_stdout.contains("Machine: Desk"));
    assert!(status_stdout.contains("Shared folders:"));
    assert_product_output_is_clean(&status_stdout);

    let doctor = fixture.devbox(["doctor"]);
    assert_success(&doctor);
    let doctor_stdout = stdout(&doctor);
    assert!(doctor_stdout.contains("Loom diagnostics:"));
    assert!(doctor_stdout.contains("loom doctor"));
    assert!(doctor_stdout.contains(path_str(&fixture.source)));

    let pause = fixture.devbox(["pause", path_str(&fixture.target)]);
    assert_success(&pause);
    assert!(stdout(&pause).contains("Files: left untouched"));
    assert!(fixture.target.join("README.md").exists());

    let resume = fixture.devbox(["resume", path_str(&fixture.target), "--no-background-sync"]);
    assert_success(&resume);
    assert!(stdout(&resume).contains("Resumed sync for source"));

    let unlink = fixture.devbox(["unlink", path_str(&fixture.target)]);
    assert_success(&unlink);
    let unlink_stdout = stdout(&unlink);
    assert!(unlink_stdout.contains("Unlinked shared folder: source"));
    assert!(unlink_stdout.contains("Files: left untouched"));
    assert!(fixture.target.join("README.md").exists());
    assert!(fixture.target.join(".loom").exists());
}

#[test]
fn product_sparse_folder_commands_are_intent_based_and_safe() {
    let fixture = ProductCliFixture::new("sparse");
    fixture.write_source("README.md", "hello\n");
    fixture.write_source("src/main.rs", "fn main() {}\n");
    fixture.write_source("config/app.toml", "debug=1\n");
    fixture.write_source("big.bin", &"x".repeat(80));

    assert_success(&fixture.devbox([
        "login",
        "--local-dev-direct",
        "--api",
        &fixture.api.base_url(),
        "--device-name",
        "Desk",
    ]));
    assert_success(&fixture.devbox(["share", path_str(&fixture.source), "--no-background-sync"]));

    let clone = fixture.devbox([
        "clone",
        "source",
        path_str(&fixture.target),
        "--sparse",
        "--no-background-sync",
    ]);
    assert_success(&clone);
    let clone_stdout = stdout(&clone);
    assert!(clone_stdout.contains("Files: available on demand"));
    assert_product_output_is_clean(&clone_stdout);
    assert!(fixture.target.join(".loom").is_dir());
    assert!(!fixture.target.join("README.md").exists());

    let sparse_status = fixture.devbox(["status", path_str(&fixture.target)]);
    assert_success(&sparse_status);
    let sparse_status_stdout = stdout(&sparse_status);
    assert!(sparse_status_stdout.contains("Hydrated: 0 files"));
    assert!(sparse_status_stdout.contains("Cloud-only: 4 files"));
    assert!(sparse_status_stdout.contains("Pending upload: 0 files"));
    assert_product_output_is_clean(&sparse_status_stdout);

    let hydrate = fixture.devbox(["hydrate", path_str(&fixture.target.join("README.md"))]);
    assert_success(&hydrate);
    let hydrate_stdout = stdout(&hydrate);
    assert!(hydrate_stdout.contains("Hydrated: README.md"));
    assert_product_output_is_clean(&hydrate_stdout);
    assert_eq!(
        fs::read_to_string(fixture.target.join("README.md")).expect("readme hydrated"),
        "hello\n"
    );

    let warm = fixture.devbox(["warm", path_str(&fixture.target), "--max-bytes", "20"]);
    assert_success(&warm);
    let warm_stdout = stdout(&warm);
    assert!(warm_stdout.contains("Selected: 3 files"));
    assert!(warm_stdout.contains("Already local: 6 bytes"));
    assert_product_output_is_clean(&warm_stdout);
    assert!(fixture.target.join("src/main.rs").exists());
    assert!(fixture.target.join("config/app.toml").exists());
    assert!(!fixture.target.join("big.bin").exists());

    let keep = fixture.devbox(["keep", path_str(&fixture.target.join("README.md"))]);
    assert_success(&keep);
    let keep_stdout = stdout(&keep);
    assert!(keep_stdout.contains("Kept for offline: README.md"));
    assert!(keep_stdout.contains("Keep prevents cleanup"));
    assert_product_output_is_clean(&keep_stdout);

    fs::write(fixture.target.join("src/main.rs"), "dirty local change\n")
        .expect("dirty source writes");
    let free_space = fixture.devbox(["free-space", path_str(&fixture.target), "--max-bytes", "0"]);
    assert_success(&free_space);
    let free_space_stdout = stdout(&free_space);
    assert!(free_space_stdout.contains("Removed: 1 files"));
    assert!(free_space_stdout.contains("Skipped: 1 kept, 1 changed locally"));
    assert_product_output_is_clean(&free_space_stdout);
    assert!(fixture.target.join("README.md").exists());
    assert_eq!(
        fs::read_to_string(fixture.target.join("src/main.rs")).expect("dirty source remains"),
        "dirty local change\n"
    );
    assert!(!fixture.target.join("config/app.toml").exists());

    let status = fixture.devbox(["status", path_str(&fixture.target)]);
    assert_success(&status);
    let status_stdout = stdout(&status);
    assert!(status_stdout.contains("Hydrated: 1 files"));
    assert!(status_stdout.contains("Cloud-only: 2 files"));
    assert!(status_stdout.contains("Partial: 1 files"));
    assert!(status_stdout.contains("Changed locally: 1 files"));
    assert!(status_stdout.contains("Kept offline: 1 files"));
    assert_product_output_is_clean(&status_stdout);
}

#[test]
fn product_free_space_refuses_without_backed_up_copy() {
    let fixture = ProductCliFixture::new("free-space-proof");
    fixture.write_source("README.md", "local backed-up proof\n");

    assert_success(&fixture.devbox([
        "login",
        "--local-dev-direct",
        "--api",
        &fixture.api.base_url(),
        "--device-name",
        "Desk",
    ]));
    assert_success(&fixture.devbox(["share", path_str(&fixture.source), "--no-background-sync"]));
    fs::remove_dir_all(fixture.api_root.join("objects")).expect("hosted objects removed");

    let free_space = fixture.devbox(["free-space", path_str(&fixture.source), "--max-bytes", "0"]);

    assert_failure(&free_space);
    assert!(stderr(&free_space).contains("not safely backed up"));
    assert_eq!(
        fs::read_to_string(fixture.source.join("README.md")).expect("source remains"),
        "local backed-up proof\n"
    );
}

#[test]
fn unauthenticated_share_and_clone_fail_without_touching_files() {
    let fixture = ProductCliFixture::new("unauthenticated");
    fixture.write_source("README.md", "safe\n");

    let share = fixture.devbox(["share", path_str(&fixture.source), "--no-background-sync"]);
    assert_failure(&share);
    assert!(stderr(&share).contains("not logged in"));
    assert_eq!(
        fs::read_to_string(fixture.source.join("README.md")).expect("source file remains"),
        "safe\n"
    );

    let clone = fixture.devbox(["clone", "source", path_str(&fixture.target)]);
    assert_failure(&clone);
    assert!(stderr(&clone).contains("not logged in"));
    assert!(!fixture.target.exists());
}

#[test]
fn local_dev_direct_login_refuses_non_loopback_api_by_default() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = dir.path().join("config");

    let login = run_devbox_with_env(
        [("DEVBOX_CONFIG_DIR", path_str(&config))],
        [
            "login",
            "--local-dev-direct",
            "--api",
            "https://api.devbox.example",
            "--account",
            "alice",
            "--device-name",
            "Desk",
        ],
    );

    assert_failure(&login);
    let stderr = stderr(&login);
    assert!(stderr.contains("--local-dev-direct is local-dev only"));
    assert!(stderr.contains("loopback --api URL"));
}

#[test]
fn secret_blocking_still_applies_through_product_share() {
    let fixture = ProductCliFixture::new("secret");
    let raw_secret = "sk-abcdefghijklmnopqrstuvwxyzABCDEFGH123456";
    fixture.write_source("README.md", "safe\n");
    fixture.write_source("secrets.env", &format!("OPENAI_API_KEY={raw_secret}\n"));

    assert_success(&fixture.devbox([
        "login",
        "--local-dev-direct",
        "--api",
        &fixture.api.base_url(),
        "--device-name",
        "Desk",
    ]));
    let share = fixture.devbox(["share", path_str(&fixture.source), "--no-background-sync"]);

    assert_failure(&share);
    let combined = format!("{}\n{}", stdout(&share), stderr(&share));
    assert!(combined.contains("blocked secret pattern at line 1"));
    assert!(combined.contains("evidence: sk-<redacted>"));
    assert!(combined.contains("Remove it or exclude the file"));
    assert!(!combined.contains(raw_secret));
    assert_product_output_is_clean(&combined);
}

#[test]
fn invalid_session_resume_asks_user_to_login_without_internal_terms() {
    let fixture = ProductCliFixture::new("expired-session");
    fixture.write_source("README.md", "safe\n");

    assert_success(&fixture.devbox([
        "login",
        "--local-dev-direct",
        "--api",
        &fixture.api.base_url(),
        "--device-name",
        "Desk",
    ]));
    assert_success(&fixture.devbox(["share", path_str(&fixture.source), "--no-background-sync"]));
    fixture.replace_session_token("invalid-session-token");

    let resume = fixture.devbox(["resume", path_str(&fixture.source), "--no-background-sync"]);
    assert_failure(&resume);
    let combined = format!("{}\n{}", stdout(&resume), stderr(&resume));
    assert!(combined.contains("run devbox login again"));
    assert_product_output_is_clean(&combined);
}

#[test]
fn another_account_cannot_clone_a_protected_shared_folder() {
    let dir = tempfile::tempdir().expect("temp dir");
    let api = devbox_api::spawn_local_test_server(dir.path().join("api")).expect("api starts");
    let alice_config = dir.path().join("alice-config");
    let bob_config = dir.path().join("bob-config");
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    fs::create_dir_all(&source).expect("source creates");
    fs::write(source.join("README.md"), "private\n").expect("source writes");

    assert_success(&run_devbox_with_env(
        [("DEVBOX_CONFIG_DIR", path_str(&alice_config))],
        [
            "login",
            "--local-dev-direct",
            "--api",
            &api.base_url(),
            "--account",
            "alice",
            "--device-name",
            "Alice machine",
        ],
    ));
    assert_success(&run_devbox_with_env(
        [("DEVBOX_CONFIG_DIR", path_str(&alice_config))],
        ["share", path_str(&source), "--no-background-sync"],
    ));
    assert_success(&run_devbox_with_env(
        [("DEVBOX_CONFIG_DIR", path_str(&bob_config))],
        [
            "login",
            "--local-dev-direct",
            "--api",
            &api.base_url(),
            "--account",
            "bob",
            "--device-name",
            "Bob machine",
        ],
    ));

    let bob_clone = run_devbox_with_env(
        [("DEVBOX_CONFIG_DIR", path_str(&bob_config))],
        ["clone", "source", path_str(&target), "--no-background-sync"],
    );
    assert_failure(&bob_clone);
    assert!(stderr(&bob_clone).contains("was not found"));
    assert!(!target.exists());
}

struct ProductCliFixture {
    _dir: tempfile::TempDir,
    api: devbox_api::LocalApiServer,
    api_root: PathBuf,
    config: PathBuf,
    source: PathBuf,
    target: PathBuf,
}

impl ProductCliFixture {
    fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let api_root = dir.path().join("api");
        let api = devbox_api::spawn_local_test_server(&api_root).expect("api starts");
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::create_dir_all(&source).expect("source creates");
        Self {
            config: dir.path().join(format!("{name}-config")),
            api_root,
            _dir: dir,
            api,
            source,
            target,
        }
    }

    fn write_source(&self, path: &str, content: &str) {
        let path = self.source.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent creates");
        }
        fs::write(path, content).expect("source file writes");
    }

    fn devbox<const N: usize>(&self, args: [&str; N]) -> Output {
        run_devbox_with_env([("DEVBOX_CONFIG_DIR", path_str(&self.config))], args)
    }

    fn replace_session_token(&self, token: &str) {
        let path = self.config.join("config.json");
        let mut config: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("config reads"))
                .expect("config parses");
        config["session_token"] = serde_json::Value::String(token.to_string());
        fs::write(
            &path,
            serde_json::to_vec_pretty(&config).expect("config serializes"),
        )
        .expect("config writes");
    }
}

fn run_devbox_with_env<const E: usize, const N: usize>(
    envs: [(&str, &str); E],
    args: [&str; N],
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_devbox"));
    command.args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("devbox command runs")
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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("test path is UTF-8")
}

fn assert_product_output_is_clean(output: &str) {
    for hidden_word in ["pack", "cursor", "remote", "devbox://", "loom"] {
        assert!(
            !output.to_ascii_lowercase().contains(hidden_word),
            "product output exposed {hidden_word}: {output}"
        );
    }
}
