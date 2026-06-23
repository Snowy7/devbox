# Devbox Area

Devbox is the product and hosted platform.

This directory is the home for Devbox-owned work. New product, platform, hosted API, and
Devbox-specific remote code should land under [`crates/`](crates/).

Devbox owns:

- accounts and sessions
- machines and device membership
- shared-folder discovery and permissions
- hosted API surface
- object-access brokering and platform metadata
- the Devbox-hosted implementation of Loom's remote protocol
- product CLI commands such as `login`, `share`, `clone`, `pause`, `resume`, and `unlink`

`devbox login` uses browser-based machine auth by default. The CLI starts a
short-lived device flow with `devbox-api`, opens the web app's `/auth/cli`
route, and stores the Devbox session returned by API polling after WorkOS/AuthKit
has verified the browser session. Hosted browser auth must derive account
identity from that verified session; use `devbox login --local-dev-direct` only
for deterministic local alpha setup against a local dev API. The direct helper
requires `DEVBOX_API_ENABLE_LOCAL_DEV_SESSION=1` on `devbox-api`, and the CLI
refuses non-loopback API URLs unless `DEVBOX_ALLOW_NON_LOOPBACK_LOCAL_DEV_LOGIN=1`
is set for local development.

Devbox configures and hosts Loom. It does not decide folder-state semantics, file-version capture,
folder revision shape, checkpoint behavior, pack format, or remote reconciliation.

See [manifest.toml](manifest.toml) for the current crate map.
