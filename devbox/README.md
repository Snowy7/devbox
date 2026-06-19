# Devbox Area

Devbox is the product and hosted platform.

This directory is the intended home for Devbox-owned work. PR 1 keeps Rust crates in the existing
root `crates/` workspace to avoid breaking alpha packaging, but product/platform ownership is now
explicit.

Devbox owns:

- accounts and sessions
- machines and device membership
- shared-folder discovery and permissions
- hosted API surface
- object-access brokering and platform metadata
- product CLI commands such as `login`, `share`, `clone`, `pause`, `resume`, and `unlink`

Devbox configures and hosts Loom. It does not decide folder-state semantics, file-version capture,
folder revision shape, checkpoint behavior, pack format, or remote reconciliation.

See [manifest.toml](manifest.toml) for the current crate map.
