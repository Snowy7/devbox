# Second-Device Materialization Foundation

This slice completes the first local/mock Phase 1 vertical path for second-device continuation:

- publish a persisted snapshot through the encrypted local filesystem remote
- import the encrypted bundle into a separate local SQLite database and blob cache
- run local sync preflight against the receiving device/project cursor before importing file blobs
  or materializing workspace bytes
- update the receiving device/project cursor
- materialize the imported snapshot with the existing restore engine

## Boundary

This is still a local/mock foundation.

It does not implement production account ownership proof, hosted metadata, real R2/S3 credentials,
server-side cursors, production pairing UX, automatic conflict resolution, or Electron UI. Local
preflight now refuses divergent local/mock import/materialize paths and persists local conflict
records, but it does not resolve or merge them. The manual smoke path can use
`--mock-key-source-db` so a receiving local context can decrypt a publisher's locally encrypted
objects without printing raw key material. That flag is a test/dev trust bootstrap, not a
production key exchange.

## Domain Boundary

`crates/devbox-materialize` composes the existing foundations:

- `devbox-store` loads and persists snapshot/project/manifest metadata
- `devbox-store::BlobCache` owns local plaintext cache bytes
- `devbox-sync` encrypts/decrypts remote snapshot bundle and file blob objects
- `devbox-snapshot` plans and applies safe restore/materialization

The materialization crate owns only the orchestration model:

- publish/import/materialize request structs
- publish/import/materialize result structs
- encrypted bundle envelope serialization
- typed errors for missing snapshots, invalid bundles, remote object failures, and restore failures
- local preflight refusal when the receiver cursor/local snapshot and incoming snapshot diverge

It does not write workspace files directly.

## Published Bundle

Publishing reads an already persisted snapshot from SQLite and writes:

- one encrypted snapshot bundle object containing project, snapshot, manifest, and included blob
  references
- one encrypted remote object per included file blob, using the existing encrypted blob transport

Remote manifest bytes must not contain plaintext file bytes or obvious path strings. Generated
directories such as `.git`, `node_modules`, and build outputs remain excluded/deferred by the
snapshot policy and are not materialized as included content.

Secret-blocked files are also not materialized as included content. They persist in the encrypted
bundle as manifest policy entries with no included blob reference, so publish/import can explain the
policy decision without uploading or restoring the blocked file bytes.

Object names are deterministic and safe for the local filesystem provider. They are intentionally
not a final cloud layout.

## Import And Cursor

Import decrypts the bundle manifest, checks the receiving local device/project cursor and latest
local snapshot, downloads all included file blobs into the receiving `BlobCache` only when preflight
allows, persists the project/snapshot/manifest rows into the receiving SQLite database, and updates
the receiving local device/project cursor to the imported snapshot id.

Import is idempotent for an already persisted snapshot id. Re-running import can refill missing
receiver cache blobs while leaving the existing snapshot metadata intact.

If preflight finds that local and incoming snapshots both diverged from the known cursor/base,
import persists the metadata needed for a readable conflict record, returns refusal, leaves the
cursor unchanged, and does not download file blobs into the receiver cache.

## Materialization

`devbox sync materialize` imports first, then delegates planning and apply to
`devbox-snapshot` restore logic. The existing restore safety rules still apply:

- targets must be missing or empty directories for apply
- existing files are never overwritten
- unsafe manifest paths are rejected
- missing blobs block apply
- symlinks, unsupported entries, excluded paths, and entries requiring user decisions are skipped

## CLI Smoke Path

```text
devbox init --db <SOURCE_DB> --device-name Desk
devbox init --db <RECEIVER_DB> --device-name Laptop
devbox snapshot --db <SOURCE_DB> --cache <SOURCE_CACHE> <PROJECT_ROOT>
devbox sync publish-snapshot --db <SOURCE_DB> --cache <SOURCE_CACHE> --remote <REMOTE_DIR> <SNAPSHOT_ID>
devbox sync import-snapshot --db <RECEIVER_DB> --cache <RECEIVER_CACHE> --remote <REMOTE_DIR> --mock-key-source-db <SOURCE_DB> <SNAPSHOT_ID>
devbox sync materialize --db <RECEIVER_DB> --cache <RECEIVER_CACHE> --remote <REMOTE_DIR> --to <TARGET> --mock-key-source-db <SOURCE_DB> <SNAPSHOT_ID> --apply
```

The same imported snapshot can also be materialized with:

```text
devbox snapshot restore --db <RECEIVER_DB> --cache <RECEIVER_CACHE> --to <TARGET> <SNAPSHOT_ID> --apply
```

## Deferred

Remaining Phase 1 work includes:

- production account ownership and key exchange
- hosted metadata service and server-side cursor reconciliation
- real R2/S3 credentials and final object layout
- production pairing UX, recovery, and rotation
- automatic conflict merge/apply resolution and user-facing conflict flows
- minimal tray/Electron continuation UI
- explicit path-scoped secret allow/template/envelope policy
