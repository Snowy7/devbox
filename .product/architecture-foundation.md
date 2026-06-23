# Architecture Foundation

## Architecture Goal

Build a local-first folder-state graph, codenamed Loom, that can support:

- personal code-folder sync
- automatic WIP snapshots
- reliable restore
- team sharing and policy
- agent sandboxes
- future source-control workflows

The architecture should never depend on treating a developer code folder as dumb files.

## Conceptual Model

```text
Local filesystem
  -> watcher and scanner
  -> developer-folder analyzer
  -> policy engine
  -> content-addressed object store
  -> folder timeline
  -> encrypted sync protocol
  -> device materializer
```

## Core Entities

| Entity | Meaning |
| --- | --- |
| Account | Owner identity and billing/security scope |
| Device | Trusted machine with keys and capabilities |
| SharedFolder | User-selected folder such as `~/Code` |
| FolderScope | Implementation boundary for one synced folder or nested folder |
| PolicySet | Rules for sync, ignore, secrets, rehydration, and retention |
| Object | Content-addressed bytes |
| FileVersion | One path's captured content state |
| FolderRevision | Coherent folder tree assembled from file versions |
| Snapshot | Current implementation term that maps roughly to a folder revision |
| Manifest | Path tree with blob ids, metadata, permissions, symlinks, and policy decisions |
| Blob | Content-addressed file chunk or packed object |
| Checkpoint | Human meaningful marker on captured folder state |
| Pin | Retention marker that protects a folder revision |
| Sandbox | Isolated writable view for parallel human or agent work |
| Overlay | Machine-local dependencies, caches, local config, and secrets shared safely across sandboxes |
| Operation | Timeline event: edit, snapshot, restore, divergence, merge, policy change |
| DeviceState | Last known folder state on a device |
| GitState | Git-specific context captured safely when the folder uses Git |
| SecretEnvelope | Encrypted secret payload or blocked secret decision |

## Storage Model

Local:

- embedded database for metadata, likely SQLite first
- local object cache
- append-only operation log
- folder manifests
- device sync cursors

Cloud:

- encrypted object storage for blobs
- metadata service for manifests and device cursors
- key envelope storage
- account/device registry
- optional relay for peer-to-peer sync later

Current Phase 1 foundation status:

- local watcher feeds the pending operation log
- local account/current-device identity exists in SQLite
- encrypted immutable blob upload/download works through a local filesystem provider
- S3-compatible encrypted blob transport can target Cloudflare R2, AWS S3, or MinIO with redacted
  env-based credential configuration
- hosted metadata API/store/handler foundations model accounts, devices, implementation folder scopes, published
  snapshot manifests, and server-side device/folder cursors with compare-and-set updates using
  SQLite for dev/tests
- local/mock auth session, pairing invitation approval, key envelopes, device revocation, and
  device/folder cursor primitives exist in SQLite
- production-shaped account ownership proof and account session primitives model external provider
  subject, verified email/domain, token-hash sessions, expiration, revocation, and safe no-network
  dev persistence without live OAuth
- hosted metadata handlers preserve explicit mock-dev header auth for tests/dev and add
  production-shaped account-session bearer auth resolved through the hosted session store, scoping
  devices, folder scopes, snapshots, cursors, and managed leases to the authenticated account boundary
- hosted metadata managed object credential lease primitives model account/session/folder-scoped
  R2/S3/MinIO-compatible provider references, capabilities, expiration, revocation, and rotation
  generation without storing or printing raw object credentials
- local pairing primitives now include recovery grant references, grant revocation, device rotation
  intents, and key-envelope rotation generation for future production recovery UX
- local/mock second-device materialization can publish/import an encrypted snapshot bundle through a
  local filesystem remote and apply it safely with the existing restore engine
- local high-confidence secret detection blocks detected files before blob-cache writes and local
  publish paths
- local conflict records compare divergent snapshots with path-level metadata rows and persist them
  in SQLite without source bytes
- local sync preflight reconciles receiving device/folder cursors with local and incoming
  snapshots, refuses divergent local/mock import or materialization, and persists readable conflict
  records without advancing the cursor
- local/mock publish/import/materialize can opt into in-process hosted metadata for manifest
  discovery and server-side cursor compare-and-set
- no-network Electron private-alpha shell, explicit path-scoped secret policy records, and guarded
  manual conflict resolution records are in place
- live OAuth/login integration, live Cloudflare/AWS object credential provisioning, production
  pairing/recovery UX, automatic merge/apply resolution, paid/team/agent/Loom work, and
  production deployment hardening remain deferred

## Content Addressing

Every durable file content chunk should be addressed by hash.

Benefits:

- deduplication across devices and folders
- fast restore
- cheap divergent snapshots
- foundation for agent sandboxes
- future source-control compatibility

## Loom History Model

Loom should not create a heavyweight whole-folder revision for every edit. It should capture and
deduplicate file content frequently, then assemble coherent folder revisions at stable boundaries:

- after a debounce window
- before or after a Loom command
- before sync
- before restore
- before sandbox merge
- when a user creates a checkpoint

The user can inspect and restore automatic folder revisions even if no checkpoint was made. A
checkpoint is a name and message attached to a folder revision, not the first moment the work becomes
durable. Pins protect important revisions from retention cleanup.

## Git Handling

Do not sync `.git` as ordinary files.

Git repositories need a dedicated adapter:

1. Detect repo root, worktree type, current HEAD, remotes, refs, submodules, sparse checkout, and worktree state.
2. Capture Git object requirements using Git plumbing where possible.
3. Snapshot tracked file state through the workspace manifest.
4. Snapshot uncommitted and untracked files separately.
5. Reconstruct on another device atomically:
   - clone/fetch from remote when available
   - hydrate missing Git objects from Bindhub when no remote exists
   - apply WIP snapshot as a safe overlay
   - restore untracked files according to policy
6. Run verification such as `git status`, `git fsck` where appropriate, and manifest checks.

This lets Git keep doing what it is good at while Bindhub owns live folder continuity.

## Policy Engine

Policy layers, highest priority first:

1. explicit user/folder policy
2. team policy later
3. secret policy
4. language/framework defaults
5. `.gitignore` and related ignore files
6. global Bindhub safety defaults

Default generated directories:

- `node_modules`
- `.next`
- `dist`
- `build`
- `target`
- `.venv`
- `venv`
- `.turbo`
- `.gradle`
- `.cache`
- `coverage`

Important caveat: some folders intentionally commit or depend on directories named like generated
artifacts. The policy engine must explain and allow overrides.

## Secrets

Secrets are blocked by default unless explicitly allowed.

Current local foundation: high-confidence provider tokens, private-key PEM headers, and selected
dotenv-style high-entropy secret assignments are blocked before regular file bytes are written to
the local blob cache. Blocked files are represented as manifest policy entries without blob refs.

Policy modes:

- block: never upload
- template: sync only a redacted template or variable names
- encrypted personal: sync encrypted to the user's devices
- encrypted team: sync encrypted to approved team members later
- external: point to 1Password, Doppler, Vault, SOPS, or another manager later

The product should detect likely secrets and pause that path until policy exists.

## Conflict Model

Never create opaque "conflicted copy" files as the main UX.

Instead:

- create divergent snapshots
- name the device and timestamp
- show affected files
- allow restore, compare, merge, or keep both states

Example:

```text
shared-folder
  main folder state
  divergent state from laptop at 2026-06-18 18:45
  divergent state from desktop at 2026-06-18 18:47
```

## Sync Algorithm Shape

1. Watch filesystem events.
2. Debounce and classify changes.
3. Apply policy decisions.
4. Chunk and hash changed content.
5. Create local snapshot manifest.
6. Append operation log entry.
7. Upload missing blobs and encrypted manifest.
8. In hosted metadata mode, register published snapshot metadata.
9. Update device cursor, using hosted compare-and-set first when metadata mode is enabled.
10. On receiving device, discover the manifest object key through hosted metadata when enabled,
    then compare local cursor and policy.
11. Refuse divergent local and incoming snapshots with a readable conflict record before applying
    bytes.
12. Materialize snapshot atomically with rollback when preflight and restore safety allow.

The first alpha workflow proves desktop-to-laptop continuation, but the account/device architecture
must remain multi-device-capable. A local installation should have one current local device identity,
while an account can accumulate many approved devices over time.

## Rehydration

The product should avoid syncing heavy dependency directories by default. It should make rehydration
or overlay behavior visible:

- "Run npm install"
- "Run pnpm install"
- "Run cargo fetch"
- "Create virtualenv"
- "Docker image must be rebuilt"

Later this can become automated per folder policy.

## Future Team Foundation

Add later:

- team-owned policy sets
- SSO
- device approval
- managed retention
- audit log
- protected folders
- private package visibility
- shared folder links
- admin recovery

These are metadata and policy layers on top of the same Loom folder-state graph.

## Future Agent Foundation

Agents should get sandboxes:

- create agent sandbox from a checkpoint or current folder state
- isolate source edits while sharing safe overlays
- capture every agent operation
- provide diff, test, and provenance
- merge or discard sandbox work

This is much cleaner if the core snapshot graph exists from day one.

## Future Loom Foundation

Long term, Loom becomes the source-control primitive and Git becomes an adapter:

- import Git repo
- map commits to snapshots
- map branches to sandboxes or checkpoints where needed
- export selected checkpoint/proposal to Git commits
- publish to GitHub/GitLab
- preserve compatibility with existing CI and code review

New primitives:

- automatic checkpoints
- semantic summaries
- sandboxes instead of branches/worktrees as the default parallel-work model
- snapshots/checkpoints instead of stashes
- shared overlays instead of duplicated local environments
- operation log instead of reflog archaeology
