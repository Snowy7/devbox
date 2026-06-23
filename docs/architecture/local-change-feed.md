# Local Change Feed Foundation

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Bindhub.

This foundation moves Bindhub from persisted manual snapshots toward a local answer to "what
changed?" The daemon watcher now calls the same shared scan orchestration after debounced filesystem
events; the persisted feed remains local-only.

## Boundary

The change feed is local-only. `bindhub changes scan --db <DB_PATH> --cache <CACHE_ROOT>
<PROJECT_ROOT>` and `bindhub-daemon watch --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>` both
build the current snapshot draft with the existing scanner, policy, manifest, BLAKE3 blob cache, and
SQLite store foundations. They then compare included regular files against the latest persisted
snapshot for the same local project id and replace that project's pending change rows in SQLite.

No source files are mutated by the scan. The scan may read files and write content-addressed blobs
to the local cache so current file content has a stable identity.

## Semantics

The diff is deterministic and file-oriented:

- `created`: an included regular file exists now but was not in the latest included-file snapshot.
- `modified`: an included regular file exists in both states but has a different blob id or size.
- `deleted`: an included regular file from the base snapshot is missing from the current included
  file set.
- `unchanged`: an included regular file exists in both states with the same blob id and size.
- `skipped/deferred`: policy exclusions, generated dependency directories, symlinks, unsupported
  filesystem nodes, and secret-blocked files. These are summarized but not persisted as uploadable
  operations.

Pending operations are replaced per project on every scan. Re-running a scan with the same filesystem
state therefore keeps the feed stable instead of creating duplicate rows.

## Storage

Schema version 3 adds `pending_local_changes`. Rows are constrained to pending file operations:
created/modified rows require a current blob id and object ref, while deleted rows do not. The table
stores the project id, optional base snapshot id, path, operation kind, previous blob id when
available, current blob id when available, byte count, and detection timestamp.

The existing broad `operations` table remains available for future workflow lifecycle records. The
new table is intentionally narrower: it is the daemon/cloud-sync input queue for pending local file
changes, not a general task log. The watcher only feeds this queue; it does not make pending rows
durable cloud operations.

## Safety

The CLI preserves the existing cache/database preflight checks: the blob cache root and SQLite
database path must not live inside the project being scanned. Generated dependency directories are
excluded by policy and are not treated as sync operations. Symlinks and unsupported filesystem nodes
remain deferred and are never persisted as uploadable file changes.

Secret-blocked regular files have no blob id or object ref, so they are never persisted as
uploadable created or modified pending changes.

The scan and watcher do not write cloud objects, delete user files, restore files, or mutate source
trees.

## Deferred Work

This foundation deliberately does not implement:

- cloud upload/download, R2/S3, encryption, compression, packfiles, or garbage collection
- conflict detection or resolution
- cross-device sync
- teams, agents, or shared workspaces
- Loom source-control behavior
- Electron or desktop UI surfaces
