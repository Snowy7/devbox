# Devbox

Devbox is a developer-native workspace continuity project: your code folder, work-in-progress, and project context should follow you across machines.

The first product wedge is simple:

> Close desktop. Open laptop. Keep coding.

This repository currently contains the product foundation and MVP planning artifacts:

- [.product](.product/README.md) - product strategy, market sizing, KPIs, architecture, roadmap, and sources.
- [.plans](.plans/README.md) - MVP execution plan with static HTML pages for phases, architecture, and validation.
- [docs/alpha-cli-distribution.md](docs/alpha-cli-distribution.md) - GitHub Release packaging for alpha CLI binaries.

## Current Stack Direction

- Core daemon: Rust
- Desktop app: Electron, React, TypeScript
- Local metadata: SQLite
- Local blob cache: content-addressed files on disk
- Local change feed: SQLite-backed pending file operations derived from the latest snapshot
- Backend: Rust API, Postgres
- Remote blob storage: Cloudflare R2 behind an S3-compatible interface; provider foundation exists
  for R2/S3/MinIO-compatible encrypted blobs. A hosted metadata API foundation now models
  accounts, devices, projects, published snapshot manifests, and server-side compare-and-set
  cursors with SQLite for dev/tests. The local/mock publish, import, and materialize flows can now
  opt into an in-process mock-dev SQLite metadata store for manifest discovery and cursor
  compare-and-set without network services. A production-shaped account ownership proof and account
  session boundary now models provider subject/email/domain proof, token-hash sessions, expiration,
  and revocation without live OAuth. Hosted metadata HTTP handlers now preserve explicit mock-dev
  header mode for tests/dev and also accept production-shaped `Authorization: Bearer <session-token>`
  account-session auth resolved through the hosted session store. Hosted metadata now also models
  managed object credential leases with account/session/project scoping, R2/S3/MinIO-shaped provider
  references, redacted credential references, expiration, revocation, and rotation generation.
  Hosted object-access resolution now returns account-session-authorized, project-scoped shared
  bucket prefixes such as `accounts/<account-id>/projects/<project-id>` through a server-mediated
  broker boundary, and fails closed unless the metadata server has server-managed object credentials
  configured. The local daemon now has a live sync loop that can scan/debounce a project, persist an
  idempotent live snapshot, publish encrypted objects, register hosted mock-dev metadata, discover
  the latest published remote snapshot, and import or materialize it with cursor/conflict preflight.
  Local
  pairing now includes receiver-generated join/complete handoff, so a paired laptop can install its
  own local account key envelope and materialize without opening the publisher DB or sharing its
  local device key with the source. It also
  includes no-network recovery grants and device key-envelope rotation intents for future
  production pairing/recovery flows. The hosted metadata service can now run as a single-instance
  alpha API with one-time invite login, bearer session status/logout, `/ready`, and mock-dev auth
  disabled by default in the server binary. The private-alpha desktop shell now provides a
  no-network Electron control surface for status, projects, sync activity, conflicts, devices,
  secret policy, and settings. OAuth, live Cloudflare/AWS credential provisioning, hosted object
  proxy/signed URL data transfer, Postgres-backed
  production deployment hardening, automatic conflict resolution, and paid/team/agent/Git-replacement
  work remain deferred.

## Local MVP Surface

The current CLI can create/list/show/restore local snapshots and scan pending local changes:

- `devbox snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes scan --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes list --db <DB_PATH> [--project <PROJECT_ID>]`
- `devbox metadata check --endpoint <URL> [--auth-mode mock-dev-headers|account-session]`
- `devbox metadata alpha-invite create --db <METADATA_DB> --email <EMAIL>|--domain <DOMAIN>`
- `devbox metadata credential-lease mock-create --db <METADATA_DB> --session-token <TOKEN> --verified-email <EMAIL>|--verified-domain <DOMAIN> --project <PROJECT_ID> --lease <LEASE_ID> --endpoint <URL> --bucket <BUCKET>`
- `devbox metadata credential-lease check --db <METADATA_DB> --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>`
- `devbox metadata credential-lease rotate --db <METADATA_DB> --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>`
- `devbox metadata credential-lease revoke --db <METADATA_DB> --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>`
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

Hosted metadata sync wiring is explicit opt-in for dev/test flows:

- `devbox sync publish-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>`
- `devbox sync import-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --metadata-account <ACCOUNT_ID>`
- `devbox sync materialize ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --metadata-account <ACCOUNT_ID>`
- `devbox-daemon sync --pull --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-account <ACCOUNT_ID> --metadata-project <PROJECT_ID> --to <TARGET_DIR> --apply ...`

For import/materialize, the hosted metadata account scope is either passed explicitly with
`--metadata-account <ACCOUNT_ID>` or derived from `--mock-key-source-db <PUBLISHER_DB>` for the
legacy local/mock trust bootstrap. New paired receiver flows should run `devices join`,
`devices approve-join`, and `devices complete` first; after completion the receiver can decrypt with
its own local key state and does not need `--mock-key-source-db`. Invite-based hosted alpha login,
session-auth hosted metadata request handling, and server-mediated object-access prefix grants now
exist. The grant is the authorization boundary for a shared bucket; raw R2/S3 credentials are still
not returned to tester clients. Live daemon sync with `--remote-kind s3` requires an object-access
API/lease preflight and an explicit `--s3-prefix` matching the grant, while direct manual
`devbox sync` S3 commands remain a trusted-operator smoke path. OAuth, object proxy/signed URL
transfer, production deployment hardening, and UI onboarding remain deferred.

`changes scan` compares the current included regular files against the latest persisted snapshot
for the project root. Created, modified, and deleted files become pending local operations in
SQLite. Generated dependencies, policy exclusions, symlinks, and unsupported filesystem nodes are
summarized but are not persisted as uploadable operations.
