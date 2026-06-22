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
[docs/devbox/loom-and-devbox.md](docs/devbox/loom-and-devbox.md).

This repository currently contains the product foundation and alpha planning artifacts:

- [.product](.product/README.md) - product strategy, market sizing, KPIs, architecture, roadmap, and sources.
- [.plans](.plans/README.md) - MVP execution plan with static HTML pages for phases, architecture, and validation.
- [docs/alpha-cli-distribution.md](docs/alpha-cli-distribution.md) - GitHub Release packaging, server-owned storage setup, and two-device alpha smoke testing.
- [docs/devbox/loom-and-devbox.md](docs/devbox/loom-and-devbox.md) - the product/engine split and the vocabulary to use in new work.
- [docs/devbox/workspace-adapters-alpha.md](docs/devbox/workspace-adapters-alpha.md) - current sparse folder, agent workspace, materialized fallback, and filesystem adapter alpha state.
- [docs/architecture/loom-storage-consistency.md](docs/architecture/loom-storage-consistency.md) - current storage consistency guarantees, non-guarantees, and evidence path.
- [docs/evidence/alpha-readiness.md](docs/evidence/alpha-readiness.md) - concise alpha evidence and the canonical smoke commands.

## Current Code Shape

The workspace now has first-class Loom and Devbox platform boundaries:

- `loom/crates/loom-core` owns canonical Loom vocabulary: objects, file versions, folder revisions,
  checkpoints, pins, cursors, shared folders, and folder scopes.
- `loom/crates/loom-cli` provides the `loom` binary and the MVP command shape: `track`, `status`,
  `history`, `diff`, `checkpoint`, `restore`, `sync`, and `clone`.
- `loom/crates/loom-store`, `loom/crates/loom-worktree`, `loom/crates/loom-pack`,
  `loom/crates/loom-sync`, `loom/crates/loom-daemon`, and `loom/crates/loom-git` are compileable
  boundaries for follow-up
  engine work.
- `devbox/crates/devbox-platform` is the hosted/product boundary for accounts, machines, shared-folder
  membership, and hosted discovery.
- `devbox/crates/devbox-api` is the hosted API skeleton for auth, devices, shared folders, Loom
  remote API facade, and object-access routes.
- `devbox/crates/devbox-remote` owns the Devbox-hosted implementation of Loom's remote trait.
- `devbox/crates/devbox-cli` is the `devbox` product CLI. It exposes `login`, `share`, `clone`,
  `status`, `pause`, `resume`, and `unlink` for shared folders while keeping the existing alpha
  commands available for compatibility.
- `loom/` and `devbox/` are the active top-level homes. Their manifests map current crate ownership.

The older `devbox-*` alpha crates still compile and intentionally keep their historical
`project`/`snapshot` naming where changing it would create churn. Legacy alpha architecture docs
are marked as compatibility-era notes. Future PRs should migrate engine responsibilities into Loom
crates without silently deleting the alpha behavior.

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
  scripts, docs, a user CLI env template, and an operator env template. A deterministic two-device smoke harness proves
  receiver-generated pairing, pending-receiver fail-closed behavior, live publish, latest remote
  discovery, and receiver materialization with redacted evidence logs. The private-alpha desktop
  shell now provides an Electron control surface for local DB/cache/folder paths, hosted
  API/session/folder config, R2/shared-bucket prefix state, pairing, live sync command state,
  conflicts, devices, secret policy, and settings. It reads redacted `DEVBOX_*` setup state and
  does not start sync or mutate files directly. The default Railway deploy now builds the MVP
  `devbox-api` product service for `devbox login/share/clone`; product API metadata uses Railway
  Postgres and Loom pack bytes use server-owned R2, while the legacy hosted metadata server remains
  available for compatibility/operator smoke paths. OAuth, live
  Cloudflare/AWS credential provisioning, signed installers, multi-region/observability hardening,
  automatic conflict resolution, and paid/team/agent/Git-replacement work remain deferred.

## Local MVP Surface

Language note: many current commands still use `project` because the first alpha schema used that
word for a scoped shared folder. New product language should say shared folder.

### Quickstart: Prove the MVP Locally

Run the storage-v2 smoke harness first. It builds the local binaries if needed, starts a temporary
`devbox-api`, simulates two machines on one computer, and writes redacted evidence logs:

```text
scripts/mvp-two-device-smoke
```

The smoke proves the current MVP path end to end:

- Loom local-only capture/checkpoint/status.
- Loom local filesystem remote sync, eager clone, sparse clone, hydrate, evict, pin, and cache status.
- Devbox hosted `login`, eager `share`, eager `clone`, source edit, and target sync through local `devbox-api`.
- Hosted metadata/object split, including object hash mismatch rejection.
- Git metadata protection, generated dependency suppression, plain folders, nested folders, conflict refusal, and secret blocking.

Run this before trusting a local alpha change. It is the canonical proof path for the current
Devbox/Loom MVP.

The product CLI path is intentionally small:

```text
devbox login
devbox share <folder>
devbox clone
devbox clone <name> [target]
devbox clone <name> [target] --sparse
devbox warm <path>
devbox hydrate <path>
devbox keep <path>
devbox free-space <path>
devbox status
devbox doctor
devbox pause|resume|unlink [name]
```

Packaged builds can bake in the Devbox API endpoint. Local/dev builds default to
`http://127.0.0.1:8787`, and operators can override that with `devbox login --api <URL>` or
`DEVBOX_API_URL=<URL>`.

To remove the generated workspace after a passing run:

```text
DEVBOX_CLEAN_SMOKE_DIR=true scripts/mvp-two-device-smoke
```

The script starts `devbox-api` with `DEVBOX_API_METADATA_MODE=memory`, so local evidence does not
require Postgres or R2. It prints the evidence directory and writes `SUMMARY.txt` plus per-step logs
with session tokens and Devbox clone URLs redacted.

The product CLI keeps the normal path centered on folders and machines. For local development,
run `devbox-api` and point the CLI at it with `--api` or `DEVBOX_API_URL`:

```text
mkdir source
printf 'hello from this machine\n' > source/README.md

DEVBOX_API_METADATA_MODE=memory devbox-api --root .devbox-api --bind 127.0.0.1:3030
devbox login --api http://127.0.0.1:3030 --account local-dev --device-name "Desktop"
devbox share ./source --no-background-sync

DEVBOX_CONFIG_DIR=.devbox-laptop \
  devbox login --api http://127.0.0.1:3030 --account local-dev --device-name "Laptop"
DEVBOX_CONFIG_DIR=.devbox-laptop \
  devbox clone source ./target --no-background-sync

printf 'hello from the edited source\n' > source/README.md
devbox resume ./source --no-background-sync
DEVBOX_CONFIG_DIR=.devbox-laptop \
  devbox sync run-loop ./target --max-cycles 1
devbox status
devbox pause ./target
devbox resume ./target
devbox unlink ./target
```

Short Loom engine try path:

```text
mkdir source
printf 'hello\n' > source/README.md
loom track ./source
loom remote add local ./remote ./source
loom sync ./source
loom clone ./remote ./sparse-target --sparse
loom cache warm ./sparse-target
loom doctor ./sparse-target
```

`devbox share <folder>` registers the shared folder, configures the hidden Loom sync endpoint, syncs
the folder, and starts live sync. `devbox clone` lists folders available to this account; `devbox
clone <name> [target]` materializes a shared folder on this machine and starts live sync.
`devbox clone --sparse` links the folder first and lets `devbox warm`, `devbox hydrate`,
`devbox keep`, and `devbox free-space` control what stays local. Tokens, pack names, cursors,
remotes, and `devbox://` URLs are not printed in normal product output. Debug and engine-level
operations remain under `loom`. See [docs/devbox/sparse-folders.md](docs/devbox/sparse-folders.md)
for the hydrate versus keep distinction and current CLI-only limitations.

The current CLI can create/list/show/restore local snapshots and scan pending local changes:

- `devbox snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes scan --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes list --db <DB_PATH> [--project <PROJECT_ID>]`
- `devbox metadata check --endpoint <URL> [--auth-mode mock-dev-headers|account-session]`
- `devbox metadata alpha-invite create (--db <METADATA_DB>|--postgres-url-env <ENV>) --email <EMAIL>|--domain <DOMAIN>`
- `devbox metadata object-access resolve --api <URL> --session-token-env DEVBOX_SESSION_TOKEN --project <PROJECT_ID> --lease devbox-managed`
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
- `devbox-daemon sync --remote-kind hosted --object-access-api <URL> --object-access-lease devbox-managed --metadata-mode hosted-api --metadata-api <URL> --metadata-project <PROJECT_ID> [--metadata-session-token-env DEVBOX_SESSION_TOKEN] ...`

Hosted object storage is owned by the API. Deploy the API with `DEVBOX_R2_ENDPOINT`,
`DEVBOX_R2_BUCKET`, and server-side R2 credentials; users should not configure buckets, prefixes, or
object leases on their machines. The `credential-lease` commands still exist for low-level
admin/debug smoke tests, but they are not the product path.

Alpha helper scripts:

- `scripts/alpha-workspace-adapters-smoke.ps1` runs the workspace adapter alpha proof for sparse folders, agent virtual sessions, materialized sandbox fallback, and filesystem adapter truthfulness.
- `scripts/alpha-two-device-smoke.sh` runs a local two-device proof with pairing, pending receiver refusal, live publish, latest pull, materialization, and redacted evidence logs.
- `scripts/devbox-live-sync-alpha.sh` maps `.env` values into a live daemon command for local, hosted object-transfer, or trusted direct-S3 remotes.
- `scripts/package-cli.sh <VERSION>` builds macOS/Linux alpha tool archives with Loom, Devbox CLI, daemon, metadata server, docs, separate user/operator env templates, and helper scripts.
- `scripts/package-cli.ps1 -Version <VERSION>` builds the Windows alpha zip from a Windows machine.
- `scripts/install-devbox.sh` and `scripts/install-devbox.ps1` install or update the latest release globally for the current user.
- `scripts/package-desktop-alpha.sh <VERSION>` builds an unsigned Electron alpha control-surface bundle for macOS/Linux.

Hosted metadata sync wiring is explicit opt-in. Local deterministic smoke tests can use the
in-process SQLite store:

- `devbox sync publish-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>`
- `devbox sync import-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --metadata-account <ACCOUNT_ID>`
- `devbox sync materialize ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --metadata-account <ACCOUNT_ID>`
- `devbox-daemon sync --pull --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-account <ACCOUNT_ID> --metadata-project <PROJECT_ID> --to <TARGET_DIR> --apply ...`

External hosted alpha testers should use the product CLI: `devbox login`, `devbox share`, and
`devbox clone`. The lower-level `devbox sync ... --metadata-mode hosted-api` and
`devbox-daemon sync ...` commands remain for compatibility and smoke testing, not for normal users.

For local/mock import/materialize, the mock-dev metadata account scope is either passed explicitly with
`--metadata-account <ACCOUNT_ID>` or derived from `--mock-key-source-db <PUBLISHER_DB>` for the
legacy local/mock trust bootstrap. New paired receiver flows should run `devices join`,
`devices approve-join`, and `devices complete` first; after completion the receiver can decrypt with
its own local key state and does not need `--mock-key-source-db`. Invite-based hosted alpha login,
session-auth hosted metadata request handling, and server-mediated object access now exist. The API
derives the shared-bucket object scope from the authenticated account and folder; raw R2/S3
credentials are not returned to tester clients. Live daemon sync with `--remote-kind hosted`
transfers encrypted object bytes and resolves metadata through the metadata API using only the
tester's session token on the client; hosted API mode rejects tester-supplied `--metadata-account`
and uses the authenticated server session account. Trusted operators can still use
`--remote-kind s3` for direct S3/R2 smoke tests with local bucket env keys.
Railway deployment is wired to the MVP `devbox-api` product service by default. The hosted metadata
backend remains available for compatibility/operator smoke paths; OAuth, UI onboarding, and
multi-region/observability hardening remain deferred.

`changes scan` compares the current included regular files against the latest persisted snapshot
for the shared folder root. Created, modified, and deleted files become pending local operations in
SQLite. Generated dependencies, policy exclusions, symlinks, and unsupported filesystem nodes are
summarized but are not persisted as uploadable operations.
