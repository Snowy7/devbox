# Experiments And Research Plan

## Goal

Validate the wedge before building a broad platform.

## Core Hypotheses

1. Developers with multiple machines feel acute pain around uncommitted work and local project continuity.
2. Developers will trust a tool that snapshots and restores before it syncs.
3. Dev-aware policy is enough to differentiate from generic sync.
4. A USD 10/month Pro plan is plausible for power users.
5. The same workspace graph can later support teams and agents.

## Prototype Experiments

### Experiment 1: Local Snapshot Trust

Build a CLI that snapshots a repo and restores it.

Success:

- user understands what happened
- user can restore untracked and modified files
- user says it feels safer than stash/WIP commit

### Experiment 2: Second Device Demo

Fake or build enough sync to show desktop-to-laptop continuation.

Success:

- user reacts with "I want this"
- user asks about safety, not usefulness
- user can explain the product to another developer

### Experiment 3: Ignore Policy Explainability

Show files included/excluded from sync.

Success:

- user agrees with defaults
- user can override a policy
- user understands why `node_modules` is not copied

### Experiment 4: Git Safety Messaging

Show architecture note and Git status before/after restore.

Success:

- user is comfortable testing on non-critical repo
- user understands `.git` is not dumb-synced

### Experiment 5: Pricing Smoke Test

Landing page with Pro pricing.

Success:

- 5%+ of waitlist clicks pricing CTA
- 1%+ of visitors join paid-intent waitlist
- qualitative interviews confirm value at USD 10/month

## User Interview Questions

1. How many machines do you code on weekly?
2. What is in your main code folder?
3. When did you last lose, forget, or manually move work-in-progress?
4. Do you ever avoid switching machines because setup/state is annoying?
5. How do you handle uncommitted work today?
6. Have you tried Dropbox, iCloud, Syncthing, rsync, dotfiles, or dev containers for this?
7. What would make you afraid to use this?
8. What would make you pay for this?
9. Would this be more valuable personally, for your team, or for agents?
10. What is the one repo you would never let this touch until trust is proven?

## Alpha Selection Criteria

Choose users with:

- two or more machines
- active local development
- mixed stacks
- Git experience
- willingness to test on real but non-critical projects
- strong feedback habits

Avoid initially:

- giant monorepos
- regulated company code
- air-gapped workflows
- repos with extreme binary assets
- users expecting enterprise support

## Validation Milestones

| Milestone | Evidence Needed |
| --- | --- |
| Problem validation | 20 interviews, 70%+ report recurring pain |
| Trust validation | 10 users restore real WIP successfully |
| Sync validation | 25 users complete two-device workflow |
| Pricing validation | 100 paid-intent signups or 20 paid pilots |
| Team validation | 5 design partners identify budget owner and use case |

