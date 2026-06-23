# Full-Scale Project Shape

This is the target shape if we rebuild Bindhub around the clean Lo../bindhub split.

## Core Split

```text
Loom   = engine
Bindhub = hosted platform and product
```

Loom decides what folder state is and how it syncs.

Bindhub decides who owns it, where it is hosted, and who can access it.

## Repository Shape

The Rust workspace is physically split by ownership:

```text
loom/
  crates/
    loom-core
    loom-store
    loom-worktree
    loom-pack
    loom-sync
    loom-daemon
    loom-cli
    loom-git
bindhub/
  crates/
    bindhub-auth
    bindhub-platform
    bindhub-remote
    bindhub-api
    bindhub-cli
```

Legacy alpha compatibility crates also live under `bindhub/crates/` until their responsibilities are
absorbed or retired. The full repository shape is:

```text
bindhub/
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

  bindhub/
    crates/
      bindhub-auth/
      bindhub-platform/
      bindhub-api/
      bindhub-remote/
      bindhub-cli/

  apps/
    desktop/
    web/

  docs/
    loom/
    bindhub/
    architecture/

  tests/
    loom-fixtures/
    Bindhub-e2e/

  infra/
    local/
    hosted/
```

## Loom

Loom is the full local engine and sync system. It should be useful with no Bindhub account, no hosted
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
loom diff
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

## Bindhub

Bindhub is the hosted platform for Loom.

All Bindhub backend and CLI crates should be Rust.

```text
bindhub-auth      Rust
```

Accounts, sessions, device identity, tokens.

```text
bindhub-platform  Rust
```

Shared folder registry, permissions, device membership, folder discovery.

```text
bindhub-remote    Rust
```

Hosted implementation of the Loom remote protocol.

```text
bindhub-api       Rust
```

HTTP service for auth, platform APIs, hosted storage, object access, metadata.

```text
bindhub-cli       Rust
```

Product CLI:

```text
bindhub login
bindhub share <folder>
bindhub clone [name]
bindhub manage <name>
bindhub pause <name>
bindhub resume <name>
bindhub unlink <name>
```

Bindhub CLI should mostly authenticate, discover hosted folders, configure Loom, and then let Loom do
the actual tracking and syncing.

## Apps

```text
apps/desktop    TypeScript + React + Electron
```

Local product shell. Talks to Loom daemon and Bindhub API. It must not mutate shared folders
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

The MVP should prove Loom and Bindhub separately.

```text
Loom proof:
  use Loom offline as local source control for a folder
  create file versions and folder revisions
  checkpoint meaningful states
  restore previous states
  sync to a simple Loom remote

Bindhub proof:
  login
  share a folder
  clone it on another machine
  let Loom keep it synced through Bindhub hosted storage
```

## Language Summary

```text
Loom engine: Rust
Loom CLI/daemon: Rust
Bindhub backend/platform: Rust
Bindhub CLI: Rust
Desktop app: TypeScript + React + Electron
Web app later: TypeScript + React
Docs: Markdown/MDX
Infra: Docker/YAML/scripts
```
