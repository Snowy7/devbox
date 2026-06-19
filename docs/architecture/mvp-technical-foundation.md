# MVP Technical Foundation

Historical terminology note: this architecture slice uses `project` as an early implementation term
for a scoped shared folder. New product language should say shared folder. Loom is the codename for
the deeper source-control primitive underneath Devbox; `devbox-git` is compatibility support for
existing Git folders, not the product foundation.

This document records the first PR-sized technical skeleton for Devbox. It is intentionally kept as
historical architecture context: the goal was to establish crate boundaries, local domain vocabulary,
and future app layout before snapshot, restore, sync, and policy execution landed.

## Rust Workspace

The Rust workspace is rooted at `Cargo.toml` and has grown from the original four-crate skeleton into
focused crates for auth, CLI, conflict metadata, core scanning/policy, daemon live sync, Git
boundaries, materialization, hosted metadata, snapshot/restore, local store, and encrypted sync.
The original skeleton boundaries were:

- `crates/devbox-core`: shared domain types for projects, snapshots, blobs, manifests, policy decisions, and read-only project scanning.
- `crates/devbox-daemon`: daemon binary that now owns watch/debounce and live sync orchestration.
- `crates/devbox-cli`: `devbox` CLI binary that now exposes local snapshots, auth/session,
  pairing, hosted metadata, sync, conflict, and secret-policy commands.
- `crates/devbox-git`: Git adapter boundary. Git repositories must be inspected and reconstructed
  through a dedicated adapter rather than by syncing `.git` as ordinary files.
- `crates/devbox-store`: SQLite local metadata boundary with idempotent migrations, foreign-key
  enforcement, schema summary reporting, project/snapshot metadata, local identity, pairing state,
  cursors, conflicts, and secret policies.

## Desktop Boundary

The Electron app scaffold lives in `apps/desktop`.

Electron is the product shell. It may display project status, timelines, policy explanations, restore previews, and daemon health. It must not mutate workspace state directly. In particular, Electron should not write, delete, move, restore, or reconcile project files on its own.

The Rust daemon owns filesystem correctness:

- file watching and debouncing
- project scanning and classification
- policy evaluation
- content hashing and blob cache writes
- manifest creation
- snapshot timeline updates
- restore and rollback behavior
- Git-aware reconstruction

This keeps the riskiest behavior in a testable Rust core and prevents UI state from becoming the source of truth.

## Local Metadata and Storage Direction

SQLite is used for local metadata through the `devbox-store` crate. The initial schema records projects, snapshots, manifest entries, blob and chunk references, operations, policies, policy evaluations, and restore attempts. SQLite stores identifiers, paths, sizes, policy decisions, operation state, and object references. It does not store file contents as database BLOBs.

Blob content will be addressed by BLAKE3 hashes and stored in a future local content-addressed cache on disk. SQLite rows point at that cache through object references so metadata transactions stay small and content storage can evolve separately. Remote object storage will later sit behind an S3-compatible interface so Cloudflare R2 is an implementation choice, not a domain dependency.

The early CLI could conservatively inspect a local metadata database with:

```text
devbox status --db <PATH>
```

That command opens the SQLite store, applies migrations idempotently, and prints the schema version plus table counts. It does not create snapshots, hash files, restore files, or start the daemon.

## Current Non-Goals

This initial slice did not implement:

- file watching
- content hashing
- snapshot manifests beyond core type definitions
- restore logic
- sync
- Electron runtime wiring
- local Postgres or MinIO services

The next scanner slice is documented in [Project Scanner and Policy Foundation](project-scanner-policy.md).

Those pieces should land in later vertical slices with focused tests around each correctness boundary.
