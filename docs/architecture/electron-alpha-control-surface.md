# Electron Alpha Control Surface

This Phase 1 slice turns `apps/desktop` from a placeholder into a private-alpha Electron shell.

## Boundary

The desktop app is a local-first control surface. It can run without cloud credentials, browser
login, Docker, Postgres, production hosted metadata, or external network services.

The app does not directly mutate workspace files. Renderer code reads alpha state through the
Electron preload bridge. Future workspace mutations must go through the Rust daemon or an explicit
local bridge command boundary.

## Surface

The first screen is the actual control surface, not a landing page. It includes:

- status and tray affordance
- watched projects
- sync activity
- manual conflict records
- devices and pairing state
- explicit secret safety policy records
- redacted remote/provider settings
- CLI command hints for local alpha workflows

The fixture-backed bridge is intentional for this PR. It keeps the UI buildable and reviewable while
the daemon API remains a later integration point. The fixture contains redacted identifiers and
opaque references only; no raw secret, key, recovery, token, or cloud credential material is present.

## Validation

Desktop validation is headless:

```text
npm run typecheck
npm run test:safety
npm run build
```

`test:safety` scans desktop fixture and renderer source for common raw secret/token shapes.

## Deferred

Deferred work includes live daemon IPC, production OAuth UI, live provider provisioning, paid/team
flows, agent workflows, Git replacement UI, automatic conflict resolution, and production packaging.
