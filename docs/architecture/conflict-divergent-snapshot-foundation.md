# Conflict as Divergent Snapshots Foundation

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Bindhub.

This Phase 1 slice adds a local conflict model for comparing and persisting divergent snapshots.
It is a compare-and-record foundation, not an automatic merge engine.

## Boundary

The foundation is local-first and metadata-only:

- `Bindhub-conflict` owns deterministic comparison semantics over snapshot manifest metadata.
- `Bindhub-store` owns SQLite conflict records and comparison rows.
- `bindhub-cli` exposes manual/scriptable conflict commands and guarded manual resolution records.

It does not implement production auth, hosted conflict metadata, server-side cursors, R2/S3,
automatic merge, Loom UX, or materialization into non-empty targets. The private-alpha
Electron shell can display conflict records and manual CLI paths, but real daemon-driven conflict UI
and automatic resolution remain deferred.

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

Status transitions are intentionally small: `open`, `resolved`, and `dismissed`. Marking a conflict
resolved requires a manual-resolution mode plus an explicit `--confirm-no-auto-apply` acknowledgement.
Bindhub does not merge or apply file bytes as part of that status transition.

## CLI Surface

```text
bindhub conflicts compare --db <DB_PATH> --local <LOCAL_SNAPSHOT_ID> --incoming <INCOMING_SNAPSHOT_ID> [--base <BASE_SNAPSHOT_ID>]
bindhub conflicts list --db <DB_PATH> [--project <PROJECT_ID>]
bindhub conflicts show --db <DB_PATH> <CONFLICT_ID>
bindhub conflicts resolve --db <DB_PATH> <CONFLICT_ID> --manual-resolution keep-local|keep-incoming|keep-both|exported --confirm-no-auto-apply
bindhub conflicts dismiss --db <DB_PATH> <CONFLICT_ID>
```

The output is plain text and tabular so future daemon/sync code can call the same model before
refusing unsafe overwrites.

Manual resolution options are records of what the user did outside automatic apply semantics:

- `keep-local`
- `keep-incoming`
- `keep-both`
- `exported`

Normal output prints snapshot ids, paths, row counts, and redacted policy reasons only. It does not
print source file contents or secret material.

`bindhub sync preflight` and the local/mock import/materialize path now call this model when the
receiving device cursor and local snapshot diverge from an incoming snapshot. Blocked preflight
persists the same idempotent conflict record, prints the conflict id and summary counts, refuses the
sync operation, and leaves the cursor unchanged.

## Deferred

Remaining work includes:

- merge planning and apply semantics
- hosted conflict metadata and cross-device conflict service
- richer Git-aware compare views
- automatic conflict resolution
