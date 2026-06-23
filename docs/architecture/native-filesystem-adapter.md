# Native Filesystem Adapter Alpha

Bindhub wants a shared folder to feel continuous across machines. Native filesystem adapters are one
way to make sparse folders feel ordinary later, but they are not Loom core. Loom owns objects, cache
entries, file versions, folder revisions, checkpoints, pins, cursors, sandboxes, and overlays. A
filesystem adapter projects one selected folder revision through an operating-system surface.

The alpha boundary lives in `loom-fs` and is exposed through `loom fs ...` first. Bindhub can wrap
this with product wording later, but the underlying model stays folder/revision/cache centered.

## CLI Surface

- `loom fs mount [FOLDER] --mount <PATH> [--adapter native|local-dev|windows|macos|linux]`
- `loom fs unmount [FOLDER] --mount <PATH> [--adapter native|local-dev|windows|macos|linux]`
- `loom fs status [FOLDER] [--mount <PATH>] [--adapter native|local-dev|windows|macos|linux]`
- `loom fs doctor [FOLDER] [--adapter native|local-dev|windows|macos|linux]`

`native` picks the current host direction. `local-dev` is the only adapter that can record a mount
today, and it is explicit simulation: it persists metadata under `.loom/fs`, does not create
placeholder files, and does not hydrate bytes on open.

## Support Matrix

| Adapter | Direction | Alpha behavior | Hydrate on open |
| --- | --- | --- | --- |
| `local-dev` | Metadata-only simulation | Can record, status, and unmount simulated mount state | No |
| `windows` / `windows-cloud-files` | Cloud Files or Projected File System | Capability detection only; mount fails closed | No |
| `macos` / `macos-file-provider` | File Provider, with FUSE as a possible fallback | Capability detection only; mount fails closed | No |
| `linux` / `linux-fuse` | FUSE | Capability detection only; mount fails closed | No |

Native adapters may detect host APIs such as Windows filter drivers, macOS File Provider/macFUSE, or
`/dev/fuse`, but detection is not mount support. They refuse `mount` until a real provider, callback
loop, and read/hydration path exist.

## Boundary Rules

- The adapter reads Loom store metadata and sparse-folder worktree policy; it does not define file
  versions or folder revisions.
- Mount safety checks reuse Loom capture and diff logic. A mount is refused if the working folder is
  secret-blocked, deferred, or differs from the latest folder revision.
- Mount paths inside the shared folder are refused to avoid recursive projections.
- Native adapters never record successful mount metadata while unsupported.
- `unmount` and `status` are idempotent and report persisted adapter state only.
- No adapter claims hydrate-on-open support in this PR.

## Current Limits

There is no production Windows Cloud Files provider, Projected FS provider, macOS File Provider
extension, macFUSE filesystem, or Linux FUSE process yet. There are also no placeholder files, no
kernel callback hydration, no sparse read repair, no compression/chunk transfer, and no OS virtual
filesystem packaging.

The next hardening PR should prove complete human and agent workflows across explicit hydrate/cache
commands, virtual workspace sessions, materialized sandboxes, and the filesystem adapter boundary.
