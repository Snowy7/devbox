# Loom Storage Consistency

This page documents the guarantees Devbox can claim today for Loom-backed folder continuity. It is
intended to be testable against local commands and the MVP smoke harness, not a promise about the
future sparse filesystem or remote protocol.

## Current Guarantees

### Folder Revisions And Checkpoints

- File versions are captured as content-addressed object references plus path metadata.
- Folder revisions are coherent trees assembled from file versions at stable boundaries: `loom track`,
  `loom status`, `loom sync`, `loom restore`, `devbox share`, `devbox resume`, and daemon sync cycles.
- A checkpoint labels an existing folder revision and creates a retention pin for that revision.
- Local history keeps working without a remote. The smoke proves this with `loom track`, `loom
  checkpoint`, and `loom status`.

### Remote Cursors

- A remote cursor is the remote's current pointer for a shared folder.
- Sync reads the remote cursor before upload and advances it with compare-and-set after the pack and
  required objects are stored.
- If the remote cursor changed or points to a revision that is not an ancestor of the local latest
  revision, sync refuses instead of overwriting remote state.
- Local-fs remotes use a lock file plus current-value recheck for cursor compare-and-set. Hosted
  Devbox remotes use server-side metadata compare-and-set and return conflict on stale expectations.

### Metadata/Object Split

- Separate-object remotes store Loom metadata packs separately from content objects.
- Metadata packs name object ids, sizes, compression, and availability. They do not need to carry
  object payload bytes when the remote supports separate object transfer.
- Eager clone imports metadata and hydrates all required object bytes before materializing files.
- Sparse clone imports metadata and records remote-only cache entries without writing source files.

### Remote-Only Bytes

- Remote-only means Loom has file-version and cache metadata for the object but no local object bytes.
- `loom hydrate <path>` fetches missing object bytes from the configured remote and materializes only
  the requested path or subtree.
- `loom cache status` reports hydrated, remote-only, partial, pinned, and evictable files from the
  current local metadata and best-effort remote object checks.

### Evict And Pin Safety

- `loom evict <path>` removes only clean local file bytes whose content still matches the tracked
  object id and whose object exists on the remote.
- Evict refuses dirty files, unsupported local entries, missing remote object proof, and pinned paths.
- `loom pin <path>` records offline-retention intent for the latest folder revision. It protects the
  pinned path from explicit eviction and appears in cache status.

### Integrity

- Object ids are BLAKE3 content hashes. Local object import recomputes the hash and rejects mismatched
  bytes.
- `loom fsck [FOLDER]` verifies local metadata references, revision/file-version links, checkpoint
  and pin targets, cache-entry hydration state, and local object-byte hashes.
- `loom object verify [FOLDER]` is the narrow object/cache integrity view over the same verifier.
- `loom remote check [FOLDER]` reads the configured remote cursor, checks that the cursor pack is
  readable, and proves every locally referenced object is available through the remote backend.
- `loom doctor [FOLDER]` combines local verification, worktree policy surfacing, and remote
  availability when a remote is configured. It is report-only and does not attempt automatic repair.
- Hosted object upload also recomputes the object hash at the API boundary. A request whose path
  object id does not match the body is rejected and the object is not persisted.
- Pack decoding validates row shape, object payload availability, and object payload size. The current
  format does not sign metadata packs.

### Source Safety

- Git metadata is local folder context. Loom protects `.git` metadata and does not materialize it into
  clones.
- Generated dependency and build directories such as `node_modules` and `dist` are suppressed.
- Secret-looking files are blocked before sync. The smoke checks that raw secret bytes are absent from
  remote storage, local Loom objects, and evidence logs.

## Non-Guarantees

- Sparse clone is metadata-only, not an OS virtual filesystem. Missing files do not hydrate
  transparently on open.
- Future FUSE, File Provider, Cloud Files, or similar support is an adapter over Loom folder
  revisions, file versions, object bytes, cache entries, and hydration state. It must not become the
  source of truth for folder history.
- Chunk vocabulary exists for future sparse transfer, but this implementation moves whole object bytes.
- Remote-only metadata is not enough to work offline. A path must be hydrated while online before
  offline use. Pin it after hydration only when the local bytes should be protected from later
  eviction.
- Repair remains intentionally conservative. The current inspection commands report corruption and
  missing availability; they do not rewrite metadata, discard objects, or advance cursors.
- Pinning protects local eviction; it is not a remote legal-hold or team retention policy.
- Conflict refusal does not resolve conflicts. Users still need a manual recovery path after divergent
  folder state.
- Hosted local-dev sessions and `devbox://` engine URLs are test/dev plumbing. They are not the final
  product share-token UX.
- The local-fs remote and local `devbox-api` smoke path do not prove live R2, Postgres, multiregion
  durability, compression, signed packs, or automatic object repair.

## Adapter And Backend Boundary

Loom remains folder/revision-first:

- Core state is local metadata, object bytes, cache entries, file versions, folder revisions,
  checkpoints, pins, and cursors.
- A mount layer may present remote-only files as placeholders later, but it is an adapter that calls
  Loom hydrate/evict/cache primitives.
- Backend implementations are pluggable behind the remote trait. The current boundaries are
  `local-fs` and Devbox hosted storage. S3, R2, MinIO, or self-hosted object/metadata services are
  backend choices, not changes to the Loom storage model.
- Cache metadata is separate from mounted paths. A cache entry records local byte availability for an
  object; a future virtual filesystem must keep that metadata consistent instead of inventing a
  second availability source.

## Evidence Path

Run the storage consistency evidence locally:

```text
scripts/mvp-two-device-smoke
```

On Windows:

```text
powershell -ExecutionPolicy Bypass -File scripts/mvp-two-device-smoke.ps1
```

The smoke writes redacted logs under the printed evidence directory and proves:

- local-only Loom history and checkpoints
- local-fs eager sync and clone
- local-fs sparse clone, hydrate, evict, pin, and cache status
- Devbox hosted eager share/clone through local `devbox-api`
- hosted metadata/object split and object hash mismatch rejection
- Git metadata protection, generated dependency suppression, secret blocking, and conflict refusal

The script starts the local `devbox-api` with `DEVBOX_API_METADATA_MODE=memory`, so this evidence
path does not require live Postgres or R2. Production-shaped API runs still use Postgres metadata by
leaving that variable unset.

Focused unit coverage for the same contract lives in `loom-sync`, `loom-store`, `loom-pack`,
`devbox-api`, and `devbox-remote` tests.
