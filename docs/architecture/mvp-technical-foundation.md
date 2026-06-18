# MVP Technical Foundation

This repository now has the first PR-sized technical skeleton for Devbox. It is intentionally small: the goal is to establish crate boundaries, local domain vocabulary, and future app layout without implementing snapshot, restore, sync, or policy execution yet.

## Rust Workspace

The Rust workspace is rooted at `Cargo.toml` and contains four crates:

- `crates/devbox-core`: shared domain types for projects, snapshots, blobs, manifests, policy decisions, and read-only project scanning.
- `crates/devbox-daemon`: placeholder daemon binary that will own filesystem watching, snapshot creation, restore, and local metadata writes.
- `crates/devbox-cli`: `devbox` CLI binary with `--version`, read-only `scan <path>`, and placeholders for the future commands `snapshot`, `status`, `restore`, and `explain`.
- `crates/devbox-git`: placeholder Git adapter boundary. Git repositories must be inspected and reconstructed through a dedicated adapter rather than by syncing `.git` as ordinary files.
- `crates/devbox-store`: SQLite local metadata boundary with idempotent migrations, foreign-key enforcement, schema summary reporting, and a small project/snapshot metadata API.

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

The CLI can conservatively inspect a local metadata database with:

```text
devbox status --db <PATH>
```

That command opens the SQLite store, applies migrations idempotently, and prints the schema version plus table counts. It does not create snapshots, hash files, restore files, or start the daemon.

## Current Non-Goals

This slice does not implement:

- file watching
- content hashing
- snapshot manifests beyond core type definitions
- restore logic
- sync
- Electron runtime wiring
- local Postgres or MinIO services

The next scanner slice is documented in [Project Scanner and Policy Foundation](project-scanner-policy.md).

Those pieces should land in later vertical slices with focused tests around each correctness boundary.
