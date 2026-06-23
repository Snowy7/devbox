# Bindhub Agent Brief

Bindhub is about one feeling: I open another machine and my code folder is already there.

Keep the product centered on folders, machines, and trust. A shared folder might contain many repos,
one repo, no repo, nested apps, secrets, dependencies, or agent work. Bindhub should handle that
developer mess calmly instead of making the user manage it.

Loom is the codename for the deeper source-control idea underneath Bindhub. Bindhub is the product;
Loom is the engine direction. Git matters because people use it today, but Bindhub is not built
around Git.

Vocabulary:

- Shared folder: the folder the user wants to keep continuous.
- Loom: the source-control/sync engine underneath Bindhub.
- Object: content-addressed bytes.
- Chunk: content-addressed byte range vocabulary for future sparse/lazy transfer; PR 1 only adds
  primitives and metadata, not chunk transport.
- File version: one path's captured content state.
- Folder revision: a coherent folder tree assembled from file versions.
- Hydration state: whether object/cache bytes are remote-only, partial, or fully local.
- Cache entry: local metadata that records object byte availability separately from file versions and
  folder revisions.
- Checkpoint: a human label/message on a folder revision.
- Pin: a retention marker that keeps a revision.
- Cursor: a moving pointer for a machine, remote, or materialized folder state.
- Sandbox: isolated parallel work for humans or agents.
- Overlay: local non-source state such as dependencies, caches, config, and secrets.

Do not model Loom as "one whole-folder revision per edit." Capture file versions frequently, then
coalesce them into folder revisions at stable boundaries such as debounce windows, Loom commands,
sync, restore, sandbox merge, and checkpoint creation.

Do not treat cache metadata or chunk vocabulary as sparse clone being complete. Sparse/lazy
materialization, remote protocol v2, chunk transfer, compression, and OS virtual filesystem work are
later steps.

When working here, prefer small changes that make the folder-continuity promise more real. Keep docs
and UI language simple. Say "folder" unless you are touching older implementation details that still
say "project".
