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
local account/current-device identity, and encrypted blob transport through a local filesystem
remote provider are in place. Real cloud authentication, device approval, key envelopes, revocation,
server-side cursors, and second-device materialization remain later Phase 1 work.
