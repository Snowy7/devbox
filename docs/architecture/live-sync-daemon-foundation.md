# Live Sync Daemon Foundation

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Devbox.

This Phase 1 slice moves Devbox from manual publish/import commands toward "things are always
synced" while keeping the alpha boundary conservative.

## Scope

`devbox-daemon sync` watches or scans a project root and runs a bounded live sync cycle:

1. scan the local tree with the shared local change-feed scanner
2. refuse unsafe cache/database paths and pending pairing identities
3. persist the current snapshot with reason `live-sync`
4. publish encrypted blobs and the encrypted manifest bundle
5. optionally register hosted mock-dev metadata
6. optionally discover the latest remote snapshot from hosted metadata
7. import or materialize the remote snapshot with existing cursor/conflict preflight

The deterministic alpha smoke path is:

```text
scripts/alpha-two-device-smoke.sh
```

That harness exercises receiver-generated pairing, pending-receiver fail-closed behavior, source
live publish, latest remote discovery, receiver materialization, and redacted evidence logs. The
lower-level daemon push command is:

```text
devbox-daemon sync \
  --db <DB_PATH> \
  --cache <CACHE_ROOT> \
  --remote <REMOTE_DIR> \
  --metadata-mode mock-dev-sqlite \
  --metadata-db <METADATA_DB> \
  --push \
  --once \
  <PROJECT_ROOT>
```

Receivers can discover the latest published snapshot for the remote project:

```text
devbox-daemon sync \
  --db <RECEIVER_DB> \
  --cache <RECEIVER_CACHE> \
  --remote <REMOTE_DIR> \
  --metadata-mode mock-dev-sqlite \
  --metadata-db <METADATA_DB> \
  --metadata-account <ACCOUNT_ID> \
  --metadata-project <PROJECT_ID> \
  --pull \
  --to <TARGET_DIR> \
  --apply \
  --once \
  <TARGET_DIR>
```

External hosted alpha sync swaps the shared mock metadata DB for the live account-session API:

```text
devbox-daemon sync \
  --db <DB_PATH> \
  --cache <CACHE_ROOT> \
  --remote-kind hosted \
  --object-access-api <URL> \
  --object-access-lease <LEASE_ID> \
  --metadata-mode hosted-api \
  --metadata-api <URL> \
  --metadata-project <PROJECT_ID> \
  --metadata-session-token-env DEVBOX_SESSION_TOKEN \
  --push \
  --once \
  <PROJECT_ROOT>
```

Hosted mode transfers encrypted object bytes through the metadata API and derives metadata account
scope from the authenticated session. It does not require local R2 keys or a shared metadata SQLite
database.

Without `--once`, the daemon uses the same recursive filesystem notifications, debounce planner,
idle timeout, and bounded loop controls as `watch`. Filesystem events remain hints; each cycle
reconciles by scanning the tree.

## Safety

Live sync fails closed when:

- the local SQLite database or blob cache is inside the watched project
- the local identity is missing or still pending pairing completion
- snapshot construction finds secret-blocked entries
- pull sees local pending changes before import/materialization
- hosted latest-snapshot discovery is requested without mock-dev metadata configuration
- cursor preflight detects divergent local and incoming snapshots
- S3 live sync is missing object-access API/lease configuration or a grant-matching prefix

Successful live publish clears pending local changes for the published project, because the current
snapshot has become the remote candidate. Manual `devbox sync publish-snapshot` remains unchanged.

## Hosted Metadata Discovery

`devbox-metadata` now supports latest published snapshot lookup by `(account_id, project_id)`,
including:

```text
GET /v1/projects/:project_id/snapshots/latest
```

The daemon uses the SQLite mock-dev store directly in local tests. A hosted client can use the HTTP
route later without changing the discovery contract.

## Shared Bucket Boundary

For external tester object transfer, `--remote-kind hosted` requires:

- `--object-access-api <URL>`
- `--object-access-lease <LEASE_ID>`
- `--object-access-session-token-env <ENV>` (defaults to `DEVBOX_SESSION_TOKEN`)
- `--metadata-project <PROJECT_ID>`

The daemon resolves the account-session object-access grant before opening the hosted object provider.
The metadata API keeps bucket credentials server-side and enforces account/project prefix scope,
object-key safety, lease state, and read/write/head/list capabilities on each encrypted object
operation.

For trusted-operator direct R2/S3 smoke, `--remote-kind s3` requires:

- `--s3-prefix accounts/<account-id>/projects/<project-id>`
- `--object-access-api <URL>`
- `--object-access-lease <LEASE_ID>`
- `--object-access-session-token-env <ENV>` (defaults to `DEVBOX_SESSION_TOKEN`)

The daemon resolves the account-session object-access grant before opening the S3 provider and
verifies the grant bucket, region, project, account when supplied, and prefix. The grant does not
return raw object credentials. The direct S3 provider still loads trusted-operator credentials from
environment variable names and is not the external tester path.

## Deferred

This is an alpha automation foundation, not full Dropbox semantics. Deferred work remains:

- background retry queues, durable leases, and resumable transfer state
- automatic merge/apply resolution for non-empty divergent worktrees
- Electron tray controls and daemon IPC
- production OAuth/OIDC onboarding and provider-backed credential provisioning
