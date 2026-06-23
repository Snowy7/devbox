# Bindhub Area

Bindhub is the product and hosted platform.

This directory is the home for Bindhub-owned work. New product, platform, hosted API, and
Bindhub-specific remote code should land under [`crates/`](crates/).

Bindhub owns:

- accounts and sessions
- machines and device membership
- shared-folder discovery and permissions
- hosted API surface
- object-access brokering and platform metadata
- the Bindhub-hosted implementation of Loom's remote protocol
- product CLI commands such as `login`, `share`, `clone`, `pause`, `resume`, and `unlink`

`bindhub login` uses browser-based machine auth by default. The CLI starts a
short-lived device flow with `bindhub-api`, opens the web app's `/auth/cli`
route, and stores the Bindhub session returned by API polling after WorkOS/AuthKit
has verified the browser session. Hosted browser auth must derive account
identity from that verified session; use `bindhub login --local-dev-direct` only
for deterministic local alpha setup against a local dev API. The direct helper
requires `BINDHUB_API_ENABLE_LOCAL_DEV_SESSION=1` on `bindhub-api`, and the CLI
refuses non-loopback API URLs unless `BINDHUB_ALLOW_NON_LOOPBACK_LOCAL_DEV_LOGIN=1`
is set for local development.

For local API development, copy `.env.example` to `.env.local` and start the API
with:

```sh
powershell -ExecutionPolicy Bypass -File scripts/bindhub-api-local.ps1
```

The web app's `BINDHUB_HOSTED_API_SERVICE_TOKEN` must match the API's
`BINDHUB_API_SERVICE_TOKEN` for CLI browser approval and WorkOS session exchange.

Bindhub configures and hosts Loom. It does not decide folder-state semantics, file-version capture,
folder revision shape, checkpoint behavior, pack format, or remote reconciliation.

See [manifest.toml](manifest.toml) for the current crate map.
