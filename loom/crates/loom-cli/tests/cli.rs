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
        "remote",
        "sync",
        "clone",
    ] {
        assert!(stdout.contains(command), "{command} missing from help");
    }
}

#[test]
fn remaining_placeholder_commands_are_stable_and_successful() {
    let output = run_loom(["remote"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("remote command requires"));
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
fn remote_sync_and_clone_move_folder_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::write(source.join("README.md"), "hello from source\n").expect("readme writes");
    std::fs::create_dir_all(source.join(".git")).expect("git dir creates");
    std::fs::write(source.join(".git/config"), "local git metadata\n").expect("git writes");
    std::fs::create_dir_all(source.join("node_modules/pkg")).expect("generated dir creates");
    std::fs::write(source.join("node_modules/pkg/index.js"), "generated\n")
        .expect("generated writes");

    assert_success(&run_loom(["track", source.to_str().expect("UTF-8 path")]));

    let remote_add = run_loom([
        "remote",
        "add",
        "local",
        remote.to_str().expect("UTF-8 path"),
        source.to_str().expect("UTF-8 path"),
    ]);
    assert_success(&remote_add);
    assert!(stdout(&remote_add).contains("Kind: local-fs"));

    let sync = run_loom(["sync", source.to_str().expect("UTF-8 path")]);
    assert_success(&sync);
    let sync_stdout = stdout(&sync);
    assert!(sync_stdout.contains("Synced revision:"));
    assert!(sync_stdout.contains("Pack objects: 1"));

    let clone = run_loom([
        "clone",
        remote.to_str().expect("UTF-8 path"),
        target.to_str().expect("UTF-8 path"),
    ]);
    assert_success(&clone);
    let clone_stdout = stdout(&clone);
    assert!(clone_stdout.contains("Cloned revision:"));
    assert_eq!(
        std::fs::read_to_string(target.join("README.md")).expect("readme reads"),
        "hello from source\n"
    );
    assert!(!target.join(".git").exists());
    assert!(!target.join("node_modules/pkg/index.js").exists());
    assert!(target.join(".loom").is_dir());
}

#[test]
fn devbox_hosted_remote_sync_and_clone_move_folder_state() {
    let dir = tempfile::tempdir().expect("temp dir");
    let api =
        devbox_api::spawn_local_test_server(dir.path().join("api")).expect("api server starts");
    let api_url = api.base_url();
    let source = dir.path().join("source");
    let target = dir.path().join("target");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::write(source.join("README.md"), "hello from hosted source\n").expect("readme writes");
    std::fs::create_dir_all(source.join(".git")).expect("git dir creates");
    std::fs::write(source.join(".git/config"), "local git metadata\n").expect("git writes");
    std::fs::create_dir_all(source.join("node_modules/pkg")).expect("generated dir creates");
    std::fs::write(source.join("node_modules/pkg/index.js"), "generated\n")
        .expect("generated writes");

    assert_success(&run_loom(["track", source.to_str().expect("UTF-8 path")]));

    let remote_add = run_loom([
        "remote",
        "add",
        "devbox",
        &api_url,
        source.to_str().expect("UTF-8 path"),
    ]);
    assert_success(&remote_add);
    let remote_add_stdout = stdout(&remote_add);
    assert!(remote_add_stdout.contains("Kind: devbox"));
    let clone_url = value_after(&remote_add_stdout, "Clone URL: ");

    let sync = run_loom(["sync", source.to_str().expect("UTF-8 path")]);
    assert_success(&sync);
    let sync_stdout = stdout(&sync);
    assert!(sync_stdout.contains("Remote: devbox"));
    assert!(sync_stdout.contains("Synced revision:"));
    assert!(sync_stdout.contains("Pack objects: 1"));

    let clone = run_loom(["clone", &clone_url, target.to_str().expect("UTF-8 path")]);
    assert_success(&clone);
    let clone_stdout = stdout(&clone);
    assert!(clone_stdout.contains("Cloned revision:"));
    assert_eq!(
        std::fs::read_to_string(target.join("README.md")).expect("readme reads"),
        "hello from hosted source\n"
    );
    assert!(!target.join(".git").exists());
    assert!(!target.join("node_modules/pkg/index.js").exists());
    assert!(target.join(".loom").is_dir());
}

#[test]
fn background_sync_start_stop_updates_materialized_target() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::write(source.join("README.md"), "before\n").expect("readme writes");

    assert_success(&run_loom(["track", source.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "remote",
        "add",
        "local",
        remote.to_str().expect("UTF-8 path"),
        source.to_str().expect("UTF-8 path"),
    ]));
    assert_success(&run_loom(["sync", source.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "clone",
        remote.to_str().expect("UTF-8 path"),
        target.to_str().expect("UTF-8 path"),
    ]));

    let source_start = run_loom_vec(vec![
        "sync".to_string(),
        "start".to_string(),
        source.to_str().expect("UTF-8 path").to_string(),
        "--debounce-ms".to_string(),
        "50".to_string(),
        "--poll-ms".to_string(),
        "50".to_string(),
    ]);
    assert_success(&source_start);
    let source_pid = value_after(&stdout(&source_start), "Daemon pid: ");
    let target_start = run_loom_vec(vec![
        "sync".to_string(),
        "start".to_string(),
        target.to_str().expect("UTF-8 path").to_string(),
        "--debounce-ms".to_string(),
        "50".to_string(),
        "--poll-ms".to_string(),
        "50".to_string(),
    ]);
    assert_success(&target_start);

    wait_for_status(&source, "running");
    wait_for_status(&target, "running");

    let duplicate_source_start = run_loom_vec(vec![
        "sync".to_string(),
        "start".to_string(),
        source.to_str().expect("UTF-8 path").to_string(),
        "--debounce-ms".to_string(),
        "50".to_string(),
        "--poll-ms".to_string(),
        "50".to_string(),
    ]);
    assert_success(&duplicate_source_start);
    let duplicate_stdout = stdout(&duplicate_source_start);
    assert!(duplicate_stdout.contains("Background sync: already running"));
    assert_eq!(value_after(&duplicate_stdout, "Daemon pid: "), source_pid);

    std::fs::write(source.join("README.md"), "after\n").expect("readme edits");
    wait_for_file_contents(&target.join("README.md"), "after\n");

    let source_status = run_loom(["sync", "status", source.to_str().expect("UTF-8 path")]);
    assert_success(&source_status);
    assert!(stdout(&source_status).contains("Daemon state: running"));

    assert_success(&run_loom([
        "sync",
        "stop",
        target.to_str().expect("UTF-8 path"),
    ]));
    assert_success(&run_loom([
        "sync",
        "stop",
        source.to_str().expect("UTF-8 path"),
    ]));
}

#[test]
fn sync_refuses_divergent_remote_cursor() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::write(source.join("README.md"), "one\n").expect("readme writes");

    assert_success(&run_loom(["track", source.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "remote",
        "add",
        "local",
        remote.to_str().expect("UTF-8 path"),
        source.to_str().expect("UTF-8 path"),
    ]));

    std::fs::create_dir_all(remote.join("cursors")).expect("cursor dir creates");
    std::fs::write(
        remote.join("cursors").join("shared-folder.txt"),
        "folder-revision-b3-other\n",
    )
    .expect("cursor writes");

    let sync = run_loom(["sync", source.to_str().expect("UTF-8 path")]);

    assert!(!sync.status.success());
    assert!(stderr(&sync).contains("diverged"));
}

#[test]
fn clone_refusal_leaves_existing_loom_target_unchanged() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::create_dir_all(&target).expect("target creates");
    std::fs::write(source.join("README.md"), "remote source\n").expect("source readme writes");
    std::fs::write(target.join("local.txt"), "local target\n").expect("target local writes");

    assert_success(&run_loom(["track", source.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "remote",
        "add",
        "local",
        remote.to_str().expect("UTF-8 path"),
        source.to_str().expect("UTF-8 path"),
    ]));
    let sync = run_loom(["sync", source.to_str().expect("UTF-8 path")]);
    assert_success(&sync);
    let remote_revision = value_after(&stdout(&sync), "Synced revision: ");

    assert_success(&run_loom(["track", target.to_str().expect("UTF-8 path")]));
    let metadata_dir = target.join(".loom").join("metadata");
    let shared_folder_before =
        std::fs::read_to_string(metadata_dir.join("shared_folder.tsv")).expect("shared reads");
    let file_versions_before =
        std::fs::read_to_string(metadata_dir.join("file_versions.tsv")).expect("files read");
    let revisions_before =
        std::fs::read_to_string(metadata_dir.join("revisions.tsv")).expect("revisions read");
    let object_count_before = count_files(&target.join(".loom").join("objects"));

    let clone = run_loom([
        "clone",
        remote.to_str().expect("UTF-8 path"),
        target.to_str().expect("UTF-8 path"),
    ]);

    assert!(!clone.status.success());
    assert!(stderr(&clone).contains("already contains a Loom store"));
    assert_eq!(
        std::fs::read_to_string(metadata_dir.join("shared_folder.tsv")).expect("shared rereads"),
        shared_folder_before
    );
    assert_eq!(
        std::fs::read_to_string(metadata_dir.join("file_versions.tsv")).expect("files reread"),
        file_versions_before
    );
    assert_eq!(
        std::fs::read_to_string(metadata_dir.join("revisions.tsv")).expect("revisions reread"),
        revisions_before
    );
    assert_eq!(
        std::fs::read_to_string(target.join("local.txt")).expect("local reads"),
        "local target\n"
    );
    assert_eq!(
        count_files(&target.join(".loom").join("objects")),
        object_count_before
    );
    assert!(!std::fs::read_to_string(metadata_dir.join("revisions.tsv"))
        .expect("revisions reread")
        .contains(&remote_revision));
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

fn run_loom_vec(args: Vec<String>) -> Output {
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

fn count_files(path: &std::path::Path) -> usize {
    let mut count = 0;
    let mut stack = vec![path.to_path_buf()];

    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(path).expect("directory reads") {
            let entry = entry.expect("directory entry reads");
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else {
                count += 1;
            }
        }
    }

    count
}

fn wait_for_status(folder: &std::path::Path, expected: &str) {
    for _ in 0..100 {
        let status = run_loom(["sync", "status", folder.to_str().expect("UTF-8 path")]);
        if status.status.success() && stdout(&status).contains(&format!("Daemon state: {expected}"))
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("daemon did not reach {expected} for {}", folder.display());
}

fn wait_for_file_contents(path: &std::path::Path, expected: &str) {
    for _ in 0..120 {
        if std::fs::read_to_string(path)
            .map(|contents| contents == expected)
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("{} did not become expected contents", path.display());
}
