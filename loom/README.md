# Loom Area

Loom is the source-control and sync engine underneath Devbox.

This directory is the intended home for Loom-owned work. PR 1 keeps Rust crates in the existing
root `crates/` workspace to avoid breaking alpha packaging, but the ownership boundary is explicit:
new engine work should land in Loom crates and move here once the repository is ready for the
physical reshuffle.

Loom owns:

- objects and object identity
- file versions
- folder revisions
- checkpoints and pins
- cursors
- shared-folder worktree capture and restore
- pack format and remote sync semantics
- Git compatibility analysis

Loom does not own hosted Devbox accounts, billing, product onboarding, or device membership.

See [manifest.toml](manifest.toml) for the current crate map.
