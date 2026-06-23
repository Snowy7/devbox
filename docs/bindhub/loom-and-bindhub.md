# Bindhub And Loom

## Bindhub

Bindhub is the product.

It keeps developer folders continuous across machines. A user should be able to work on a desktop,
open a laptop, and find the same folder state waiting for them. The folder might be `~/Code`, one app,
a nested workspace, a Git repo, or a plain directory. Bindhub should treat all of those as normal.

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

**Loom** is the codename for the source-control primitive underneath Bindhub.

Loom exists because generic file sync is too careless for code, while Git is not built around live
working-state continuity, agent work, or safe multi-machine folder presence.

Loom should give Bindhub a better base model:

- continuous file history and coherent folder revisions instead of only manual commits
- human checkpoints when someone wants to name a meaningful moment
- isolated sandboxes for agents or parallel work
- shared overlays for dependencies, caches, local config, and secrets
- readable conflicts instead of mystery files
- API-addressable folder state for tools and agents

Users do not need to know Loom exists at first. They should feel it through Bindhub being safe,
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

## Trust And Inspection

Loom owns the trust primitive; Bindhub may wrap it in product language.

- `loom doctor [FOLDER]` is the broad report-only health check for a shared folder.
- `loom fsck [FOLDER]` verifies local metadata references, object hashes, and cache-entry consistency.
- `loom object verify [FOLDER]` focuses on local object/cache integrity.
- `loom remote check [FOLDER]` proves the configured remote cursor and referenced object bytes are
  available.

These commands should stay deterministic and conservative. They can tell a user what is corrupt,
missing, remote-only, or unsafe, but they should not perform risky automatic repair.

Sparse folders should stay intent-based instead of asking users to choose cache modes:

- `loom pin <PATH>` means keep this path available locally.
- `loom cache warm <PATH>` means hydrate the useful source, manifest, config, and small files here.
- `loom cache free-space --max-bytes <BYTES> [FOLDER]` means safely remove clean, unpinned local
  bytes that have remote proof.
- `loom cache status [FOLDER]` means explain what is local, remote-only, pinned, evictable, or still
  pending upload.

Internal presets such as online-first, offline-pinned, low-disk, agent-sandbox, and ci-ephemeral may
exist as implementation data, but Bindhub should remain opinionated by default. A normal user should
not have to pick a cache mode before a folder feels continuous.

## Git

Git is supported because developers use Git.

Bindhub should detect and respect Git folders, but Git should be treated as folder context, not as
the center of the product. `.git` must not be synced as ordinary files. Bindhub should help existing
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

## Current Rust Workspace

The repository now uses top-level `loom/` and `bindhub/` areas as the active homes for Rust crates:

```text
loom/manifest.toml      Loom area ownership map
bindhub/manifest.toml    Bindhub area ownership map
loom/crates/loom-core       canonical Loom vocabulary and pure domain types
loom/crates/loom-store      object and metadata store boundary
loom/crates/loom-worktree   shared-folder scan/materialize/restore boundary
loom/crates/loom-pack       pack/import/export boundary
loom/crates/loom-sync       remote trait, local remote, and reconciliation boundary
loom/crates/loom-daemon     background capture/sync process
loom/crates/loom-cli        `loom` command surface
loom/crates/loom-git        Git compatibility analyzer boundary
bindhub/crates/bindhub-auth     account/session/device trust boundary
bindhub/crates/bindhub-platform hosted product/platform boundary
bindhub/crates/bindhub-remote   Bindhub-hosted Loom remote implementation
bindhub/crates/bindhub-api      hosted API boundary
bindhub/crates/bindhub-cli      Bindhub product CLI plus alpha compatibility commands
```

The old alpha compatibility crates live under `bindhub/crates/` too, so the existing local snapshot,
sync, metadata, and desktop smoke paths remain available while Loom becomes the place where
folder-state decisions move over time. Old alpha docs and commands that still say `project` or
`snapshot` are compatibility surfaces, not the model for new work.

Future FUSE, File Provider, Cloud Files, or similar integrations belong outside the core as adapters.
They should present Loom hydration state to the OS and call Loom primitives for hydrate, evict, and
cache inspection. Likewise, local filesystem storage, Bindhub hosted storage, and later S3/R2/MinIO or
self-hosted storage are backend choices behind Loom's remote boundary, not separate product models.
