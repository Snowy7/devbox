# Full-Scale Project Shape

This is the target shape if we rebuild Devbox around the clean Loom/Devbox split.

## Core Split

```text
Loom   = engine
Devbox = hosted platform and product
```

Loom decides what folder state is and how it syncs.

Devbox decides who owns it, where it is hosted, and who can access it.

## Repository Shape

```text
devbox/
  loom/
    crates/
      loom-core/
      loom-store/
      loom-worktree/
      loom-pack/
      loom-sync/
      loom-daemon/
      loom-cli/
      loom-git/

  devbox/
    crates/
      devbox-auth/
      devbox-platform/
      devbox-remote/
      devbox-api/
      devbox-cli/

  apps/
    desktop/
    web/

  docs/
    loom/
    devbox/
    architecture/

  tests/
    loom-fixtures/
    devbox-e2e/

  infra/
    local/
    hosted/
```

## Loom

Loom is the full local engine and sync system. It should be useful with no Devbox account, no hosted
remote, and no network.

All Loom crates should be Rust.

```text
loom-core       Rust
```

Vocabulary and pure domain model: objects, file versions, folder revisions, checkpoints, pins,
cursors, conflicts.

```text
loom-store      Rust
```

Local database, object cache, metadata persistence, retention.

```text
loom-worktree   Rust
```

Folder scanning, path safety, materialization, restore, generated-file policy, secret boundaries.

```text
loom-pack       Rust
```

Pack format, compression, deduplication, encryption envelope, import/export.

```text
loom-sync       Rust
```

Remote protocol, reconciliation, upload/download, cursor compare, clone/sync logic.

```text
loom-daemon     Rust
```

Watch folders, capture file versions, coalesce folder revisions, run background sync.

```text
loom-cli        Rust
```

Human CLI:

```text
loom track
loom status
loom history
loom checkpoint
loom restore
loom sync
loom clone
loom sandbox
```

```text
loom-git        Rust
```

Git compatibility analyzer: detect Git, protect `.git`, preserve normal Git workflows.

## Devbox

Devbox is the hosted platform for Loom.

All Devbox backend and CLI crates should be Rust.

```text
devbox-auth      Rust
```

Accounts, sessions, device identity, tokens.

```text
devbox-platform  Rust
```

Shared folder registry, permissions, device membership, folder discovery.

```text
devbox-remote    Rust
```

Hosted implementation of the Loom remote protocol.

```text
devbox-api       Rust
```

HTTP service for auth, platform APIs, hosted storage, object access, metadata.

```text
devbox-cli       Rust
```

Product CLI:

```text
devbox login
devbox share <folder>
devbox clone [name]
devbox manage <name>
devbox pause <name>
devbox resume <name>
devbox unlink <name>
```

Devbox CLI should mostly authenticate, discover hosted folders, configure Loom, and then let Loom do
the actual tracking and syncing.

## Apps

```text
apps/desktop    TypeScript + React + Electron
```

Local product shell. Talks to Loom daemon and Devbox API. It must not mutate shared folders
directly.

```text
apps/web        TypeScript + React
```

Later web dashboard for accounts, devices, shared folders, billing, and teams.

## Docs And Infra

```text
docs            Markdown / MDX
infra/local     Docker / scripts
infra/hosted    deployment configs
```

## MVP Proofs

The MVP should prove Loom and Devbox separately.

```text
Loom proof:
  use Loom offline as local source control for a folder
  create file versions and folder revisions
  checkpoint meaningful states
  restore previous states
  sync to a simple Loom remote

Devbox proof:
  login
  share a folder
  clone it on another machine
  let Loom keep it synced through Devbox hosted storage
```

## Language Summary

```text
Loom engine: Rust
Loom CLI/daemon: Rust
Devbox backend/platform: Rust
Devbox CLI: Rust
Desktop app: TypeScript + React + Electron
Web app later: TypeScript + React
Docs: Markdown/MDX
Infra: Docker/YAML/scripts
```

