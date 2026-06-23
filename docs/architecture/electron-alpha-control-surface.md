# Electron Alpha Control Surface

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Bindhub.

This Phase 1 slice turns `apps/desktop` from a placeholder into a private-alpha Electron shell.

## Boundary

The desktop app is a local-first control surface. It can run without cloud credentials, browser
login, Docker, Postgres, production hosted metadata, or external network services.

The app does not directly mutate workspace files. Renderer code reads alpha state through the
Electron preload bridge. The main process derives redacted state from `BINDHUB_*` environment
variables and never reads raw Cloudflare/R2/API/session values except to report whether a named env
var is present. Future workspace mutations must go through the Rust daemon or an explicit local
bridge command boundary.

## Surface

The first screen is the actual control surface, not a landing page. It includes:

- status and tray affordance
- watched projects
- sync activity
- local DB/cache/project/receiver paths
- hosted metadata API, session env, and shared-folder scope
- server-owned object storage status
- generated live sync, pairing, release, and smoke-test commands
- manual conflict records
- devices and pairing state
- explicit secret safety policy records
- redacted remote/provider settings
- CLI command hints for local alpha workflows

The env-backed bridge is still command-state only: it does not start the daemon, publish snapshots,
or materialize files. That keeps the private alpha honest until live daemon IPC is wired. Placeholder
state remains available for builds and screenshots without credentials. The fixture contains
redacted identifiers and opaque references only; no raw secret, key, recovery, token, or cloud
credential material is present.

## Validation

Desktop validation is headless:

```text
pnpm typecheck
pnpm test:safety
pnpm build
```

`test:safety` scans desktop fixture and renderer source for common raw secret/token shapes.

The unsigned desktop alpha bundle can be produced with:

```text
scripts/package-desktop-alpha.sh v0.1.0-alpha.1
```

## Deferred

Deferred work includes live daemon IPC, production OAuth UI, live provider provisioning, signed
installers, paid/team flows, agent workflows, Loom UI, automatic conflict resolution, and
production packaging.
