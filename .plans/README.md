# Devbox MVP Plans

Open [html/index.html](html/index.html) to browse the MVP plan.

This folder turns the product strategy into implementation phases:

- Phase 0: local snapshot, restore, and local change feed foundation.
- Phase 1: private alpha with two-device sync.
- Phase 2: trust beta with Electron UI, timeline, policy, and restore flows.
- Architecture: daemon, desktop app, backend, storage, and repo layout.
- Validation: metrics, experiments, and launch gates.

The MVP rule is simple: earn trust before expanding into teams, agents, or a Git replacement.

Current Phase 1 foundation status: snapshot/restore, manual change-feed scanning, the local watcher,
local account/current-device identity, encrypted blob transport through a local filesystem remote
provider, S3-compatible encrypted object transport for R2/S3/MinIO-style remotes, hosted metadata
API/store/handler foundations for accounts/devices/projects/published manifests/server-side
compare-and-set cursors, local/mock auth plus device-pairing trust primitives, local/mock
second-device materialization through encrypted remotes, local high-confidence secret blocking, and
local conflict-as-divergent-snapshot compare/persist metadata are in place. Local sync preflight now
uses device/project cursors to refuse divergent local/mock import and materialization before file
blobs are downloaded or applied, while persisting readable conflict records. Publish/import/materialize
can now opt into in-process mock-dev hosted metadata for published manifest discovery and
server-side cursor compare-and-set. Production-shaped account ownership proof and account session
models now cover provider subject/email/domain proof, token-hash sessions, expiration, revocation,
and no-network CLI/dev persistence. Hosted metadata now has explicit mock-dev header auth for
tests/dev plus production-shaped account-session bearer auth resolved through the hosted session
store, with handlers scoping devices/projects/snapshots/cursors to the authenticated account.
Hosted metadata now also models account/session/project-scoped managed object credential leases
with redacted provider references, expiration, revocation, rotation generation, and no-network
mock/dev CLI smoke commands. Local pairing now includes no-network recovery grant references,
revocation, device rotation intents, and key-envelope rotation generation. The final private-alpha
surface now adds a no-network Electron shell, explicit path-scoped secret policy records, and guarded
manual conflict resolution records. Live OAuth/login integration, live Cloudflare/AWS credential
provisioning, production pairing UX, automatic merge/apply resolution, paid/team/agent/Git
replacement work, and production deployment hardening remain deferred.
