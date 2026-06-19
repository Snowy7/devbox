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
fn placeholder_commands_are_stable_and_successful() {
    for command in [
        "track",
        "status",
        "history",
        "checkpoint",
        "restore",
        "sync",
        "clone",
    ] {
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
