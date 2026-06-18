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

It does not implement live OAuth-backed account login, managed R2/S3 credential provisioning,
production pairing UX, or automatic conflict resolution. The private-alpha Electron shell can show
fixture-backed continuation, conflict, device, and safety state; this materialization path itself can
now opt into an in-process mock-dev hosted metadata store for manifest discovery and server-side
cursor compare-and-set, but the default remains local/mock only. Production-shaped account/session
proof primitives exist for future hosted auth, but this materialization path does not enforce them
yet. Local preflight now refuses divergent local/mock import/materialize paths and persists local
conflict records, but it does not resolve or merge them. The manual smoke path can use
`--mock-key-source-db` so a receiving local context can decrypt a publisher's locally encrypted
objects without printing raw key material. That flag is a test/dev trust bootstrap, not a production
key exchange.

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

Object names are deterministic and safe for both the local filesystem provider and the
S3-compatible provider. They are a foundation layout, not the final hosted metadata indexing model.

## Import And Cursor

`sync import-snapshot` decrypts the bundle manifest, checks the receiving local device/project
cursor and latest local snapshot, downloads all included file blobs into the receiving `BlobCache`
only when preflight allows, persists the project/snapshot/manifest rows into the receiving SQLite
database, and updates the receiving local device/project cursor to the imported snapshot id.

Import is idempotent for an already persisted snapshot id. Re-running import can refill missing
receiver cache blobs while leaving the existing snapshot metadata intact.

If preflight finds that local and incoming snapshots both diverged from the known cursor/base,
import persists the metadata needed for a readable conflict record, returns refusal, leaves the
cursor unchanged, and does not download file blobs into the receiver cache.

## Materialization

`devbox sync materialize` imports first, then delegates planning and apply to
`devbox-snapshot` restore logic. Its internal import does not advance the cursor; materialize
commits the cursor only after restore planning and the requested `--apply` behavior succeed. The
existing restore safety rules still apply:

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

## Hosted Metadata Opt-In

For dev/test wiring, add `--metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>` to publish.
For import/materialize, also pass `--metadata-project <PROJECT_ID>` and either
`--metadata-account <ACCOUNT_ID>` or the existing `--mock-key-source-db <PUBLISHER_DB>` local/mock
trust bootstrap. That account/project scope lets the manifest object key be looked up from
publisher-scoped hosted metadata instead of derived locally. Cursor advancement uses hosted
compare-and-set first under the hosted account scope and receiver device id; if the hosted cursor is
stale, the local cursor remains unchanged.

## Deferred

Remaining Phase 1 work includes:

- live OAuth/OIDC account ownership verification and production key exchange
- managed R2/S3 credential provisioning, rotation, and hosted object indexing
- production pairing/recovery UX and live recovery/rotation exchange
- automatic conflict merge/apply resolution
- live daemon-backed desktop materialization actions
