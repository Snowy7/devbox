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

Devbox configures and hosts Loom. It does not decide folder-state semantics, file-version capture,
folder revision shape, checkpoint behavior, pack format, or remote reconciliation.

See [manifest.toml](manifest.toml) for the current crate map.
