# Sparse Folders

Devbox can keep a shared folder present on a machine before every file has been downloaded. The
folder metadata is local, while some file bytes can stay cloud-only until the user asks for them.

This is meant to feel like ordinary folder continuity:

- `devbox clone <folder> <target> --sparse` links the folder and leaves files available on demand.
- `devbox warm <path>` downloads useful small source, manifest, and config files under a path.
- `devbox hydrate <path>` downloads exactly the tracked file or folder scope requested.
- `devbox keep <path>` protects a path from cleanup for offline use.
- `devbox free-space <path>` removes only safe local file bytes under a path.
- `devbox status [path]` shows what is local, cloud-only, changed locally, kept, cleanable, or still
  pending upload.

## Hydrate Versus Keep

Hydrate downloads bytes and materializes files on this machine. It refuses to overwrite a dirty
local file, replace a symlink, or materialize generated/dependency paths that Devbox policy excludes.

Keep is a retention intent. It protects a path from `free-space` cleanup, but it does not download
missing bytes by itself. Use `devbox hydrate <path>` when the path must be available offline now,
then use `devbox keep <path>` when it should stay available.

## Free-Space Safety

`devbox free-space` is conservative:

- it never removes dirty local changes
- it never removes kept paths
- it never removes bytes unless the hosted shared folder proves it has a copy
- it treats unsupported local entries as not cleanable
- it leaves cloud-only untouched files as cloud-only, not deletions

Generated dependencies, ignored tool folders, and blocked secrets continue to follow the same capture
policy used by share, sync, and restore.

## Current Limits

Before native OS filesystem adapters land, sparse folders are explicit CLI workflows. Cloud-only
files do not appear as placeholder files in Explorer, Finder, or shells. A command that needs a file
must run `devbox hydrate <path>` or `devbox warm <path>` first.

Native Windows, macOS, and Linux filesystem adapters are the next layer. They should call the same
Loom hydrate, keep, status, and free-space primitives rather than invent separate cache behavior.
