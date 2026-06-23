# Workspace Adapter Alpha

This page is the current-state map for the bindhub/Loom workspace adapter arc. Bindhub is the product
for folder continuity across machines. Loom is the engine boundary for objects, file versions, folder
revisions, cache entries, pins, checkpoints, cursors, overlays, and sandboxes.

The alpha adapter work proves explicit sparse folder flows, agent virtual sessions, materialized
sandbox fallback, and filesystem adapter boundaries. It does not claim native OS placeholder files
or hydrate-on-open behavior.

## Evidence Command

Run the adapter smoke locally:

```text
powershell -ExecutionPolicy Bypass -File scripts/alpha-workspace-adapters-smoke.ps1
```

The script builds local binaries if needed, starts a temporary in-memory `bindhub-api`, uses only
temporary folders, and writes redacted logs under the printed evidence directory.

It proves:

- human sparse folder flow through `bindhub login`, `bindhub share`, `bindhub clone --sparse`,
  `bindhub status`, `bindhub hydrate`, `bindhub warm`, `bindhub keep`, and `bindhub free-space`
- agent virtual workspace flow through `loom workspace open/read/write/exec/diff/checkpoint/discard`
- materialized sandbox fallback for real commands, including safe capture and unsafe host mutation
  refusal
- filesystem adapter alpha behavior, including native fail-closed reporting and `local-dev`
  metadata-only mount/status/unmount simulation

## Sparse Folder Quickstart

Use `Bindhub` for the human product flow:

```text
bindhub login --api <BINDHUB_API_URL> --account <ACCOUNT> --device-name "Desktop"
bindhub share <folder> --no-background-sync

BINDHUB_CONFIG_DIR=.bindhub-laptop \
  bindhub login --api <BINDHUB_API_URL> --account <ACCOUNT> --device-name "Laptop"
BINDHUB_CONFIG_DIR=.bindhub-laptop \
  bindhub clone <shared-folder-name> <target-folder> --sparse --no-background-sync

BINDHUB_CONFIG_DIR=.bindhub-laptop bindhub status <target-folder>
BINDHUB_CONFIG_DIR=.bindhub-laptop bindhub hydrate <target-folder>/README.md
BINDHUB_CONFIG_DIR=.bindhub-laptop bindhub warm <target-folder> --max-bytes 4096
BINDHUB_CONFIG_DIR=.bindhub-laptop bindhub keep <target-folder>/README.md
BINDHUB_CONFIG_DIR=.bindhub-laptop bindhub free-space <target-folder> --max-bytes 0
```

`hydrate` downloads and materializes a requested path now. `warm` chooses useful small source,
manifest, and config files. `keep` records offline retention intent; it does not download missing
bytes by itself. `free-space` only removes clean, unpinned local bytes when Bindhub can prove a
hosted copy exists.

Sparse folders are explicit CLI workflows today. Cloud-only files do not appear as placeholder files
in Explorer, Finder, or shells.

## Agent Workspace Workflow

Agents use Loom workspace sessions over a folder revision:

```text
loom track <folder>
loom workspace open <folder> --session agent-1
loom workspace list <folder> --session agent-1
loom workspace read <folder> --session agent-1 README.md
loom workspace exec <folder> --session agent-1 -- cat README.md
loom workspace write <folder> --session agent-1 src/change.txt --text "agent edit"
loom workspace diff <folder> --session agent-1
loom workspace checkpoint <folder> --session agent-1 -m "agent checkpoint"
```

The virtual adapter reads base revision metadata and overlay files without exposing the full source
folder as a mutable host worktree. Writes land in the session overlay until checkpoint. Checkpoint
coalesces overlay file versions into a folder revision and creates a human-readable checkpoint. A
session that should be abandoned uses:

```text
loom workspace discard <folder> --session agent-1
```

## Virtual Versus Materialized Execution

`loom workspace exec` is a virtual command surface. It supports deterministic commands such as
`pwd`, `ls`, `cat`, `stat`, `rg`, and `write`. Unsupported commands return a clear failure and tell
the caller to use materialized sandbox fallback.

Use materialized fallback when a real shell command or tool needs a filesystem:

```text
loom workspace materialize-run <folder> --session agent-1 -- <COMMAND> [ARGS...]
```

Materialized fallback creates an isolated sandbox from the session view, runs the command there, and
captures safe changes back into the overlay. It refuses deleted tracked files, secret-looking files,
generated/dependency paths, and mutations to the real shared folder outside the sandbox capture.

This fallback is for commands that need real files; it is not permission to mutate the host folder
directly.

## Filesystem Adapter Alpha

The filesystem adapter boundary is exposed through:

```text
loom fs status <folder>
loom fs mount <folder> --mount <path>
loom fs mount <folder> --adapter local-dev --mount <path>
loom fs status <folder> --adapter local-dev --mount <path>
loom fs unmount <folder> --adapter local-dev --mount <path>
```

Native adapters for Windows, macOS, and Linux are alpha stubs. They report host direction and fail
closed for mount. They do not record successful native mount metadata, do not create placeholder
files, and do not support hydrate-on-open.

`--adapter local-dev` is a deterministic metadata simulation for tests and wiring. It records mount
state under `.loom/fs`, reports status, and supports idempotent unmount. It does not create the
mount path, project files into the OS, or hydrate bytes on open.

## Safety Guarantees

The current alpha keeps these guarantees:

- Loom captures file versions frequently and coalesces folder revisions at stable boundaries such as
  commands, sync, restore, sandbox merge, and checkpoint.
- Cache metadata records object byte availability separately from file versions and folder
  revisions.
- Sparse cleanup keeps dirty files, pinned files, unsupported local entries, and files without
  hosted proof.
- Agent overlay writes and materialized captures re-check secret/generated/dependency policy before
  object bytes enter the cache.
- Materialized fallback refuses host shared-folder mutation outside the sandbox.
- Native filesystem adapters fail closed until real OS integrations exist.

## Non-Goals

These are not implemented in the current alpha:

- native Windows Cloud Files, Projected File System, macOS File Provider, macFUSE, or Linux FUSE
  drivers
- placeholder files in normal OS file browsers or shells
- hydrate-on-open, sparse reads, or kernel callback hydration
- remote protocol v2, chunk transfer, compression, or lazy byte-range transport
- broad automatic conflict resolution
- weakening secret, generated dependency, or unsupported filesystem policies
