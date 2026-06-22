use std::process::{Command, Output};

#[test]
fn help_lists_the_mvp_commands() {
    let output = run_loom(["--help"]);

    assert_success(&output);
    let stdout = stdout(&output);
    for command in [
        "doctor",
        "fsck",
        "object",
        "track",
        "status",
        "history",
        "diff",
        "checkpoint",
        "restore",
        "remote",
        "sync",
        "clone",
        "hydrate",
        "evict",
        "pin",
        "cache",
        "workspace",
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
fn object_verify_detects_corrupted_local_object_bytes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = dir.path().join("fixture");
    std::fs::create_dir_all(&fixture).expect("fixture creates");
    std::fs::write(fixture.join("README.md"), "hello\n").expect("readme writes");
    assert_success(&run_loom(["track", fixture.to_str().expect("UTF-8 path")]));

    let object_path = first_file_under(&fixture.join(".loom").join("objects"));
    std::fs::write(object_path, "corrupt\n").expect("object corrupts");

    let verify = run_loom(["object", "verify", fixture.to_str().expect("UTF-8 path")]);

    assert!(!verify.status.success());
    assert!(stdout(&verify).contains("ERROR object-hash-mismatch"));
    assert!(stderr(&verify).contains("object verification found"));
}

#[test]
fn fsck_detects_missing_local_object_cache_inconsistency() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = dir.path().join("fixture");
    std::fs::create_dir_all(&fixture).expect("fixture creates");
    std::fs::write(fixture.join("README.md"), "hello\n").expect("readme writes");
    assert_success(&run_loom(["track", fixture.to_str().expect("UTF-8 path")]));

    let object_path = first_file_under(&fixture.join(".loom").join("objects"));
    std::fs::remove_file(object_path).expect("object removes");

    let fsck = run_loom(["fsck", fixture.to_str().expect("UTF-8 path")]);

    assert!(!fsck.status.success());
    assert!(stdout(&fsck).contains("ERROR cache-entry-missing-local-object"));
    assert!(stderr(&fsck).contains("fsck found"));
}

#[test]
fn remote_check_detects_missing_remote_object_bytes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::write(source.join("README.md"), "hello remote\n").expect("readme writes");
    assert_success(&run_loom(["track", source.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "remote",
        "add",
        "local",
        remote.to_str().expect("UTF-8 path"),
        source.to_str().expect("UTF-8 path"),
    ]));
    assert_success(&run_loom(["sync", source.to_str().expect("UTF-8 path")]));

    let remote_object_path = first_file_under(&remote.join("object-cache").join("objects"));
    std::fs::remove_file(remote_object_path).expect("remote object removes");

    let check = run_loom(["remote", "check", source.to_str().expect("UTF-8 path")]);

    assert!(!check.status.success());
    assert!(stdout(&check).contains("ERROR missing-remote-object"));
    assert!(stderr(&check).contains("remote check found"));
}

#[test]
fn remote_check_rejects_unknown_remote_cursor_even_when_pack_and_objects_exist() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let foreign = dir.path().join("foreign");
    let remote = dir.path().join("remote");
    let foreign_remote = dir.path().join("foreign-remote");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::create_dir_all(&foreign).expect("foreign creates");
    std::fs::write(source.join("README.md"), "same bytes\n").expect("source readme writes");
    std::fs::write(foreign.join("README.md"), "same bytes\n").expect("foreign readme writes");

    assert_success(&run_loom(["track", source.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "remote",
        "add",
        "local",
        remote.to_str().expect("UTF-8 path"),
        source.to_str().expect("UTF-8 path"),
    ]));
    assert_success(&run_loom(["sync", source.to_str().expect("UTF-8 path")]));

    assert_success(&run_loom(["track", foreign.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "remote",
        "add",
        "local",
        foreign_remote.to_str().expect("UTF-8 path"),
        foreign.to_str().expect("UTF-8 path"),
    ]));
    assert_success(&run_loom(["sync", foreign.to_str().expect("UTF-8 path")]));

    let foreign_cursor =
        std::fs::read_to_string(foreign_remote.join("cursors").join("shared-folder.txt"))
            .expect("foreign cursor reads")
            .trim()
            .to_string();
    std::fs::copy(
        foreign_remote
            .join("packs")
            .join(format!("{foreign_cursor}.loompack")),
        remote
            .join("packs")
            .join(format!("{foreign_cursor}.loompack")),
    )
    .expect("foreign pack copies");
    std::fs::write(
        remote.join("cursors").join("shared-folder.txt"),
        format!("{foreign_cursor}\n"),
    )
    .expect("remote cursor rewrites");

    let check = run_loom(["remote", "check", source.to_str().expect("UTF-8 path")]);

    assert!(!check.status.success());
    let check_stdout = stdout(&check);
    assert!(check_stdout.contains("cursor known locally: false"));
    assert!(check_stdout.contains("missing objects: 0"));
    assert!(check_stdout.contains("ERROR remote-cursor-unknown-locally"));
    assert!(stderr(&check).contains("remote check found"));
}

#[test]
fn doctor_reports_healthy_after_normal_sync_and_sparse_clone() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let sparse_target = dir.path().join("sparse-target");
    std::fs::create_dir_all(source.join("src")).expect("source creates");
    std::fs::write(source.join("README.md"), "hello\n").expect("readme writes");
    std::fs::write(source.join("src").join("main.rs"), "fn main() {}\n").expect("main writes");

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
        sparse_target.to_str().expect("UTF-8 path"),
        "--sparse",
    ]));

    let source_doctor = run_loom(["doctor", source.to_str().expect("UTF-8 path")]);
    assert_success(&source_doctor);
    assert!(stdout(&source_doctor).contains("Status: healthy"));

    let sparse_doctor = run_loom(["doctor", sparse_target.to_str().expect("UTF-8 path")]);
    assert_success(&sparse_doctor);
    assert!(stdout(&sparse_doctor).contains("Status: healthy"));

    assert_success(&run_loom([
        "pin",
        sparse_target
            .join("README.md")
            .to_str()
            .expect("UTF-8 path"),
    ]));
    let pinned_sparse_doctor = run_loom(["doctor", sparse_target.to_str().expect("UTF-8 path")]);
    assert_success(&pinned_sparse_doctor);
    let pinned_stdout = stdout(&pinned_sparse_doctor);
    assert!(pinned_stdout.contains("WARN pinned-path-not-hydrated"));
    assert!(pinned_stdout.contains("Status: warnings"));
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
fn workspace_help_lists_agent_session_commands() {
    let output = run_loom(["workspace", "--help"]);

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("loom workspace open"));
    assert!(stdout.contains("loom workspace read"));
    assert!(stdout.contains("loom workspace write"));
    assert!(stdout.contains("loom workspace checkpoint"));
    assert!(stdout.contains("agent virtual sessions"));
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
fn sparse_clone_hydrate_evict_pin_and_cache_status_control_materialization() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let sparse_target = dir.path().join("sparse-target");
    let eager_target = dir.path().join("eager-target");
    std::fs::create_dir_all(source.join("src")).expect("source creates");
    std::fs::write(source.join("README.md"), "hello from source\n").expect("readme writes");
    std::fs::write(source.join("src").join("main.rs"), "fn main() {}\n").expect("main writes");

    assert_success(&run_loom(["track", source.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "remote",
        "add",
        "local",
        remote.to_str().expect("UTF-8 path"),
        source.to_str().expect("UTF-8 path"),
    ]));
    assert_success(&run_loom(["sync", source.to_str().expect("UTF-8 path")]));

    let sparse_clone = run_loom([
        "clone",
        remote.to_str().expect("UTF-8 path"),
        sparse_target.to_str().expect("UTF-8 path"),
        "--sparse",
    ]);
    assert_success(&sparse_clone);
    let sparse_stdout = stdout(&sparse_clone);
    assert!(sparse_stdout.contains("Mode: sparse metadata-only"));
    assert!(sparse_stdout.contains("Materialized: 0 files"));
    assert!(sparse_target.join(".loom").is_dir());
    assert!(!sparse_target.join("README.md").exists());
    assert!(!sparse_target.join("src").join("main.rs").exists());

    let sparse_status = run_loom([
        "cache",
        "status",
        sparse_target.to_str().expect("UTF-8 path"),
    ]);
    assert_success(&sparse_status);
    let sparse_status_stdout = stdout(&sparse_status);
    assert!(sparse_status_stdout.contains("remote-only: 2"));
    assert!(sparse_status_stdout.contains("total files: 2"));

    let status_after_sparse = run_loom(["status", sparse_target.to_str().expect("UTF-8 path")]);
    assert_success(&status_after_sparse);
    let status_after_sparse_stdout = stdout(&status_after_sparse);
    assert!(
        status_after_sparse_stdout.contains("No source changes since the latest folder revision.")
    );
    assert!(!status_after_sparse_stdout.contains("Captured new folder revision"));

    let sync_after_sparse = run_loom(["sync", sparse_target.to_str().expect("UTF-8 path")]);
    assert_success(&sync_after_sparse);

    let hydrate_readme = run_loom([
        "hydrate",
        sparse_target
            .join("README.md")
            .to_str()
            .expect("UTF-8 path"),
    ]);
    assert_success(&hydrate_readme);
    assert_eq!(
        std::fs::read_to_string(sparse_target.join("README.md")).expect("readme reads"),
        "hello from source\n"
    );
    assert!(!sparse_target.join("src").join("main.rs").exists());

    std::fs::write(sparse_target.join("README.md"), "changed sparse file\n")
        .expect("sparse readme edits");
    let sync_after_sparse_edit = run_loom(["sync", sparse_target.to_str().expect("UTF-8 path")]);
    assert_success(&sync_after_sparse_edit);
    assert!(stdout(&sync_after_sparse_edit).contains("Pack objects: 1"));

    std::fs::write(sparse_target.join("README.md"), "hello from source\n")
        .expect("clean hydrated file restores");
    assert_success(&run_loom([
        "sync",
        sparse_target.to_str().expect("UTF-8 path"),
    ]));

    std::fs::write(sparse_target.join("README.md"), "dirty after hydrate\n")
        .expect("dirty hydrated file writes");
    let dirty_evict = run_loom([
        "evict",
        sparse_target
            .join("README.md")
            .to_str()
            .expect("UTF-8 path"),
    ]);
    assert!(!dirty_evict.status.success());
    assert!(stderr(&dirty_evict).contains("dirty local file"));
    std::fs::write(sparse_target.join("README.md"), "hello from source\n")
        .expect("clean hydrated file restores");

    let evict_readme = run_loom([
        "evict",
        sparse_target
            .join("README.md")
            .to_str()
            .expect("UTF-8 path"),
    ]);
    assert_success(&evict_readme);
    assert!(!sparse_target.join("README.md").exists());
    let evict_status = run_loom([
        "cache",
        "status",
        sparse_target.to_str().expect("UTF-8 path"),
    ]);
    assert_success(&evict_status);
    assert!(stdout(&evict_status).contains("remote-only: 2"));

    std::fs::create_dir_all(sparse_target.join("src")).expect("sparse src creates");
    std::fs::write(
        sparse_target.join("src").join("main.rs"),
        "dirty local placeholder\n",
    )
    .expect("dirty sparse file writes");
    let dirty_hydrate = run_loom([
        "hydrate",
        sparse_target
            .join("src")
            .join("main.rs")
            .to_str()
            .expect("UTF-8 path"),
    ]);
    assert!(!dirty_hydrate.status.success());
    assert!(stderr(&dirty_hydrate).contains("dirty local file"));
    assert_eq!(
        std::fs::read_to_string(sparse_target.join("src").join("main.rs"))
            .expect("dirty sparse file reads"),
        "dirty local placeholder\n"
    );
    std::fs::remove_file(sparse_target.join("src").join("main.rs"))
        .expect("dirty sparse file removes");

    assert_success(&run_loom([
        "hydrate",
        sparse_target
            .join("README.md")
            .to_str()
            .expect("UTF-8 path"),
    ]));
    assert_success(&run_loom([
        "pin",
        sparse_target
            .join("README.md")
            .to_str()
            .expect("UTF-8 path"),
    ]));
    let pinned_evict = run_loom([
        "evict",
        sparse_target
            .join("README.md")
            .to_str()
            .expect("UTF-8 path"),
    ]);
    assert!(!pinned_evict.status.success());
    assert!(stderr(&pinned_evict).contains("pinned"));
    assert!(sparse_target.join("README.md").exists());

    let eager_clone = run_loom([
        "clone",
        remote.to_str().expect("UTF-8 path"),
        eager_target.to_str().expect("UTF-8 path"),
    ]);
    assert_success(&eager_clone);
    let eager_stdout = stdout(&eager_clone);
    assert!(eager_stdout.contains("Mode: eager materialized"));
    assert_eq!(
        std::fs::read_to_string(eager_target.join("README.md")).expect("eager readme reads"),
        "hello from source\n"
    );
    assert_eq!(
        std::fs::read_to_string(eager_target.join("src").join("main.rs"))
            .expect("eager main reads"),
        "fn main() {}\n"
    );
}

#[test]
fn workspace_agent_sessions_read_write_diff_checkpoint_and_isolate_overlays() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::write(source.join("README.md"), "hello workspace\n").expect("readme writes");

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
        "--sparse",
    ]));
    assert!(!target.join("README.md").exists());

    let open_a = run_loom([
        "workspace",
        "open",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-a",
    ]);
    assert_success(&open_a);
    assert!(stdout(&open_a).contains("Adapter: agent virtual"));
    assert_success(&run_loom([
        "workspace",
        "open",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-b",
    ]));

    let read_a = run_loom([
        "workspace",
        "read",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-a",
        "README.md",
    ]);
    assert_success(&read_a);
    assert_eq!(stdout(&read_a), "hello workspace\n");
    assert!(!target.join("README.md").exists());

    assert_success(&run_loom([
        "workspace",
        "write",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-a",
        "README.md",
        "--text",
        "agent A",
    ]));
    let read_a_overlay = run_loom([
        "workspace",
        "read",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-a",
        "README.md",
    ]);
    assert_success(&read_a_overlay);
    assert_eq!(stdout(&read_a_overlay), "agent A");

    let read_b_base = run_loom([
        "workspace",
        "read",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-b",
        "README.md",
    ]);
    assert_success(&read_b_base);
    assert_eq!(stdout(&read_b_base), "hello workspace\n");

    assert_success(&run_loom([
        "workspace",
        "write",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-b",
        "README.md",
        "--text",
        "agent B",
    ]));

    let diff_a = run_loom([
        "workspace",
        "diff",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-a",
    ]);
    assert_success(&diff_a);
    let diff_a_stdout = stdout(&diff_a);
    assert!(diff_a_stdout.contains("Changes: 0 created, 1 modified"));
    assert!(diff_a_stdout.contains("README.md"));

    let checkpoint_a = run_loom([
        "workspace",
        "checkpoint",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-a",
        "-m",
        "agent A checkpoint",
    ]);
    assert_success(&checkpoint_a);
    let checkpoint_stdout = stdout(&checkpoint_a);
    assert!(checkpoint_stdout.contains("Boundary: sandbox-merge"));
    assert!(checkpoint_stdout.contains("Overlay files: 1"));

    assert_success(&run_loom([
        "workspace",
        "close",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-a",
    ]));

    let read_b_overlay = run_loom([
        "workspace",
        "read",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-b",
        "README.md",
    ]);
    assert_success(&read_b_overlay);
    assert_eq!(stdout(&read_b_overlay), "agent B");
    assert_success(&run_loom([
        "workspace",
        "discard",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-b",
    ]));

    let history = run_loom(["history", target.to_str().expect("UTF-8 path")]);
    assert_success(&history);
    assert!(stdout(&history).contains("agent A checkpoint"));
    assert!(!target.join("README.md").exists());
}

#[test]
fn workspace_read_writes_binary_bytes_without_utf8_loss() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    let binary = vec![0x00, 0xff, 0x80, b'L', b'o', b'o', b'm', 0x00];
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::write(source.join("blob.bin"), &binary).expect("binary writes");

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
        "--sparse",
    ]));
    assert_success(&run_loom([
        "workspace",
        "open",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-binary",
    ]));

    let read = run_loom([
        "workspace",
        "read",
        target.to_str().expect("UTF-8 path"),
        "--session",
        "agent-binary",
        "blob.bin",
    ]);

    assert_success(&read);
    assert_eq!(read.stdout, binary);
    assert!(!target.join("blob.bin").exists());
}

#[test]
fn cache_status_prune_and_prefetch_keep_sparse_cache_bounded() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    std::fs::create_dir_all(source.join("src")).expect("src creates");
    std::fs::create_dir_all(source.join("config")).expect("config creates");
    std::fs::create_dir_all(source.join("node_modules/pkg")).expect("generated creates");
    std::fs::write(source.join("README.md"), "hello\n").expect("readme writes");
    std::fs::write(source.join("src").join("main.rs"), "fn main() {}\n").expect("main writes");
    std::fs::write(source.join("config").join("app.toml"), "debug=1\n").expect("config writes");
    std::fs::write(source.join("big.bin"), "x".repeat(80)).expect("large writes");
    std::fs::write(source.join("node_modules/pkg/index.js"), "generated\n")
        .expect("generated writes");

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
        "--sparse",
    ]));

    let sparse_status = run_loom(["cache", "status", target.to_str().expect("UTF-8 path")]);
    assert_success(&sparse_status);
    let sparse_status_stdout = stdout(&sparse_status);
    assert!(sparse_status_stdout.contains("hydrated: 0"));
    assert!(sparse_status_stdout.contains("remote-only: 4"));
    assert!(sparse_status_stdout.contains("remote-only bytes: 107"));
    assert!(sparse_status_stdout.contains("evictable: 0 files, 0 bytes"));
    assert!(sparse_status_stdout.contains("would avoid downloading: 0 bytes already local"));
    assert!(sparse_status_stdout.contains("pending uploads: 0 files, 0 bytes"));
    assert!(sparse_status_stdout.contains("cache hits/misses: not measured yet"));

    let prefetch = run_loom([
        "cache",
        "prefetch",
        target.to_str().expect("UTF-8 path"),
        "--max-bytes",
        "20",
    ]);
    assert_success(&prefetch);
    let prefetch_stdout = stdout(&prefetch);
    assert!(prefetch_stdout.contains("Selected: 3 files; skipped large: 1 files"));
    assert!(target.join("README.md").exists());
    assert!(target.join("src").join("main.rs").exists());
    assert!(target.join("config").join("app.toml").exists());
    assert!(!target.join("big.bin").exists());
    assert!(!target.join("node_modules/pkg/index.js").exists());

    assert_success(&run_loom([
        "pin",
        target.join("README.md").to_str().expect("UTF-8 path"),
    ]));
    std::fs::write(target.join("src").join("main.rs"), "dirty local change\n")
        .expect("dirty source writes");

    let hydrated_status = run_loom(["cache", "status", target.to_str().expect("UTF-8 path")]);
    assert_success(&hydrated_status);
    let hydrated_status_stdout = stdout(&hydrated_status);
    assert!(hydrated_status_stdout.contains("hydrated: 2"));
    assert!(hydrated_status_stdout.contains("remote-only: 1"));
    assert!(hydrated_status_stdout.contains("partial: 1"));
    assert!(hydrated_status_stdout.contains("pinned: 1 files, 6 bytes"));
    assert!(hydrated_status_stdout.contains("evictable: 1 files, 8 bytes"));

    let prune = run_loom([
        "cache",
        "prune",
        "--max-bytes",
        "0",
        target.to_str().expect("UTF-8 path"),
    ]);
    assert_success(&prune);
    let prune_stdout = stdout(&prune);
    assert!(prune_stdout.contains("Evicted: 1 files, 1 objects"));
    assert!(prune_stdout.contains("Skipped: 1 pinned, 1 dirty, 0 unsupported"));
    assert!(target.join("README.md").exists());
    assert!(target.join("src").join("main.rs").exists());
    assert!(!target.join("config").join("app.toml").exists());
    assert_eq!(
        std::fs::read_to_string(target.join("src").join("main.rs")).expect("dirty reads"),
        "dirty local change\n"
    );
}

#[test]
fn cache_warm_hydrates_useful_files_and_skips_large_generated_and_secret_blocked() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    std::fs::create_dir_all(source.join("src")).expect("src creates");
    std::fs::create_dir_all(source.join("config")).expect("config creates");
    std::fs::create_dir_all(source.join("node_modules/pkg")).expect("generated creates");
    std::fs::write(source.join("README.md"), "hello\n").expect("readme writes");
    std::fs::write(source.join("package.json"), "{}\n").expect("package writes");
    std::fs::write(source.join("src").join("main.rs"), "fn main() {}\n").expect("main writes");
    std::fs::write(source.join("config").join("app.toml"), "debug=1\n").expect("config writes");
    std::fs::write(source.join("big.bin"), "x".repeat(80)).expect("large writes");
    std::fs::write(source.join("node_modules/pkg/index.js"), "generated\n")
        .expect("generated writes");
    let raw_secret = ["sk-", "abcdefghijklmnopqrstuvwxyzABCDEFGH123456"].concat();
    std::fs::write(
        source.join("secrets.env"),
        format!("OPENAI_API_KEY={raw_secret}\n"),
    )
    .expect("secret writes");

    let track = run_loom(["track", source.to_str().expect("UTF-8 path")]);
    assert_success(&track);
    assert!(stdout(&track).contains("secret-blocked"));
    std::fs::remove_file(source.join("secrets.env")).expect("local secret removes before sync");
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
        "--sparse",
    ]));

    let warm = run_loom([
        "cache",
        "warm",
        target.to_str().expect("UTF-8 path"),
        "--max-bytes",
        "20",
    ]);
    assert_success(&warm);
    let warm_stdout = stdout(&warm);
    assert!(warm_stdout.contains("Warmed: ."));
    assert!(warm_stdout.contains("Selected: 4 files (3 manifest/config, 1 source, 0 other small)"));
    assert!(warm_stdout.contains("Skipped: 1 large, 0 outside manifest filter"));
    assert!(warm_stdout.contains("Avoided download bytes: 0"));
    assert!(target.join("README.md").exists());
    assert!(target.join("package.json").exists());
    assert!(target.join("src").join("main.rs").exists());
    assert!(target.join("config").join("app.toml").exists());
    assert!(!target.join("big.bin").exists());
    assert!(!target.join("node_modules/pkg/index.js").exists());
    assert!(!target.join("secrets.env").exists());

    let repeat_warm = run_loom([
        "cache",
        "warm",
        target.to_str().expect("UTF-8 path"),
        "--max-bytes",
        "20",
    ]);
    assert_success(&repeat_warm);
    assert!(stdout(&repeat_warm).contains("Avoided download bytes: 30"));
}

#[test]
fn cache_warm_manifest_filter_only_hydrates_manifests_and_config() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    std::fs::create_dir_all(source.join("src")).expect("src creates");
    std::fs::create_dir_all(source.join("config")).expect("config creates");
    std::fs::write(source.join("README.md"), "hello\n").expect("readme writes");
    std::fs::write(source.join("src").join("main.rs"), "fn main() {}\n").expect("main writes");
    std::fs::write(source.join("config").join("app.toml"), "debug=1\n").expect("config writes");

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
        "--sparse",
    ]));

    let warm = run_loom([
        "cache",
        "warm",
        target.to_str().expect("UTF-8 path"),
        "--manifest",
        "--max-bytes",
        "20",
    ]);
    assert_success(&warm);
    let warm_stdout = stdout(&warm);
    assert!(warm_stdout.contains("Filter: manifest/config files only"));
    assert!(warm_stdout.contains("Selected: 2 files (2 manifest/config, 0 source, 0 other small)"));
    assert!(warm_stdout.contains("Skipped: 0 large, 1 outside manifest filter"));
    assert!(target.join("README.md").exists());
    assert!(target.join("config").join("app.toml").exists());
    assert!(!target.join("src").join("main.rs").exists());
}

#[test]
fn cache_free_space_uses_safe_prune_behavior_with_pins_and_remote_proof() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::write(source.join("keep.txt"), "keep\n").expect("keep writes");
    std::fs::write(source.join("drop.txt"), "drop\n").expect("drop writes");

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
        "--sparse",
    ]));
    assert_success(&run_loom(["hydrate", target.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "pin",
        target.join("keep.txt").to_str().expect("UTF-8 path"),
    ]));

    let free_space = run_loom([
        "cache",
        "free-space",
        "--max-bytes",
        "0",
        target.to_str().expect("UTF-8 path"),
    ]);
    assert_success(&free_space);
    let free_space_stdout = stdout(&free_space);
    assert!(free_space_stdout.contains("Free-space target: 0 hydrated bytes"));
    assert!(free_space_stdout.contains("remote proof"));
    assert!(free_space_stdout.contains("Evicted: 1 files, 1 objects"));
    assert!(free_space_stdout.contains("Skipped: 1 pinned, 0 dirty, 0 unsupported"));
    assert!(target.join("keep.txt").exists());
    assert!(!target.join("drop.txt").exists());
}

#[test]
fn cache_policy_show_exposes_internal_presets_without_mode_switching() {
    let policy = run_loom(["cache", "policy", "show"]);

    assert_success(&policy);
    let policy_stdout = stdout(&policy);
    assert!(policy_stdout.contains("These are internal presets"));
    assert!(policy_stdout.contains("Normal use stays intent-based"));
    for preset in [
        "online-first",
        "offline-pinned",
        "low-disk",
        "agent-sandbox",
        "ci-ephemeral",
    ] {
        assert!(policy_stdout.contains(preset), "{preset} missing");
    }
    assert!(!policy_stdout.contains("cache mode"));
}

#[test]
fn cache_prune_refuses_without_remote_object_proof() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = dir.path().join("fixture");
    std::fs::create_dir_all(&fixture).expect("fixture creates");
    std::fs::write(fixture.join("README.md"), "local only\n").expect("readme writes");

    assert_success(&run_loom(["track", fixture.to_str().expect("UTF-8 path")]));

    let prune = run_loom([
        "cache",
        "prune",
        "--max-bytes",
        "0",
        fixture.to_str().expect("UTF-8 path"),
    ]);

    assert!(!prune.status.success());
    assert!(stderr(&prune).contains("no Loom remote is configured"));
    assert_eq!(
        std::fs::read_to_string(fixture.join("README.md")).expect("readme reads"),
        "local only\n"
    );

    let status = run_loom(["cache", "status", fixture.to_str().expect("UTF-8 path")]);
    assert_success(&status);
    let status_stdout = stdout(&status);
    assert!(status_stdout.contains("hydrated: 1"));
    assert!(status_stdout.contains("evictable: 0 files, 0 bytes"));
    assert!(status_stdout.contains("pending uploads: unknown (no remote configured)"));
    assert!(status_stdout.contains("cache hits/misses: not measured yet"));
}

#[test]
fn cache_status_counts_materialized_duplicate_objects_once_per_present_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let source = dir.path().join("source");
    let remote = dir.path().join("remote");
    let target = dir.path().join("target");
    std::fs::create_dir_all(&source).expect("source creates");
    std::fs::write(source.join("a.txt"), "same text\n").expect("a writes");
    std::fs::write(source.join("b.txt"), "same text\n").expect("b writes");

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
        "--sparse",
    ]));
    assert_success(&run_loom(["hydrate", target.to_str().expect("UTF-8 path")]));
    assert_success(&run_loom([
        "pin",
        target.join("a.txt").to_str().expect("UTF-8 path"),
    ]));

    let prune = run_loom([
        "cache",
        "prune",
        "--max-bytes",
        "0",
        target.to_str().expect("UTF-8 path"),
    ]);
    assert_success(&prune);
    assert!(target.join("a.txt").exists());
    assert!(!target.join("b.txt").exists());

    let status = run_loom(["cache", "status", target.to_str().expect("UTF-8 path")]);
    assert_success(&status);
    let status_stdout = stdout(&status);
    assert!(status_stdout.contains("hydrated: 1"));
    assert!(status_stdout.contains("partial: 1"));
    assert!(status_stdout.contains("hydrated bytes: 10"));
    assert!(status_stdout.contains("pinned: 1 files, 10 bytes"));
    assert!(status_stdout.contains("evictable: 0 files, 0 bytes"));
    assert!(!status_stdout.contains("hydrated bytes: 20"));
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

fn first_file_under(path: &std::path::Path) -> std::path::PathBuf {
    let mut stack = vec![path.to_path_buf()];
    while let Some(path) = stack.pop() {
        let mut entries = std::fs::read_dir(&path)
            .expect("directory reads")
            .collect::<Result<Vec<_>, _>>()
            .expect("entries read");
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else {
                return entry_path;
            }
        }
    }
    panic!("no file under {}", path.display());
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
