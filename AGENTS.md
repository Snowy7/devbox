# Devbox Agent Brief

Devbox is about one feeling: I open another machine and my code folder is already there.

Keep the product centered on folders, machines, and trust. A shared folder might contain many repos,
one repo, no repo, nested apps, secrets, dependencies, or agent work. Devbox should handle that
developer mess calmly instead of making the user manage it.

Loom is the codename for the deeper source-control idea underneath Devbox. Devbox is the product;
Loom is the engine direction. Git matters because people use it today, but Devbox is not built
around Git.

Vocabulary:

- Shared folder: the folder the user wants to keep continuous.
- Loom: the source-control/sync engine underneath Devbox.
- Object: content-addressed bytes.
- File version: one path's captured content state.
- Folder revision: a coherent folder tree assembled from file versions.
- Checkpoint: a human label/message on a folder revision.
- Pin: a retention marker that keeps a revision.
- Cursor: a moving pointer for a machine, remote, or materialized folder state.
- Sandbox: isolated parallel work for humans or agents.
- Overlay: local non-source state such as dependencies, caches, config, and secrets.

Do not model Loom as "one whole-folder revision per edit." Capture file versions frequently, then
coalesce them into folder revisions at stable boundaries such as debounce windows, Loom commands,
sync, restore, sandbox merge, and checkpoint creation.

When working here, prefer small changes that make the folder-continuity promise more real. Keep docs
and UI language simple. Say "folder" unless you are touching older implementation details that still
say "project".
