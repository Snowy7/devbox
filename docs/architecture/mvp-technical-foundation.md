# MVP Technical Foundation

This repository now has the first PR-sized technical skeleton for Devbox. It is intentionally small: the goal is to establish crate boundaries, local domain vocabulary, and future app layout without implementing snapshot, restore, sync, or policy execution yet.

## Rust Workspace

The Rust workspace is rooted at `Cargo.toml` and contains four crates:

- `crates/devbox-core`: shared domain types for projects, snapshots, blobs, manifests, policy decisions, and read-only project scanning.
- `crates/devbox-daemon`: placeholder daemon binary that will own filesystem watching, snapshot creation, restore, and local metadata writes.
- `crates/devbox-cli`: `devbox` CLI binary with `--version`, read-only `scan <path>`, and placeholders for the future commands `snapshot`, `status`, `restore`, and `explain`.
- `crates/devbox-git`: placeholder Git adapter boundary. Git repositories must be inspected and reconstructed through a dedicated adapter rather than by syncing `.git` as ordinary files.

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

SQLite will be used for local metadata when snapshot implementation begins. Blob content will be addressed by BLAKE3 hashes and stored in a local content-addressed cache. Remote object storage will later sit behind an S3-compatible interface so Cloudflare R2 is an implementation choice, not a domain dependency.

## Current Non-Goals

This slice does not implement:

- file watching
- SQLite schema
- content hashing
- snapshot manifests beyond core type definitions
- restore logic
- sync
- Electron runtime wiring
- local Postgres or MinIO services

The next scanner slice is documented in [Project Scanner and Policy Foundation](project-scanner-policy.md).

Those pieces should land in later vertical slices with focused tests around each correctness boundary.
