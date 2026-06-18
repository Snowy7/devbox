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
  for R2/S3/MinIO-compatible encrypted blobs, while hosted metadata and managed credentials remain
  later Phase 1 work

## Local MVP Surface

The current CLI can create/list/show/restore local snapshots and scan pending local changes:

- `devbox snapshot --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes scan --db <DB_PATH> --cache <CACHE_ROOT> <PROJECT_ROOT>`
- `devbox changes list --db <DB_PATH> [--project <PROJECT_ID>]`

`changes scan` compares the current included regular files against the latest persisted snapshot
for the project root. Created, modified, and deleted files become pending local operations in
SQLite. Generated dependencies, policy exclusions, symlinks, and unsupported filesystem nodes are
summarized but are not persisted as uploadable operations.
