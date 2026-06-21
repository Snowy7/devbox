# Loom Docs

Loom is the engine underneath Devbox: file versions, folder revisions, checkpoints, restore, sync,
and safe developer-folder state.

Use this folder for Loom vocabulary, engine behavior, remote protocol, worktree safety, Git
compatibility, and agent/sandbox design notes.

Sync packs are folder-state manifests first. They list file versions, folder revisions, checkpoints,
pins, and object payload availability. Object bytes may still travel inline for compatibility, but
local filesystem remotes store pack metadata and content-addressed object bytes separately. Sparse
or lazy materialization is still future work; current clone and sync commands eagerly hydrate bytes.
