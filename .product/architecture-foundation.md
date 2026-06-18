# Architecture Foundation

## Architecture Goal

Build a local-first, Git-compatible workspace graph that can support:

- personal code-folder sync
- automatic WIP snapshots
- reliable restore
- team sharing and policy
- copy-on-write agent workspaces
- future source-control primitives

The architecture should never depend on treating a developer code folder as dumb files.

## Conceptual Model

```text
Local filesystem
  -> watcher and scanner
  -> project classifier
  -> policy engine
  -> content-addressed object store
  -> workspace timeline
  -> encrypted sync protocol
  -> device materializer
```

## Core Entities

| Entity | Meaning |
| --- | --- |
| Account | Owner identity and billing/security scope |
| Device | Trusted machine with keys and capabilities |
| CodeRoot | User-selected folder such as `~/Code` |
| Project | Detected repo or directory inside a code root |
| PolicySet | Rules for sync, ignore, secrets, rehydration, and retention |
| Snapshot | Immutable workspace state at a point in time |
| Manifest | Path tree with blob ids, metadata, permissions, symlinks, and policy decisions |
| Blob | Content-addressed file chunk or packed object |
| Workstream | Mutable pointer to a line of snapshots |
| Operation | Timeline event: edit, snapshot, restore, divergence, merge, policy change |
| DeviceState | Last known project state on a device |
| GitState | Git-specific refs, HEAD, index, object, and remote metadata captured safely |
| SecretEnvelope | Encrypted secret payload or blocked secret decision |

## Storage Model

Local:

- embedded database for metadata, likely SQLite first
- local object cache
- append-only operation log
- project manifests
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
- local/mock auth session, pairing invitation approval, key envelopes, device revocation, and
  device/project cursor primitives exist in SQLite
- local/mock second-device materialization can publish/import an encrypted snapshot bundle through a
  local filesystem remote and apply it safely with the existing restore engine
- local high-confidence secret detection blocks detected files before blob-cache writes and local
  publish paths
- local conflict records compare divergent snapshots with path-level metadata rows and persist them
  in SQLite without source bytes
- real cloud authentication, hosted metadata, real object-storage credentials, production pairing UX,
  explicit secret allow policy, automatic merge, and conflict UI remain later Phase 1 work

## Content Addressing

Every durable file content chunk should be addressed by hash.

Benefits:

- deduplication across devices and projects
- fast restore
- cheap divergent snapshots
- foundation for copy-on-write agent workspaces
- future source-control compatibility

## Git Handling

Do not sync `.git` as ordinary files.

Git repositories need a dedicated adapter:

1. Detect repo root, worktree type, current HEAD, remotes, refs, submodules, sparse checkout, and worktree state.
2. Capture Git object requirements using Git plumbing where possible.
3. Snapshot tracked file state through the workspace manifest.
4. Snapshot uncommitted and untracked files separately.
5. Reconstruct on another device atomically:
   - clone/fetch from remote when available
   - hydrate missing Git objects from Devbox when no remote exists
   - apply WIP snapshot as a safe overlay
   - restore untracked files according to policy
6. Run verification such as `git status`, `git fsck` where appropriate, and manifest checks.

This lets Git keep doing what it is good at while Devbox owns live workspace continuity.

## Policy Engine

Policy layers, highest priority first:

1. explicit user/project policy
2. team policy later
3. secret policy
4. language/framework defaults
5. `.gitignore` and related ignore files
6. global Devbox safety defaults

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

Important caveat: some projects intentionally commit or depend on directories named like generated artifacts. The policy engine must explain and allow overrides.

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
- allow restore, compare, merge, or keep both workstreams

Example:

```text
project-a
  main workstream
  divergent snapshot from laptop at 2026-06-18 18:45
  divergent snapshot from desktop at 2026-06-18 18:47
```

## Sync Algorithm Shape

1. Watch filesystem events.
2. Debounce and classify changes.
3. Apply policy decisions.
4. Chunk and hash changed content.
5. Create local snapshot manifest.
6. Append operation log entry.
7. Upload missing blobs and encrypted manifest.
8. Update device cursor.
9. On receiving device, compare local cursor and policy.
10. Materialize snapshot atomically with rollback.

The first alpha workflow proves desktop-to-laptop continuation, but the account/device architecture
must remain multi-device-capable. A local installation should have one current local device identity,
while an account can accumulate many approved devices over time.

## Rehydration

The product should avoid syncing heavy dependency directories by default. It should make rehydration visible:

- "Run npm install"
- "Run pnpm install"
- "Run cargo fetch"
- "Create virtualenv"
- "Docker image must be rebuilt"

Later this can become automated per project policy.

## Future Team Foundation

Add later:

- team-owned policy sets
- SSO
- device approval
- managed retention
- audit log
- protected projects
- private package visibility
- shared workspace links
- admin recovery

These are metadata and policy layers on top of the same workspace graph.

## Future Agent Foundation

Agents should get copy-on-write workspaces:

- create agent workstream from snapshot
- materialize only changed files
- capture every agent operation
- provide diff, test, and provenance
- merge or discard workstream

This is much cleaner if the core snapshot graph exists from day one.

## Future Better-Git Foundation

Long term, Git becomes an adapter:

- import Git repo
- map commits to snapshots
- map branches to workstreams
- export selected workstream to Git commits
- publish to GitHub/GitLab
- preserve compatibility with existing CI and code review

New primitives:

- automatic checkpoints
- semantic summaries
- workstreams instead of branches
- snapshots instead of stashes
- copy-on-write spaces instead of worktrees
- operation log instead of reflog archaeology
