# Devbox

Devbox is developer folder continuity: your code folder, work-in-progress, and local context should
follow you across machines.

The first product wedge is simple:

> Close desktop. Open laptop. Keep coding.

The product is about **shared folders**, not projects. A shared folder can be a big `~/Code` folder,
one repo, nested developer workspaces, or plain files. Devbox should account for developer tools such
as Git, dependencies, generated outputs, and secrets, but it should not make the user think about
those internals just to keep working.

The deeper source-control primitive underneath Devbox is codenamed **Loom**. Devbox is the product
experience; Loom is the direction for file versions, folder revisions, checkpoints, safe parallel
sandboxes, shared local overlays, and agent-friendly folder state. Git remains a compatibility surface because
developers use it today, not the foundation Devbox is trying to become. See
[docs/devbox-and-loom.md](docs/devbox-and-loom.md).

This repository currently contains the product foundation and MVP planning artifacts:

- [.product](.product/README.md) - product strategy, market sizing, KPIs, architecture, roadmap, and sources.
- [.plans](.plans/README.md) - MVP execution plan with static HTML pages for phases, architecture, and validation.
- [docs/alpha-cli-distribution.md](docs/alpha-cli-distribution.md) - GitHub Release packaging, R2/shared-bucket setup, and two-device alpha smoke testing.

## Current Stack Direction

- Core daemon: Rust
- Desktop app: Electron, React, TypeScript
- Local metadata: SQLite
- Local blob cache: content-addressed files on disk
- Local change feed: SQLite-backed pending file operations derived from the latest snapshot
- Backend: Rust API, Postgres
- Remote blob storage: Cloudflare R2 behind an S3-compatible interface; provider foundation exists
  for R2/S3/MinIO-compatible encrypted blobs. A hosted metadata API foundation now models
  accounts, devices, implementation folder scopes, published snapshot manifests, and server-side compare-and-set
  cursors with SQLite for dev/tests. The local/mock publish, import, and materialize flows can now
  opt into an in-process mock-dev SQLite metadata store for manifest discovery and cursor
  compare-and-set without network services. A production-shaped account ownership proof and account
  session boundary now models provider subject/email/domain proof, token-hash sessions, expiration,
  and revocation without live OAuth. Hosted metadata HTTP handlers now preserve explicit mock-dev
  header mode for tests/dev and also accept production-shaped `Authorization: Bearer <session-token>`
  account-session auth resolved through the hosted session store. Hosted metadata now also models
  managed object credential leases with account/session/folder-scope scoping, R2/S3/MinIO-shaped provider
  references, redacted credential references, expiration, revocation, and rotation generation.
  Hosted object-access resolution now returns account-session-authorized, folder-scoped shared
  bucket prefixes such as `accounts/<account-id>/projects/<project-id>` through a server-mediated
  broker boundary, and the hosted object transfer path now proxies encrypted object bytes through the
  metadata API so external tester clients need only a Devbox session token, not raw R2/S3 bucket
  keys. The local daemon now has a live sync loop that can scan/debounce a shared folder, persist an
  idempotent live snapshot, publish encrypted objects through local, trusted direct-S3, or hosted
  object-transfer remotes, register hosted mock-dev metadata, discover the latest published remote
  snapshot, and import or materialize it with cursor/conflict preflight.
  Local
  pairing now includes receiver-generated join/complete handoff, so a paired laptop can install its
  own local account key envelope and materialize without opening the publisher DB or sharing its
  local device key with the source. It also
  includes no-network recovery grants and device key-envelope rotation intents for future
  production pairing/recovery flows. The hosted metadata service can now run as a single-instance
  alpha API with one-time invite login, bearer session status/logout, `/ready`, and mock-dev auth
  disabled by default in the server binary. Alpha release packaging now produces macOS/Linux
  command-line tool archives containing `devbox`, `devbox-daemon`, `devbox-metadata`, helper
  scripts, docs, and an env template. A deterministic two-device smoke harness proves
  receiver-generated pairing, pending-receiver fail-closed behavior, live publish, latest remote
  discovery, and receiver materialization with redacted evidence logs. The private-alpha desktop
  shell now provides an Electron control surface for local DB/cache/folder paths, hosted
  API/session/folder config, R2/shared-bucket prefix state, pairing, live sync command state,
  conflicts, devices, secret policy, and settings. It reads redacted `DEVBOX_*` setup state and
  does not start sync or mutate files directly. The hosted metadata server now has a
  Railway-shaped Postgres backend selected by `DATABASE_URL`/`DEVBOX_METADATA_DATABASE_URL`, while
  SQLite stays available for local/dev tests. OAuth, live Cloudflare/AWS credential provisioning,
  signed installers, multi-region/observability hardening, automatic conflict resolution, and
  paid/team/agent/Git-replacement work remain deferred.

## Local MVP Surface

Language note: many current commands still use `project` because the first alpha schema used that
word for a scoped shared folder. New product language should say shared folder.

The current CLI can create/list/show/restore local snapshots and scan pending local changes:

- `devbox snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes scan --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes list --db <DB_PATH> [--project <PROJECT_ID>]`
- `devbox metadata check --endpoint <URL> [--auth-mode mock-dev-headers|account-session]`
- `devbox metadata alpha-invite create (--db <METADATA_DB>|--postgres-url-env <ENV>) --email <EMAIL>|--domain <DOMAIN>`
- `devbox metadata credential-lease mock-create (--db <METADATA_DB>|--postgres-url-env <ENV>) --session-token <TOKEN> --verified-email <EMAIL>|--verified-domain <DOMAIN> --project <PROJECT_ID> --lease <LEASE_ID> --endpoint <URL> --bucket <BUCKET>`
- `devbox metadata credential-lease check (--db <METADATA_DB>|--postgres-url-env <ENV>) --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>`
- `devbox metadata credential-lease rotate (--db <METADATA_DB>|--postgres-url-env <ENV>) --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>`
- `devbox metadata credential-lease revoke (--db <METADATA_DB>|--postgres-url-env <ENV>) --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>`
- `devbox metadata object-access resolve --api <URL> --session-token-env DEVBOX_SESSION_TOKEN --project <PROJECT_ID> --lease <LEASE_ID>`
- `devbox auth mock-verified-bootstrap --db <DB_PATH> --verified-email <EMAIL>|--verified-domain <DOMAIN> --session-token <TOKEN>`
- `devbox auth hosted-login --api <URL> --email <EMAIL> --invite-code-env <ENV>`
- `devbox auth hosted-status --api <URL> [--session-token-env <ENV>]`
- `devbox auth hosted-logout --api <URL> [--session-token-env <ENV>]`
- `devbox auth proof-check --db <DB_PATH> --session-token <TOKEN>`
- `devbox auth revoke-session --db <DB_PATH> <SESSION_ID>`
- `devbox devices invite --db <SOURCE_DB> [--ttl-seconds <SECONDS>]`
- `devbox devices join --db <RECEIVER_DB> --token-env <ENV> --device-name <NAME>`
- `devbox devices approve-join --db <SOURCE_DB> --token-env <ENV> --join-request-env <ENV> --device-name <NAME>`
- `devbox devices complete --db <RECEIVER_DB> --completion-env <ENV>`
- `devbox devices recovery create --db <DB_PATH> --device <DEVICE_ID> --recovery-ref <REDACTED_REF>`
- `devbox devices recovery revoke --db <DB_PATH> <GRANT_ID>`
- `devbox devices rotate-key-envelope --db <DB_PATH> --device <DEVICE_ID>`
- `devbox conflicts resolve --db <DB_PATH> <CONFLICT_ID> --manual-resolution keep-local|keep-incoming|keep-both|exported --confirm-no-auto-apply`
- `devbox secrets policy add --db <DB_PATH> --project <PROJECT_ID> --path <REL_PATH> --action block|template|envelope [--envelope-ref <REF>]`
- `devbox secrets policy list --db <DB_PATH> [--project <PROJECT_ID>]`
- `devbox-daemon watch --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT> [--once]`
- `devbox-daemon sync --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> [--metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>] [--push|--pull|--two-way] <PROJECT_ROOT> [--once]`
- `devbox-daemon sync --remote-kind hosted --object-access-api <URL> --object-access-lease <LEASE_ID> --metadata-mode hosted-api --metadata-api <URL> --metadata-project <PROJECT_ID> [--metadata-session-token-env DEVBOX_SESSION_TOKEN] ...`

Alpha helper scripts:

- `scripts/alpha-two-device-smoke.sh` runs a local two-device proof with pairing, pending receiver refusal, live publish, latest pull, materialization, and redacted evidence logs.
- `scripts/devbox-live-sync-alpha.sh` maps `.env` values into a live daemon command for local, hosted object-transfer, or trusted direct-S3 remotes.
- `scripts/package-cli.sh <VERSION>` builds macOS/Linux alpha tool archives with CLI, daemon, metadata server, docs, env template, and helper scripts.
- `scripts/package-desktop-alpha.sh <VERSION>` builds an unsigned Electron alpha control-surface bundle for macOS/Linux.

Hosted metadata sync wiring is explicit opt-in. Local deterministic smoke tests can use the
in-process SQLite store:

- `devbox sync publish-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>`
- `devbox sync import-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --metadata-account <ACCOUNT_ID>`
- `devbox sync materialize ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --metadata-account <ACCOUNT_ID>`
- `devbox-daemon sync --pull --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-account <ACCOUNT_ID> --metadata-project <PROJECT_ID> --to <TARGET_DIR> --apply ...`

External hosted alpha testers should use the account-session HTTP API instead of a shared metadata
DB:

- `devbox sync publish-snapshot ... --metadata-mode hosted-api --metadata-api <URL> [--metadata-session-token-env DEVBOX_SESSION_TOKEN]`
- `devbox sync import-snapshot ... --metadata-mode hosted-api --metadata-api <URL> --metadata-project <PROJECT_ID> [--metadata-session-token-env DEVBOX_SESSION_TOKEN]`
- `devbox sync materialize ... --metadata-mode hosted-api --metadata-api <URL> --metadata-project <PROJECT_ID> [--metadata-session-token-env DEVBOX_SESSION_TOKEN]`
- `devbox-daemon sync --remote-kind hosted ... --metadata-mode hosted-api --metadata-api <URL> --metadata-project <PROJECT_ID> ...`

For local/mock import/materialize, the mock-dev metadata account scope is either passed explicitly with
`--metadata-account <ACCOUNT_ID>` or derived from `--mock-key-source-db <PUBLISHER_DB>` for the
legacy local/mock trust bootstrap. New paired receiver flows should run `devices join`,
`devices approve-join`, and `devices complete` first; after completion the receiver can decrypt with
its own local key state and does not need `--mock-key-source-db`. Invite-based hosted alpha login,
session-auth hosted metadata request handling, and server-mediated object-access prefix grants now
exist. The grant is the authorization boundary for a shared bucket; raw R2/S3 credentials are not
returned to tester clients. Live daemon sync with `--remote-kind hosted` transfers encrypted object
bytes and resolves metadata through the metadata API using only the tester's session token on the
client; hosted API mode rejects tester-supplied `--metadata-account` and uses the authenticated
server session account. Trusted operators can still use `--remote-kind s3` for direct S3/R2 smoke
tests with local bucket env keys.
Railway/Postgres deployment is wired for the hosted metadata backend; OAuth, UI onboarding, and
multi-region/observability hardening remain deferred.

`changes scan` compares the current included regular files against the latest persisted snapshot
for the shared folder root. Created, modified, and deleted files become pending local operations in
SQLite. Generated dependencies, policy exclusions, symlinks, and unsupported filesystem nodes are
summarized but are not persisted as uploadable operations.
