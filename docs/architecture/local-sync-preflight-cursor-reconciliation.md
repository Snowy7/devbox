# Local Sync Preflight and Cursor Reconciliation

This Phase 1 slice makes the local divergent-snapshot conflict model active in the local/mock
second-device path.

## Boundary

The implementation is local-first:

- `devbox-materialize` owns sync preflight orchestration for local/mock import and materialize.
- `devbox-conflict` still owns deterministic snapshot comparison.
- `devbox-store` still owns local SQLite cursors, snapshots, conflicts, and conflict rows.
- `devbox-cli` exposes a scriptable `devbox sync preflight` command.

It does not add production auth, managed R2/S3 credential provisioning, Electron UI, automatic
merge/apply resolution, Git replacement UX, or a hosted conflict service. A hosted metadata API
foundation now models server-side compare-and-set cursors, and the opt-in mock-dev sync wiring can
use that hosted cursor CAS. The preflight comparison and conflict records remain local.

## Preflight Semantics

Preflight receives a project id, optional base snapshot id, optional local snapshot id, and incoming
snapshot id.

The operation is allowed when:

- there is no local snapshot state for the project;
- the local snapshot already equals the incoming snapshot;
- the local snapshot still equals the known base/cursor; or
- the incoming snapshot equals the known base/cursor.

The operation is blocked when the local snapshot and incoming snapshot both diverge from the known
base/cursor. If the base is unknown, preflight is conservative: a different local snapshot and
incoming snapshot are treated as unsafe and produce a conflict record.

Blocked preflight persists a readable divergent-snapshot conflict record and returns refusal. The
record is idempotent for the same project/base/local/incoming tuple, so repeating preflight returns
the same conflict id without duplicating rows.

## Import and Materialize

`sync import-snapshot` decrypts the published manifest enough to learn the incoming project and
snapshot metadata, then checks the receiver's current device/project cursor and latest local
snapshot for that project. If preflight blocks, Devbox persists metadata needed for the conflict
record, refuses the import, does not download file blobs into the receiver cache, and does not
advance the cursor.

`sync materialize` uses the same import path before restore planning. Existing restore safety still
applies after a safe import: non-empty targets, unsafe paths, missing blobs, symlinks, unsupported
entries, generated paths, and secret-blocked entries remain blocked or skipped by the restore
engine.

On successful `sync import-snapshot`, the receiving device/project cursor advances to the imported
snapshot as before. `sync materialize` imports the bundle, blobs, and metadata without advancing the
cursor, then commits the cursor only after restore planning and the requested `--apply` behavior
succeed. If restore safety or apply fails, the cursor remains at the previous value.

When hosted mock-dev metadata mode is enabled, cursor commit first sends the local expected cursor
to hosted metadata compare-and-set. A stale hosted cursor conflict refuses the operation and leaves
the local cursor unchanged.

## Secret and Metadata Safety

Conflict records remain metadata-only. They store snapshot ids, paths, entry kinds, blob ids,
sizes, policy decisions, and redacted policy reasons. They do not store raw file bytes or raw secret
values. Secret-blocked manifest entries keep their policy-blocked/deferred shape and are not made
uploadable or materialized by preflight.

## CLI

The scriptable command is:

```text
devbox sync preflight --db <DB_PATH> --project <PROJECT_ID> --local <LOCAL_SNAPSHOT_ID> --incoming <INCOMING_SNAPSHOT_ID> [--base <BASE_SNAPSHOT_ID>]
```

It prints stable plain text beginning with `Preflight: ok` or `Preflight: blocked`, followed by the
project id, base/local/incoming snapshot ids, conflict id when blocked, and summary counts.

Blocked `sync preflight`, `sync import-snapshot`, and `sync materialize` exit non-zero after
printing the preflight block. They do not attempt automatic merge or resolution.
