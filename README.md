# Devbox

Devbox is a developer-native workspace continuity project: your code folder, work-in-progress, and project context should follow you across machines.

The first product wedge is simple:

> Close desktop. Open laptop. Keep coding.

This repository currently contains the product foundation and MVP planning artifacts:

- [.product](.product/README.md) - product strategy, market sizing, KPIs, architecture, roadmap, and sources.
- [.plans](.plans/README.md) - MVP execution plan with static HTML pages for phases, architecture, and validation.

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
  compare-and-set without network services. Production sign-in, managed credentials, deployment
  hardening, Electron UI, automatic conflict resolution, and conflict UI remain later Phase 1 work.

## Local MVP Surface

The current CLI can create/list/show/restore local snapshots and scan pending local changes:

- `devbox snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes scan --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes list --db <DB_PATH> [--project <PROJECT_ID>]`
- `devbox metadata check --endpoint <URL> [--auth-mode mock-dev-headers]`

Hosted metadata sync wiring is explicit opt-in for dev/test flows:

- `devbox sync publish-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>`
- `devbox sync import-snapshot ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --mock-key-source-db <PUBLISHER_DB>`
- `devbox sync materialize ... --metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB> --metadata-project <PROJECT_ID> --mock-key-source-db <PUBLISHER_DB>`

For import/materialize, the hosted metadata account scope is either passed explicitly with
`--metadata-account <ACCOUNT_ID>` or derived from `--mock-key-source-db <PUBLISHER_DB>` for the
local/mock trust bootstrap. Production account proof remains deferred.

`changes scan` compares the current included regular files against the latest persisted snapshot
for the project root. Created, modified, and deleted files become pending local operations in
SQLite. Generated dependencies, policy exclusions, symlinks, and unsupported filesystem nodes are
summarized but are not persisted as uploadable operations.
