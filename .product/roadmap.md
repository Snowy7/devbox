# Roadmap

## Roadmap Principle

Ship the narrowest product that proves folder trust, while building the graph, policy, and sync
foundations needed for teams, agents, and Loom.

## Phase 0: Prototype Foundation

Timeframe: 4 to 6 weeks

Goal: prove local snapshot and restore safety.

Deliverables:

- local developer-folder scanner
- default dev ignore rules
- content-addressed local object store
- snapshot manifest
- restore command
- local change feed / operation log comparing current folder state to the latest snapshot
- Git repo detector
- destructive test suite

Exit criteria:

- can snapshot and restore 20 representative repos
- no Git corruption in test matrix
- can restore uncommitted and untracked files
- generated directories suppressed by policy
- pending local operations are deterministic and repeatable without cloud sync

## Phase 1: Personal Alpha

Timeframe: 8 to 10 weeks

Goal: two-device sync for trusted alpha users.

Deliverables:

- account/device pairing
- long-running watcher that feeds the local operation log
- encrypted object upload/download
- second-device materialization
- CLI: `init`, `status`, `devices`, `snapshot`, `restore`, `pause`
- basic tray status
- secret detection and block policy
- conflict-as-divergent-snapshot model

Completed foundations:

- local watcher daemon feeding the pending operation log
- local account/current-device identity and key material
- encrypted remote blob transport through a local filesystem provider
- S3-compatible encrypted remote blob provider foundation for Cloudflare R2, AWS S3, and MinIO
- hosted metadata API/store/handler foundation for accounts, devices, implementation folder scopes, published snapshot
  manifests, and server-side device/folder cursors with compare-and-set updates
- local/mock auth session, pairing invitation, approved-device trust, key envelope, revocation, and
  cursor primitives
- production-shaped account ownership proof and account session primitives with provider
  subject/email/domain proof, token-hash sessions, expiration, revocation, and no-network CLI/dev
  persistence
- authenticated hosted metadata request context with explicit mock-dev header mode for tests/dev
  and production-shaped account-session bearer auth resolved through the hosted session store
- managed object credential lease foundation for account/session/folder-scoped R2/S3/MinIO-shaped
  provider references, capabilities, expiration, revocation, rotation generation, and no-network
  mock/dev CLI smoke checks
- production-shaped pairing recovery and rotation primitives, including recovery grant references,
  revocation, device rotation intents, and key-envelope rotation generation
- local/mock second-device materialization through an encrypted local filesystem remote, including
  publish/import, deferred materialize cursor commit, and safe restore-engine apply
- local high-confidence secret detection and block-by-default policy before snapshot blobs become
  uploadable
- local conflict-as-divergent-snapshot compare and persistence metadata
- local sync preflight and cursor reconciliation that refuses divergent local/mock import or
  materialization and persists readable conflict records without advancing cursors
- opt-in mock-dev hosted metadata wiring for publish/import/materialize manifest discovery and
  server-side device/folder cursor compare-and-set
- no-network Electron private-alpha shell for status, folders, sync activity, conflicts, devices,
  secret policy, and settings
- explicit path-scoped secret policy records for block, template, and encrypted envelope references
- guarded manual conflict resolution records with no automatic apply or merge

Remaining deferred work:

- live OAuth/OIDC sign-in and hosted account ownership proof verification
- production pairing UX and live recovery/rotation user flows
- live managed cloud object credential provisioning against Cloudflare/AWS/MinIO APIs
- production hosted metadata deployment hardening beyond the no-network SQLite/dev boundary
- production deployment hardening
- automatic conflict merge/apply resolution
- paid/team administration
- agent workflows
- Loom source-control semantics

Exit criteria:

- 25 alpha users complete desktop-to-laptop workflow
- P50 second-device folder ready under 10 minutes
- zero data loss incidents
- zero Git corruption incidents

## Phase 2: Trust Beta

Timeframe: 10 to 12 weeks

Goal: make the product safe enough for paid personal beta.

Deliverables:

- timeline UI
- folder policy editor
- rehydration hints
- restore file/folder/code-root
- sync explain view
- Windows/macOS/Linux watcher hardening
- cost and cache controls
- beta onboarding

Exit criteria:

- 500 beta users
- 40% connect second device within 7 days
- 30% weekly active synced folders
- restore success above 95%
- support load understood by issue category

## Phase 3: Paid Pro

Timeframe: 8 to 12 weeks after beta signal

Goal: prove willingness to pay.

Deliverables:

- billing
- storage limits
- retention tiers
- priority folder selection
- VS Code extension
- import/migration assistant
- public docs

Exit criteria:

- 1,000 paid users or strong equivalent waitlist conversion
- churn reasons understood
- storage cost per active user below pricing guardrail
- activation and retention improve by cohort

## Phase 4: Team Preview

Goal: make individual trust useful to teams.

Deliverables:

- team accounts
- managed policies
- device approval
- shared workspace links
- audit log
- retention controls
- SSO preview
- admin recovery

Exit criteria:

- 10 design partners
- measured onboarding or recovery time saved
- admin trust score positive
- no increase in core safety incidents

## Phase 5: Agent Sandboxes

Goal: make agents safe by default.

Deliverables:

- isolated agent sandboxes
- agent folder-state API
- snapshot provenance
- run/test result attachment
- discard/merge agent work
- semantic summaries
- policy sandboxing

Exit criteria:

- agent sandbox creation under 30 seconds for typical folder
- agent changes reviewable without noisy micro-commits
- accepted agent work rate improving
- rollback success above 99%

## Phase 6: Loom Layer

Goal: expose Loom's source-control UX without breaking Git compatibility.

Deliverables:

- sandbox and checkpoint UI
- Git import/export
- semantic checkpoints
- branchless local flow
- GitHub/GitLab publishing
- PR export
- operation log search

Exit criteria:

- users perform meaningful workflows without manual branch/stash/worktree commands
- teams can keep GitHub/GitLab as system of record while using Bindhub locally
- source-control layer increases retention and paid conversion
