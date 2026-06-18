# Conflict as Divergent Snapshots Foundation

This Phase 1 slice adds a local conflict model for comparing and persisting divergent snapshots.
It is a compare-and-record foundation, not an automatic merge engine.

## Boundary

The foundation is local-first and metadata-only:

- `devbox-conflict` owns deterministic comparison semantics over snapshot manifest metadata.
- `devbox-store` owns SQLite conflict records and comparison rows.
- `devbox-cli` exposes manual/scriptable conflict commands.

It does not implement production auth, hosted conflict metadata, server-side cursors, R2/S3,
Electron conflict UI, automatic merge, Git replacement UX, or materialization into non-empty
targets.

## Comparison Semantics

Conflicts compare persisted snapshots for the same project. An optional base snapshot can be
provided when the caller knows the common ancestor.

Rows are path-oriented and deterministic. Each row records only manifest metadata:

- path
- entry kind
- state
- base/local/incoming blob ids when present
- base/local/incoming byte counts when present
- local/incoming policy decisions and redacted reasons

Source file bytes are never read from the blob cache and never printed. Blocked secret entries stay
policy-blocked rows with redacted policy evidence and no blob id.

Current row states are:

- `same`
- `local-only`
- `incoming-only`
- `local-deleted`
- `incoming-deleted`
- `both-modified-same`
- `both-modified-different`
- `policy-excluded`
- `policy-deferred`
- `policy-blocked`
- `unsupported`

Policy, generated-directory, symlink/deferred, secret-blocked, and unsupported manifest entries are
not promoted into normal file conflicts.

## Persistence

Schema version 7 adds:

- `conflicts`
- `conflict_rows`

A conflict record stores project id, optional base snapshot id, local snapshot id, incoming snapshot
id, status, created/updated timestamps, and summary counts. Rows store the deterministic path
comparison output needed to rehydrate the readable comparison without source bytes.

Creation is idempotent for the same project/base/local/incoming tuple. The stable conflict id uses
that tuple, and the SQLite schema also enforces tuple uniqueness. Re-running compare returns the
existing record and does not duplicate rows.

Status transitions are intentionally small: `open`, `resolved`, and `dismissed`.

## CLI Surface

```text
devbox conflicts compare --db <DB_PATH> --local <LOCAL_SNAPSHOT_ID> --incoming <INCOMING_SNAPSHOT_ID> [--base <BASE_SNAPSHOT_ID>]
devbox conflicts list --db <DB_PATH> [--project <PROJECT_ID>]
devbox conflicts show --db <DB_PATH> <CONFLICT_ID>
devbox conflicts resolve --db <DB_PATH> <CONFLICT_ID>
devbox conflicts dismiss --db <DB_PATH> <CONFLICT_ID>
```

The output is plain text and tabular so future daemon/sync code can call the same model before
refusing unsafe overwrites.

`devbox sync preflight` and the local/mock import/materialize path now call this model when the
receiving device cursor and local snapshot diverge from an incoming snapshot. Blocked preflight
persists the same idempotent conflict record, prints the conflict id and summary counts, refuses the
sync operation, and leaves the cursor unchanged.

## Deferred

Remaining work includes:

- merge planning and apply semantics
- user-facing conflict UI
- hosted conflict metadata and cross-device conflict service
- richer Git-aware compare views
- explicit secret allow/template/envelope policies
