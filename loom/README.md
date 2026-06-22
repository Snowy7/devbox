# Loom Area

Loom is the source-control and sync engine underneath Devbox.

This directory is the home for Loom-owned work. New engine code should land under
[`crates/`](crates/), and it should keep Devbox product/platform concerns out of the core engine.

Loom owns:

- objects and object identity
- file versions
- folder revisions
- checkpoints and pins
- cursors
- shared-folder worktree capture and restore
- workspace adapters and virtual agent sessions
- pack format and remote sync semantics
- Git compatibility analysis

Loom does not own hosted Devbox accounts, billing, product onboarding, or device membership.

See [manifest.toml](manifest.toml) for the current crate map.
