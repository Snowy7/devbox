# Product And Business Analysis

## Recommendation

Build the first product as solo developer, multi-machine code-folder sync with automatic snapshots
and Git-safe folder state capture.

Do not expose Loom as a Git replacement first. Do not build a CDE first. Do not build team
administration first.

The foundation should support those paths, but the first wedge should prove the trust loop.

## Why This Wedge

The pain is immediately understandable:

> I was working on my desktop. I open my laptop. Everything is just there.

This has three advantages:

- It is emotionally legible.
- It is demoable in under two minutes.
- It forces the product to solve the hardest technical trust problem early.

## Strategic Logic

### What Git Does Well

- committed history
- distributed collaboration
- branching and merging
- remote publishing
- ecosystem compatibility

### What Git Does Poorly

- uncommitted work across devices
- ignored-but-important files
- local environment continuity
- safe agent experimentation
- readable WIP timelines
- beginner-friendly branch/stash/worktree UX

### What Generic File Sync Does Poorly

- `.git` internals
- symlinks and permissions
- build outputs
- dependency directories
- file watchers
- generated artifacts
- local databases
- secrets
- conflict semantics

The opportunity is the gap between Git and generic sync. Loom is the long-term primitive that can
own that gap, but Devbox should first make the folder-continuity promise real.

## Alternatives Considered

### Start With Loom As The Visible Product

Pros:

- ambitious story
- clear differentiation
- aligned with developer discourse around better source control

Cons:

- high switching cost
- trust barrier is enormous
- teams cannot adopt without ecosystem compatibility
- hard to explain before users feel the workspace pain

Decision: not first. Build Loom underneath, but sell Devbox as folder continuity.

### Start With Cloud Development Environments

Pros:

- enterprise budget exists
- security and onboarding pain are real
- clear team buyer

Cons:

- crowded with GitHub Codespaces, Coder, Google Cloud Workstations, Gitpod, and others
- moves users away from the local workspace instead of making local work continuous
- infrastructure-heavy

Decision: not first.

### Start With Team Workspace Sharing

Pros:

- more direct revenue path
- collaboration story is powerful

Cons:

- team trust requires personal trust first
- adds RBAC, audit, and policy complexity too early

Decision: phase two or three.

### Start With Personal Workspace Sync

Pros:

- sharpest wedge
- lowest buyer complexity
- strongest founder-led adoption path
- directly validates trust, sync, restore, and Git compatibility

Cons:

- consumer/prosumer willingness to pay must be proven
- support burden can be high if sync is unreliable
- may be dismissed as "Syncthing with ignores" unless the timeline and Git safety are excellent

Decision: yes.

## Initial Product Bet

The MVP should not merely sync files. It should introduce a local folder timeline from the first day.

If the product only syncs files, it becomes a fragile Dropbox clone.

If it snapshots folder state, it becomes the first layer of Loom.

## Prioritized MVP Capabilities

1. Developer-folder analyzer and policy engine.
2. Git-safe state capture.
3. Content-addressed local snapshots.
4. Encrypted sync backend.
5. Second-device folder materialization.
6. Restore timeline.
7. Conflict-as-divergent-snapshot model.
8. Secret policy.
9. CLI and minimal tray app.

## Product Quality Bar

Alpha can be rough visually.

Alpha cannot be rough with data safety.

Required before private alpha:

- repeatable restore test suite
- Git repo corruption tests
- power-loss tests
- concurrent edit tests
- massive generated file tests
- symlink and permissions tests
- case-sensitivity tests
- secret policy tests
- Windows/macOS/Linux watcher tests

## Business Model Read

The solo wedge can support Pro revenue, but the venture-scale path requires expansion into team and agent infrastructure.

The right sequencing is:

1. Earn individual trust.
2. Convert power users to Pro.
3. Sell teams on policy, onboarding, and recovery.
4. Sell agent sandboxes as infrastructure.
5. Replace more source-control workflows over time.

## Most Important Unknowns

- Will developers pay USD 10/month for personal code-folder continuity?
- Can we make generated-file and dependency behavior feel obvious instead of surprising?
- Can we avoid `.git` corruption across all normal workflows?
- Does the product still feel valuable if users already use GitHub, dotfiles, and package managers well?
- Can we create a restore/conflict UX that feels safer than Git stash/reflog?

## Validation Priority

Before writing the full sync backend, prototype the local snapshot and restore layer.

If users do not trust automatic local snapshots, they will not trust cross-device sync.
