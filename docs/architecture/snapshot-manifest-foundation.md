# Snapshot Manifest Foundation

This slice introduces the Phase 0 snapshot manifest builder in `crates/devbox-snapshot`.

The builder walks a local project directory, applies the existing generated-artifact policy from `devbox-core`, writes included file bytes into the local `BlobCache`, and returns an in-memory draft snapshot manifest. It does not persist snapshot rows, blob rows, or manifest rows to SQLite yet.

## Boundary

`devbox-core` owns domain vocabulary:

- `BlobId`
- `SnapshotId`
- `ManifestEntryKind`
- `PolicyDecision`
- generated-artifact policy evaluation

`devbox-store` owns local storage primitives:

- SQLite metadata schema and migrations
- the content-addressed `BlobCache`
- BLAKE3 object identity for file bytes
- cache object references such as `blobs/b3/aa/bb/<digest>`

`devbox-snapshot` owns manifest construction:

- deterministic filesystem traversal
- included file writes to `BlobCache`
- one manifest entry per included file, included directory, symlink requiring a later decision, or excluded policy boundary
- explicit deferral for unsupported filesystem node types
- stable draft snapshot identity derived from canonical manifest entry fields
- aggregate summary counts for dry-run and future operation records

The CLI exposes the current surface as:

```text
devbox snapshot --cache <CACHE_ROOT> --dry-run <PATH>
```

This command validates that the cache root is outside the snapshot root before initializing the cache, then creates local blob-cache objects for included files and prints the draft manifest summary. It intentionally does not write SQLite metadata.

## Manifest Entries

Each draft manifest entry records:

- relative path
- entry kind: file, directory, symlink, or unsupported
- size for file-like entries when available
- blob id for included files
- blob object reference for included files
- policy decision, including exclusion reason or user-decision reason when applicable

Directory entries are sorted by filesystem name before recursion so repeated runs over the same tree produce the same entry order. Generated or tool-owned directories are recorded as excluded entries and are not descended into.

Current directory exclusions come from the scanner policy and include `.git`, `node_modules`, `target`, `.venv`, build outputs, language caches, and tool caches. These exclusions apply to directories before descent, not to ordinary regular files that happen to share names such as `build` or `dist`.

The blob cache root is Devbox-owned state. If the cache root is inside the snapshot root, the dry-run CLI rejects it before `BlobCache::open` can create directories. The builder keeps the same validation as defense in depth rather than trying to snapshot or explain its own object cache.

Only regular files are written to `BlobCache`. Symlinks and unsupported filesystem node types are represented as requiring a future user or safety decision so restore semantics can be designed deliberately.

Phase 0 canonical manifest identity converts relative paths to slash-separated UTF-8 strings using lossy path display. Non-Unicode path identity is intentionally deferred until the manifest format is ready to define byte-preserving path encoding across platforms.

## Relationship To SQLite

The SQLite schema already has tables for snapshots, manifests, blobs, policies, and policy evaluations. This slice stops before inserting into those tables.

Later persistence work should translate a draft snapshot into:

- one `snapshots` row
- one `manifest_entries` row per draft entry
- `blobs` metadata rows for cache objects that are referenced by included file entries
- `policy_evaluations` rows for excluded or deferred paths when useful

The local cache remains the source of file bytes. SQLite should store references and metadata, not project file bytes.

## Deferred

This slice intentionally does not implement:

- restore or materialization
- snapshot persistence to SQLite
- daemon automation
- filesystem watching
- `.gitignore` parsing
- user, project, or team policy overrides
- symlink restore semantics
- byte-preserving non-Unicode path identity
- cloud sync
- encryption
- compression
- packfiles
- garbage collection
- conflict handling
- Electron UI
