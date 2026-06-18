# Local Metadata Store

This slice introduces the Phase 0 local storage boundary in `crates/devbox-store`.

The store is intentionally a metadata database, not a file-content database. It gives the daemon a transactional place to record what Devbox knows about projects, snapshots, manifests, policy decisions, and restore attempts while keeping actual source bytes in a local content-addressed cache.

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

The local content-addressed cache owns:

- file and chunk bytes
- BLAKE3 content hashing
- deterministic object layout under `blobs/b3/<first-two-hex>/<next-two-hex>/<digest>`
- temporary files under `tmp/` while a blob write is in progress
- hash-derived object identity for local bytes

SQLite rows may reference cache objects, but SQLite must not store project files as database BLOBs. This keeps metadata reads fast, avoids oversized transactions, and lets the cache implementation change without rewriting the metadata model.

`BlobCache::open` initializes a cache root without opening SQLite. `write_bytes` and `write_file` stream content through BLAKE3, write the bytes to a temporary file inside the cache root, and then move the completed file into its sharded content-addressed path. Rewriting the same bytes returns the same `BlobId` and path without creating a second committed object. `read`, `exists`, and `path_for` are filesystem operations against that cache root.

The SQLite `blobs.object_ref` column should store a stable reference to this cache object, such as `blobs/b3/aa/bb/<digest>`, plus metadata like hash algorithm and byte length. SQLite owns the metadata row lifecycle; the blob cache owns bytes and paths.

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

- recording blob metadata rows automatically during cache writes
- snapshot creation
- translating draft snapshot manifests into SQLite rows
- restore planning or materialization
- filesystem watching
- cloud sync
- encryption
- compression
- packfiles
- cache eviction or garbage collection
- read-time integrity verification
- Electron UI integration

Those behaviors should enter through later focused PRs that use this store rather than expanding it into a snapshot engine.
