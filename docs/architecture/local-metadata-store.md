# Local Metadata Store

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Bindhub.

This document describes the Phase 0 local storage boundary in `bindhub/crates/bindhub-store`.

The store is intentionally a metadata database, not a file-content database. It gives the daemon a transactional place to record what Bindhub knows about projects, snapshots, manifests, policy decisions, and restore attempts while keeping actual source bytes in a local content-addressed cache.

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

## Snapshot Persistence Flow

Local snapshot creation now crosses three deliberately separate boundaries:

1. `bindhub-snapshot` walks the project root, evaluates default policy, writes included file bytes into `BlobCache`, and returns an in-memory draft manifest.
2. `BlobCache` stores file bytes under content-addressed BLAKE3 object paths. It does not open or mutate SQLite.
3. `bindhub-store` persists the draft's metadata into SQLite: project identity, snapshot identity, creation time, summary counts and bytes, manifest entries, blob ids and object refs for included files, and policy rows for excluded or deferred entries.

SQLite stores metadata and references only. It does not store raw project file bytes. Repeated writes of the same file content continue to converge on the same blob-cache object, while the snapshot row records which object ref a manifest entry used.

The CLI surface for the persisted path is:

```text
bindhub snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PATH>
bindhub snapshot list --db <DB_PATH>
bindhub snapshot show --db <DB_PATH> <SNAPSHOT_ID>
bindhub snapshot restore --db <DB_PATH> --cache <CACHE_ROOT> --to <TARGET_DIR> <SNAPSHOT_ID> --dry-run
bindhub snapshot restore --db <DB_PATH> --cache <CACHE_ROOT> --to <TARGET_DIR> <SNAPSHOT_ID> --apply
```

`bindhub snapshot --cache <CACHE_ROOT> --dry-run <PATH>` remains non-persisting. Both dry-run and persisted creation reject a blob cache that sits inside the snapshot root before the cache can create directories.

Persisting the same stable snapshot id twice currently returns a duplicate snapshot error. The project row is upserted so the local root metadata can be refreshed without rewriting existing snapshot rows.

## Restore Read Boundary

Restore uses SQLite as metadata lookup only. The CLI loads the persisted snapshot row and manifest entries through `Store::snapshot_with_entries`, then passes those records to `bindhub-snapshot` with an opened local `BlobCache`.

The store does not materialize files, interpret path safety, or read blob bytes. It remains responsible for preserving the manifest metadata and blob object references that make restore planning possible. Missing cache objects are detected by the restore planner before apply is allowed.

The existing `restore_attempts` table is still reserved for a later operation log slice. This CLI foundation does not write restore attempt rows yet because there is no daemon operation lifecycle, retry model, or UI timeline consuming them.

## Migration Rules

`Store::open_in_memory` and `Store::open_file` enable SQLite foreign-key enforcement immediately. `Store::apply_migrations` is idempotent and currently creates schema version `7`.

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

Schema version `2` rebuilds `manifest_entries` with the same columns and constraints, but expands `entry_kind` to include `unsupported`. That keeps SQLite aligned with the domain manifest model used by the builder for deferred filesystem node types.

Schema version `3` adds `pending_local_changes` for the watcher-backed local operation feed.

Schema version `4` adds local identity tables:

- `local_accounts`
- `local_devices`

This is local-only identity for encrypted sync foundations, not cloud authentication. One
installation has one current local device (`is_local = 1`), but the account/device schema allows
many known non-local devices under the same account for later pairing and approval work.

Schema version `5` adds local/mock auth and device-pairing metadata:

- `auth_sessions`
- `pairing_invitations`
- `trusted_devices`
- `key_envelopes`
- `revocation_markers`
- `device_project_cursors`

These tables are local client state for the trusted personal alpha foundation. They model auth
session summaries, pairing records, trusted-device metadata, encrypted key envelope references,
revocation markers, and local cursor checkpoints without adding hosted backend behavior.

Schema version `6` adds a unique invitation claim index for `trusted_devices.invitation_id`. The
store also claims a pairing invitation inside the approval transaction with `status = 'pending'` so
the same invitation cannot approve two devices.

Schema version `7` adds local divergent-snapshot conflict metadata:

- `conflicts`
- `conflict_rows`

These tables store readable path-level comparison metadata for local conflict records. They persist
snapshot ids, status, summary counts, blob ids, sizes, policy decisions, and redacted policy reasons,
but not source file bytes. Conflict creation is idempotent for the same project/base/local/incoming
tuple.

The local/mock sync preflight path uses these rows with `device_project_cursors`. When the receiving
device's cursor/base, latest local snapshot, and incoming snapshot indicate divergence, Bindhub
persists an idempotent conflict record and refuses import/materialization without advancing the
cursor or downloading receiver file blobs.

## Deferred

This boundary does not implement:

- live OAuth/OIDC authentication, hosted sign-in UI, hosted proof verification, or production pairing UX
- backend account/device cursors
- hosted second-device materialization metadata or server-side conflict resolution
- automatic merge or conflict UI
- compression
- packfiles
- cache eviction or garbage collection
- read-time integrity verification
- Electron UI integration

Those behaviors should enter through later focused PRs that use this store rather than expanding it into a snapshot engine.
