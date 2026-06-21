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
into the worktree. Use `loom hydrate <path-or-folder>` to fetch and materialize a path or subtree,
`loom evict <path-or-folder>` to remove clean local materialized bytes while keeping history, `loom
pin <path-or-folder>` to prevent local eviction for a path/subtree, and `loom cache status [folder]`
to inspect hydrated, remote-only, pinned, and clean-evictable file counts and bytes.

Sparse cache discipline stays conservative. `loom cache prune --max-bytes <bytes> [folder]`
deterministically removes clean, unpinned hydrated files until the local hydrated byte count is under
the limit or only protected entries remain. It never removes pinned paths, never removes a
materialized file whose current bytes differ from the latest folder revision, and keeps folder
revision history plus remote-only cache metadata. `loom cache prefetch [folder]` hydrates a bounded
small-file set from the latest folder revision, defaulting to files up to 65536 bytes. Generated
folders remain excluded by capture policy, and large or deferred entries are left remote-only until
the user explicitly hydrates them.

This is explicit local materialization only. It does not add OS virtual filesystem support,
background smart prefetch, compression, or full sparse/chunk transfer.
