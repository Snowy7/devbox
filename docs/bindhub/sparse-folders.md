# Sparse Folders

Bindhub can keep a shared folder present on a machine before every file has been downloaded. The
folder metadata is local, while some file bytes can stay cloud-only until the user asks for them.

This is meant to feel like ordinary folder continuity:

- `bindhub clone <folder> <target> --sparse` links the folder and leaves files available on demand.
- `Bindhub warm <path>` downloads useful small source, manifest, and config files under a path.
- `Bindhub hydrate <path>` downloads exactly the tracked file or folder scope requested.
- `Bindhub keep <path>` protects a path from cleanup for offline use.
- `Bindhub free-space <path>` removes only safe local file bytes under a path.
- `bindhub status [path]` shows what is local, cloud-only, changed locally, kept, cleanable, or still
  pending upload.

## Hydrate Versus Keep

Hydrate downloads bytes and materializes files on this machine. It refuses to overwrite a dirty
local file, replace a symlink, or materialize generated/dependency paths that Bindhub policy excludes.

Keep is a retention intent. It protects a path from `free-space` cleanup, but it does not download
missing bytes by itself. Use `Bindhub hydrate <path>` when the path must be available offline now,
then use `Bindhub keep <path>` when it should stay available.

## Free-Space Safety

`Bindhub free-space` is conservative:

- it never removes dirty local changes
- it never removes kept paths
- it never removes bytes unless the hosted shared folder proves it has a copy
- it treats unsupported local entries as not cleanable
- it leaves cloud-only untouched files as cloud-only, not deletions

Generated dependencies, ignored tool folders, and blocked secrets continue to follow the same capture
policy used by share, sync, and restore.

## Current Limits

Native OS filesystem adapters now have a Loom alpha boundary at `loom fs ...`, but real Windows,
macOS, and Linux host integrations are not implemented yet. Sparse folders are still explicit CLI
workflows. Cloud-only files do not appear as placeholder files in Explorer, Finder, or shells. A
command that needs a file must run `Bindhub hydrate <path>` or `Bindhub warm <path>` first.

The `loom fs --adapter local-dev` path can simulate mount metadata for tests, while native adapters
only report capabilities and fail closed. Future Bindhub wrappers should call the same Loom hydrate,
keep, status, and free-space primitives rather than invent separate cache behavior.
