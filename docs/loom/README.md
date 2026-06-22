# Loom Docs

Loom is the engine underneath Devbox: file versions, folder revisions, checkpoints, restore, sync,
and safe developer-folder state.

Use this folder for Loom vocabulary, engine behavior, remote protocol, worktree safety, Git
compatibility, and agent/sandbox design notes.

Sync packs are folder-state manifests first. They list file versions, folder revisions, checkpoints,
pins, and object payload availability. Object bytes may still travel inline for compatibility, but
local filesystem remotes store pack metadata and content-addressed object bytes separately.

`loom clone <REMOTE> <FOLDER>` remains eager by default: it imports metadata and object bytes, then
materializes the latest folder revision into the target folder. `loom clone <REMOTE> <FOLDER>
--sparse` imports folder history and cache/object availability metadata without writing source files
into the worktree. Use intent commands for day-to-day sparse folders:

- keep this available: `loom pin <path-or-folder>`
- warm useful files here: `loom cache warm <path-or-folder>`
- free local space safely: `loom cache free-space --max-bytes <bytes> [folder]`
- check hydration and health: `loom cache status [folder]`, `loom doctor [folder]`

Short local try path:

```text
loom track ./source
loom remote add local ./remote ./source
loom sync ./source
loom clone ./remote ./sparse-target --sparse
loom cache warm ./sparse-target
loom doctor ./sparse-target
```

`loom hydrate <path-or-folder>` still explicitly fetches and materializes a path or subtree. `loom
evict <path-or-folder>` still removes clean local materialized bytes while keeping history. Pins
protect paths from eviction and appear in cache status.

Sparse cache discipline stays conservative. `loom cache warm <path-or-folder>` deterministically
hydrates useful tracked files from the latest folder revision: manifests, config files, source files,
and other small files up to the byte limit. `--manifest` narrows warmup to manifest/config files, and
`--max-bytes <bytes>` changes the per-file cap. Generated folders remain excluded by capture policy,
secret-blocked files are never cached or materialized, and large or deferred entries are left
remote-only until the user explicitly hydrates them.

`loom cache free-space --max-bytes <bytes> [folder]` deterministically removes clean, unpinned
hydrated files until the local hydrated byte count is under the limit or only protected entries
remain. It never removes pinned paths, never removes a materialized file whose current bytes differ
from the latest folder revision, never evicts without remote object proof, and keeps folder revision
history plus remote-only cache metadata. `loom cache prune` remains a compatibility alias for the
same safe free-space behavior, and `loom cache prefetch` remains an older alias for warming bounded
small files.

`loom cache status` reports hydrated bytes, remote-only bytes, pinned bytes, evictable bytes, pending
uploads when the configured remote can be checked, and explicitly says cache hits/misses are not
measured yet. `loom cache policy show` is diagnostic only: policy presets such as online-first,
offline-pinned, low-disk, agent-sandbox, and ci-ephemeral are internal command presets, not normal
user modes.

## Workspace Adapters

Workspace adapters expose a folder revision as a session view. PR 1 adds the agent virtual adapter:
it reads from Loom metadata and object/cache state, keeps writes in a per-session overlay, and only
turns overlay writes into Loom file versions and a folder revision when the session checkpoints.

The command surface is under `loom workspace`:

```text
loom workspace open ./folder --session agent-1
loom workspace read ./folder --session agent-1 README.md
loom workspace write ./folder --session agent-1 README.md --text "changed in overlay"
loom workspace diff ./folder --session agent-1
loom workspace checkpoint ./folder --session agent-1 -m "agent checkpoint"
loom workspace close ./folder --session agent-1
```

Agent virtual sessions do not require full local materialization. Reading a base file first checks
the overlay, then the local object cache, and then lazily hydrates the object from the configured
Loom remote if the object is remote-only. The read returns bytes to the session without writing the
source file into the shared folder. `loom workspace hydrate` has the same virtual meaning: fetch
object bytes into Loom's cache for that session view, not into the visible folder tree.

Overlay writes are isolated by session under `.loom/workspaces/sessions/<session-id>`. Two sessions
opened on the same folder revision can write the same path without seeing each other's overlay.
`loom workspace diff` compares only the session overlay with the session's base folder revision.
`loom workspace checkpoint` coalesces the overlay into normal Loom file versions and a
`sandbox-merge` folder revision, then creates a checkpoint label. If another session has already
advanced the folder revision, checkpointing a stale session is refused instead of silently reverting
newer folder state.

The agent virtual adapter returns explicit unsupported-operation errors for `loom workspace
dehydrate` and `loom workspace pin`. Those capabilities belong to later adapters that can safely
coordinate per-session views with shared cache retention or materialized folder policy.

This is not OS filesystem mounting. The agent virtual adapter is a programmatic/CLI view over Loom
metadata and object bytes. It does not intercept normal file opens, provide Finder/Explorer/FUSE
integration, or make a shell see virtual files. Native Windows, macOS, and Linux filesystem adapters
are later arc work. This PR also does not add just-bash execution, SDKs, compression, or full
sparse/chunk transfer.
