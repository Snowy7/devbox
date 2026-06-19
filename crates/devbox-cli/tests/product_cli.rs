use std::process::{Command, Output};

#[test]
fn help_separates_product_commands_from_alpha_compatibility() {
    let output = run_devbox(["--help"]);

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("Product commands:"));
    assert!(stdout.contains("  login"));
    assert!(stdout.contains("  share"));
    assert!(stdout.contains("Alpha compatibility commands:"));
    assert!(stdout.contains("  snapshot"));
}

#[test]
fn product_commands_are_explicit_placeholders() {
    for command in [
        "login", "share", "clone", "manage", "pause", "resume", "unlink",
    ] {
        let output = run_devbox([command]);

        assert_success(&output);
        let stdout = stdout(&output);
        assert!(stdout.contains(&format!("devbox {command}: not implemented yet")));
        assert!(stdout.contains("Devbox configures accounts, machines, and shared folders"));
        assert!(stdout.contains("Loom owns folder state and sync semantics"));
    }
}

#[test]
fn status_defaults_to_product_language_but_keeps_db_compatibility_hint() {
    let output = run_devbox(["status"]);

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("devbox status: not implemented yet"));
    assert!(stdout.contains("shared folders, machines, and sync health"));
    assert!(stdout.contains("pass --db <PATH>"));
}

#[test]
fn product_command_help_names_the_future_shape() {
    let output = run_devbox(["share", "--help"]);

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("Usage: devbox share <FOLDER>"));
    assert!(stdout.contains("Status: not implemented yet"));
    assert!(stdout.contains("delegate folder state to Loom"));
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

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}
