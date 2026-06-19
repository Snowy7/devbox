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
    for command in ["checkpoint", "restore", "sync", "clone"] {
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
    assert!(stdout.contains("Status: not implemented yet"));
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
