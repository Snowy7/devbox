# Local Watcher Daemon Foundation

This slice introduces the local Phase 1 bridge between filesystem events and the existing pending
local change feed.

## Boundary

`devbox-daemon watch --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>` watches one project tree
recursively, debounces filesystem event bursts, and runs the same scan orchestration used by
`devbox changes scan`. The daemon writes only local SQLite metadata and local BLAKE3 cache blobs.

`devbox-daemon sync` is now the separate live-sync automation layer. `watch` itself still does not
upload objects, download objects, encrypt, compress, pack files, pair devices, resolve conflicts,
manage teams, replace Git, or drive the Electron UI.

## Event to Scan Flow

The watcher treats filesystem notifications as hints, not as authoritative diffs:

1. Start a recursive watch on the project root.
2. Record received events into an in-memory debounce batch.
3. After the quiet window elapses, run the shared local change-feed scan.
4. Replace that project's `pending_local_changes` rows with the deterministic current diff.
5. Emit scriptable status lines for start, received events, debounced batches, scan summaries, idle
   state, and errors.

Generated dependency directories can still emit filesystem events. That is acceptable because scans
continue to use the existing project policy and snapshot manifest builder, so generated directories
remain non-uploadable skipped/deferred entries rather than pending operations.

## Deterministic Modes

`--once` runs exactly one shared scan and exits. This is the preferred smoke-test path because it
validates daemon-to-change-feed wiring without depending on platform-specific watcher timing.

`--debounce-ms`, `--exit-after-idle-ms`, and `--max-scans` exist for local development and bounded
validation. Long-running production behavior remains the default when those exit controls are not
provided.

## Safety

The daemon inherits the shared preflight checks:

- the blob cache root must not be inside the watched project root
- the SQLite database path must not be inside the watched project root
- missing or uninitialized databases are opened and migrated through the same store path as the CLI
- source files are never mutated by the watcher or scan

Pending operations remain idempotent. Repeated watcher-triggered scans replace rows per project
instead of appending duplicates.

## Deferred Work

Cloud sync remains outside `watch`: R2/S3 object transport, hosted metadata discovery, device
cursors, materialization, and conflict handling live in the newer `devbox-daemon sync` foundation
and the lower-level sync/materialize crates. Garbage collection, background retries, and Electron
IPC remain deferred.
