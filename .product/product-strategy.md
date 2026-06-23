# Product Strategy

## One-Line Product

Bindhub is developer-native folder sync: your code folder, work-in-progress, and local context follow
you across every machine.

## Category Bet

The next source-control primitive is not the commit. It is the live folder timeline.

Git records intentional repo history. Bindhub records live developer folder state:

- changed files
- untracked files
- local notes
- generated-but-important fixtures
- folder policies
- secrets policy
- device state
- automatic checkpoints
- agent sandboxes

The codename for this deeper primitive is **Loom**. Bindhub is the product users install. Loom is the
source-control engine direction underneath it.

Loom history should be layered: frequent file versions, coherent folder revisions at stable
boundaries, and human checkpoints for moments worth naming.

## Initial Wedge

Personal multi-machine sync for `~/Code`.

The first product promise:

> Close desktop. Open laptop. Keep coding.

This is intentionally narrower than teams, agents, or Loom as a visible source-control product. It tests
the hardest trust loop directly: can developers let us touch their live folder?

## Expansion Path

1. Personal sync: make code folders continuous across machines.
2. Recovery: automatic snapshots, restore, and folder-level timelines.
3. Sharing: send a trusted collaborator the exact current workspace state.
4. Teams: managed policies, audit, device replacement, onboarding, and private packages.
5. Agents: safe sandboxes, reviewable agent timelines, merge or discard.
6. Loom: checkpoints, sandboxes, overlays, GitHub/GitLab export, branchless local UX.

## Personas

### Indie Developer

Has a desktop and laptop. Works across many folders and repos. Hates setup drift and forgotten WIP.

Primary job: "Let me continue wherever I am."

### Founder/Small Team Engineer

Moves between laptop, desktop, server, and AI coding tools. Needs speed without losing control.

Primary job: "Keep my work safe and runnable while I move fast."

### Platform Lead

Owns onboarding, security, laptop replacement, and source-code governance.

Primary job: "Give every developer a safe, consistent workspace without centralizing all work into a cloud IDE."

### Agent Power User

Runs coding agents in parallel and needs isolation, review, rollback, and provenance.

Primary job: "Let agents explore without wrecking my main workspace."

## Product Principles

- Never lose work.
- Git-compatible because Git is normal developer context.
- Local-first by default.
- Developer state is structured, not dumb files.
- Generated artifacts should be rehydrated, not blindly copied.
- Secrets require explicit policy.
- Divergence should become readable snapshots, not mystery conflict files.
- Every sync decision should be explainable.
- The product must feel boringly reliable before it feels magical.

## Positioning

### For Individuals

Your code folder, everywhere. Works with Git, VS Code, JetBrains, terminal, and your existing folders.

### For Teams

Reliable developer workspaces without forcing every engineer into a cloud IDE.

### For Agents

Safe sandboxes for AI agents, with reviewable timelines and Git-compatible output.

## Non-Goals For MVP

- Asking users to replace Git first.
- Hosting code review.
- Building a cloud IDE.
- Supporting real-time Google-Docs-style collaborative editing.
- Syncing every generated dependency directory.
- Solving enterprise device management.
- Becoming a full backup product.

## MVP Scope

Must have:

- cross-platform local daemon
- folder selection
- developer-folder analysis
- Git-safe state capture
- dev-aware ignore defaults
- encrypted sync
- automatic WIP snapshots
- second-device restore
- readable conflicts as divergent snapshots
- CLI status and restore

Should have:

- basic desktop tray/menu
- VS Code extension
- folder health warnings
- dependency rehydration hints
- secret policy prompts

Can wait:

- teams
- role-based access control
- agent sandboxing
- semantic commits
- hosted review UI
- enterprise admin controls

## Product Surfaces

- Local daemon: watches, snapshots, syncs, restores.
- CLI: status, devices, folders, pause, snapshot, restore, explain.
- Tray/menu app: health, conflicts, sync state.
- Timeline UI: folder restore points and divergences.
- Editor extension: current state, checkpoint/sandbox, restore, conflict notices.
- Web app later: devices, team policies, shared workspaces, billing.
