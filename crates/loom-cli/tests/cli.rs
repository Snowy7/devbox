use std::process::{Command, Output};

#[test]
fn help_lists_the_mvp_commands() {
    let output = run_loom(["--help"]);

    assert_success(&output);
    let stdout = stdout(&output);
    for command in [
        "track",
        "status",
        "history",
        "diff",
        "checkpoint",
        "restore",
        "sync",
        "clone",
    ] {
        assert!(stdout.contains(command), "{command} missing from help");
    }
}

#[test]
fn remaining_placeholder_commands_are_stable_and_successful() {
    for command in ["sync", "clone"] {
        let output = run_loom([command]);

        assert_success(&output);
        let stdout = stdout(&output);
        assert!(stdout.contains(&format!("loom {command}: not implemented yet")));
        assert!(stdout.contains("Planned behavior:"));
    }
}

#[test]
fn command_help_prints_usage() {
    let output = run_loom(["checkpoint", "--help"]);

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("Usage: loom checkpoint [FOLDER] -m <MESSAGE>"));
    assert!(stdout.contains("Status: implemented for the local offline engine"));
}

#[test]
fn local_engine_tracks_statuses_and_lists_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = dir.path().join("fixture");
    std::fs::create_dir_all(&fixture).expect("fixture creates");
    std::fs::write(fixture.join("README.md"), "hello\n").expect("readme writes");
    std::fs::create_dir_all(fixture.join("node_modules/left-pad")).expect("generated dir creates");
    std::fs::write(
        fixture.join("node_modules/left-pad/index.js"),
        "module.exports = true;\n",
    )
    .expect("generated file writes");

    let track = run_loom(["track", fixture.to_str().expect("fixture path is UTF-8")]);
    assert_success(&track);
    let track_stdout = stdout(&track);
    assert!(track_stdout.contains("Initialized Loom tracking"));
    assert!(track_stdout.contains("Captured folder revision"));
    assert!(track_stdout.contains("Policy: 2 ignored"));

    std::fs::write(fixture.join("README.md"), "hello again\n").expect("readme edits");
    std::fs::write(fixture.join("src.txt"), "new\n").expect("new file writes");

    let status = run_loom(["status", fixture.to_str().expect("fixture path is UTF-8")]);
    assert_success(&status);
    let status_stdout = stdout(&status);
    assert!(status_stdout.contains("Captured new folder revision"));
    assert!(status_stdout.contains("Changes: 1 created, 1 modified"));

    let history = run_loom(["history", fixture.to_str().expect("fixture path is UTF-8")]);
    assert_success(&history);
    let history_stdout = stdout(&history);
    assert!(history_stdout.contains("Folder revision history:"));
    assert_eq!(history_stdout.matches("folder-revision-b3-").count(), 3);
}

#[test]
fn checkpoints_diff_and_restore_make_local_history_useful() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = dir.path().join("fixture");
    std::fs::create_dir_all(&fixture).expect("fixture creates");
    std::fs::write(fixture.join("README.md"), "before\n").expect("readme writes");
    std::fs::create_dir_all(fixture.join(".git")).expect("git dir creates");
    std::fs::write(fixture.join(".git/config"), "local git metadata\n").expect("git writes");
    std::fs::create_dir_all(fixture.join("node_modules/left-pad")).expect("generated dir creates");
    std::fs::write(
        fixture.join("node_modules/left-pad/index.js"),
        "module.exports = true;\n",
    )
    .expect("generated file writes");

    assert_success(&run_loom([
        "track",
        fixture.to_str().expect("fixture path is UTF-8"),
    ]));

    let checkpoint = run_loom([
        "checkpoint",
        fixture.to_str().expect("fixture path is UTF-8"),
        "-m",
        "before change",
    ]);
    assert_success(&checkpoint);
    let checkpoint_stdout = stdout(&checkpoint);
    let checkpoint_id = value_after(&checkpoint_stdout, "Checkpoint: ");
    assert!(checkpoint_stdout.contains("Pinned: revision kept"));

    std::fs::write(fixture.join("README.md"), "after\n").expect("readme edits");
    std::fs::write(fixture.join("new.txt"), "new\n").expect("new file writes");

    let diff = run_loom(["diff", fixture.to_str().expect("fixture path is UTF-8")]);
    assert_success(&diff);
    let diff_stdout = stdout(&diff);
    assert!(diff_stdout.contains("Changes: 1 created, 1 modified, 0 deleted"));
    assert!(diff_stdout.contains("Created:"));
    assert!(diff_stdout.contains("new.txt"));
    assert!(diff_stdout.contains("Modified:"));
    assert!(diff_stdout.contains("README.md"));
    assert!(diff_stdout.contains("Ignored:"));
    assert!(diff_stdout.contains(".git"));

    let restore = run_loom([
        "restore",
        fixture.to_str().expect("fixture path is UTF-8"),
        &checkpoint_id,
    ]);
    assert_success(&restore);
    let restore_stdout = stdout(&restore);
    assert!(restore_stdout.contains("Restored: checkpoint"));
    assert!(restore_stdout.contains("Restore changes: 1 removed, 1 reverted, 0 restored"));
    assert_eq!(
        std::fs::read_to_string(fixture.join("README.md")).expect("readme reads"),
        "before\n"
    );
    assert!(!fixture.join("new.txt").exists());
    assert_eq!(
        std::fs::read_to_string(fixture.join(".git/config")).expect("git reads"),
        "local git metadata\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.join("node_modules/left-pad/index.js"))
            .expect("generated reads"),
        "module.exports = true;\n"
    );

    let history = run_loom(["history", fixture.to_str().expect("fixture path is UTF-8")]);
    assert_success(&history);
    let history_stdout = stdout(&history);
    assert!(history_stdout.contains("Checkpoints:"));
    assert!(history_stdout.contains("before change"));
    assert!(history_stdout.contains("pins=1"));
}

#[test]
fn restore_refuses_secret_blocked_working_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = dir.path().join("fixture");
    std::fs::create_dir_all(&fixture).expect("fixture creates");
    std::fs::write(fixture.join("README.md"), "before\n").expect("readme writes");

    assert_success(&run_loom([
        "track",
        fixture.to_str().expect("fixture path is UTF-8"),
    ]));
    let checkpoint = run_loom([
        "checkpoint",
        fixture.to_str().expect("fixture path is UTF-8"),
        "-m",
        "before secret",
    ]);
    assert_success(&checkpoint);
    let checkpoint_id = value_after(&stdout(&checkpoint), "Checkpoint: ");

    let raw_secret = ["sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456"].concat();
    std::fs::write(
        fixture.join("secrets.env"),
        format!("OPENAI_API_KEY={raw_secret}\n"),
    )
    .expect("secret writes");

    let restore = run_loom([
        "restore",
        fixture.to_str().expect("fixture path is UTF-8"),
        &checkpoint_id,
    ]);

    assert!(!restore.status.success());
    assert!(stderr(&restore).contains("secret-blocked"));
    assert!(fixture.join("secrets.env").exists());
}

#[test]
fn unknown_commands_fail_with_usage_hint() {
    let output = run_loom(["teleport"]);

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("unknown command 'teleport'"));
}

fn run_loom<const N: usize>(args: [&str; N]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_loom"))
        .args(args)
        .output()
        .expect("loom command runs")
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

fn value_after(output: &str, prefix: &str) -> String {
    output
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .expect("prefixed line exists")
        .trim()
        .to_string()
}
