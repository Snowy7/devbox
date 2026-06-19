# Devbox And Loom

## Devbox

Devbox is the product.

It keeps developer folders continuous across machines. A user should be able to work on a desktop,
open a laptop, and find the same folder state waiting for them. The folder might be `~/Code`, one app,
a nested workspace, a Git repo, or a plain directory. Devbox should treat all of those as normal.

The product language is intentionally simple:

- folder
- machine
- sync
- pause
- clone
- share
- restore
- conflict

Avoid making users think in projects, remotes, object leases, metadata stores, or source-control
internals.

## Loom

**Loom** is the codename for the source-control primitive underneath Devbox.

Loom exists because generic file sync is too careless for code, while Git is not built around live
working-state continuity, agent work, or safe multi-machine folder presence.

Loom should give Devbox a better base model:

- continuous file history and coherent folder revisions instead of only manual commits
- human checkpoints when someone wants to name a meaningful moment
- isolated sandboxes for agents or parallel work
- shared overlays for dependencies, caches, local config, and secrets
- readable conflicts instead of mystery files
- API-addressable folder state for tools and agents

Users do not need to know Loom exists at first. They should feel it through Devbox being safe,
fast, and calm.

## Loom Vocabulary

Loom should not create a giant whole-folder revision for every keystroke. It should model history in
layers:

- **Object**: content-addressed bytes for file content.
- **File version**: a meaningful observed version of one file path, pointing to an object plus
  metadata.
- **Folder revision**: a coherent folder tree assembled from file versions at a stable moment.
- **Checkpoint**: a human label/message attached to a folder revision.
- **Pin**: a retention marker that protects a revision from cleanup.
- **Cursor**: a moving pointer such as "this machine has synced through this folder revision".

File versions can be captured frequently. Folder revisions should be coalesced around stable
boundaries such as debounce windows, Loom commands, sync, restore, sandbox merge, or checkpoint
creation. Checkpoints do not save work for the first time; they name a folder revision that Loom has
already made durable.

Users should be able to inspect and restore automatic folder revisions even when no checkpoint was
created. Retention can prune noisy automatic history, but checkpoints and pins should survive longer
by default.

## Git

Git is supported because developers use Git.

Devbox should detect and respect Git folders, but Git should be treated as folder context, not as
the center of the product. `.git` must not be synced as ordinary files. Devbox should help existing
Git projects work safely while Loom grows into the deeper primitive.

## Naming Rule

Use these words in new product docs:

- **Shared folder** for what the user chooses to sync.
- **Machine** or **device** for where the folder appears.
- **File version** for one path's captured content state.
- **Folder revision** for a coherent assembled folder state.
- **Checkpoint** for a meaningful saved moment.
- **Pin** for a retained revision.
- **Cursor** for a moving sync/materialization pointer.
- **Sandbox** for isolated parallel work.
- **Overlay** for local non-source state shared across sandboxes.
- **Loom** for the internal source-control primitive.

Use **project** only when referring to existing code/API/schema terms that have not been renamed yet.
