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
  references, redacted credential references, expiration, revocation, and rotation generation. Local
  pairing now includes no-network recovery grants and device key-envelope rotation intents for future
  production pairing/recovery flows. The private-alpha desktop shell now provides a no-network
  Electron control surface for status, projects, sync activity, conflicts, devices, secret policy,
  and settings. Live sign-in, live Cloudflare/AWS credential provisioning, production deployment
  hardening, automatic conflict resolution, and paid/team/agent/Git-replacement work remain
  deferred.

## Local MVP Surface

The current CLI can create/list/show/restore local snapshots and scan pending local changes:

- `devbox snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes scan --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes list --db <DB_PATH> [--project <PROJECT_ID>]`
- `devbox metadata check --endpoint <URL> [--auth-mode mock-dev-headers|account-session]`
- `devbox metadata credential-lease mock-create --db <METADATA_DB> --session-token <TOKEN> --verified-email <EMAIL>|--verified-domain <DOMAIN> --project <PROJECT_ID> --lease <LEASE_ID> --endpoint <URL> --bucket <BUCKET>`
- `devbox metadata credential-lease check --db <METADATA_DB> --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>`
- `devbox metadata credential-lease rotate --db <METADATA_DB> --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>`
- `devbox metadata credential-lease revoke --db <METADATA_DB> --session-token <TOKEN> --project <PROJECT_ID> --lease <LEASE_ID>`
- `devbox auth mock-verified-bootstrap --db <DB_PATH> --verified-email <EMAIL>|--verified-domain <DOMAIN> --session-token <TOKEN>`
- `devbox auth proof-check --db <DB_PATH> --session-token <TOKEN>`
- `devbox auth revoke-session --db <DB_PATH> <SESSION_ID>`
- `devbox devices recovery create --db <DB_PATH> --device <DEVICE_ID> --recovery-ref <REDACTED_REF>`
- `devbox devices recovery revoke --db <DB_PATH> <GRANT_ID>`
- `devbox devices rotate-key-envelope --db <DB_PATH> --device <DEVICE_ID>`
- `devbox conflicts resolve --db <DB_PATH> <CONFLICT_ID> --manual-resolution keep-local|keep-incoming|keep-both|exported --confirm-no-auto-apply`
- `devbox secrets policy add --db <DB_PATH> --project <PROJECT_ID> --path <REL_PATH> --action block|template|envelope [--envelope-ref <REF>]`
- `devbox secrets policy list --db <DB_PATH> [--project <PROJECT_ID>]`

Hosted metadata sync wiring is explicit opt-in for dev/test flows:

- `devbox sync publish-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>`
- `devbox sync import-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --mock-key-source-db <PUBLISHER_DB>`
- `devbox sync materialize ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --mock-key-source-db <PUBLISHER_DB>`

For import/materialize, the hosted metadata account scope is either passed explicitly with
`--metadata-account <ACCOUNT_ID>` or derived from `--mock-key-source-db <PUBLISHER_DB>` for the
local/mock trust bootstrap. Production-shaped account/session proof primitives and session-auth
hosted metadata request handling exist, but live provider login, production deployment hardening,
and UI onboarding remain deferred.

`changes scan` compares the current included regular files against the latest persisted snapshot
for the project root. Created, modified, and deleted files become pending local operations in
SQLite. Generated dependencies, policy exclusions, symlinks, and unsupported filesystem nodes are
summarized but are not persisted as uploadable operations.
