# Local Metadata Store

This slice introduces the Phase 0 SQLite boundary in `crates/devbox-store`.

The store is intentionally a metadata database, not a file-content database. It gives the daemon a transactional place to record what Devbox knows about projects, snapshots, manifests, policy decisions, and restore attempts while keeping actual source bytes in a future content-addressed cache.

## Boundary

SQLite owns:

- schema versioning and migration history
- detected projects and their local roots
- snapshot records and parent relationships
- manifest entry metadata such as path, kind, size, blob reference, and policy decision
- blob and chunk metadata, including content hash identifiers and filesystem/object references
- operation log rows for future snapshot, restore, divergence, policy, and sync events
- policy definitions and policy evaluation results
- restore attempt status, target path, safety report references, and errors

The future content-addressed cache owns:

- file and chunk bytes
- packed blob files
- cache layout and eviction behavior
- local verification of bytes against content hashes

SQLite rows may reference cache objects, but SQLite must not store project files as database BLOBs. This keeps metadata reads fast, avoids oversized transactions, and lets the cache implementation change without rewriting the metadata model.

## Migration Rules

`Store::open_in_memory` and `Store::open_file` enable SQLite foreign-key enforcement immediately. `Store::apply_migrations` is idempotent and currently creates schema version `1`.

The initial migration creates:

- `schema_migrations`
- `projects`
- `snapshots`
- `manifest_entries`
- `blobs`
- `chunks`
- `operations`
- `policies`
- `policy_evaluations`
- `restore_attempts`

`PRAGMA user_version` is the quick schema version check. `schema_migrations` records the applied migration name so later migrations can remain explicit and auditable.

## Deferred

This boundary does not implement:

- file hashing
- content-addressed writes
- manifest construction
- snapshot creation
- restore planning or materialization
- filesystem watching
- cloud sync
- Electron UI integration

Those behaviors should enter through later focused PRs that use this store rather than expanding it into a snapshot engine.
