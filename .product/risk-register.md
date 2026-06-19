# Risk Register

## Risk Summary

The main risk is not whether sync can be built. The main risk is whether developers trust it with live source code.

## Product Risks

| Risk | Severity | Mitigation |
| --- | --- | --- |
| Users think this is just Dropbox or Syncthing | High | Lead with automatic WIP snapshots, Git-safe restore, and dev-aware folder policy |
| Users do not pay for personal sync | High | Validate Pro pricing before heavy team buildout |
| Product feels scary because it touches code | High | Local snapshots first, explainable sync, dry-run mode, visible restore |
| Too many edge cases for MVP | High | Start with explicit supported stacks and folder sizes |
| Conflict UX becomes confusing | Medium | Use divergent snapshots and clear device/time labels |

## Technical Risks

| Risk | Severity | Mitigation |
| --- | --- | --- |
| Git repo corruption | Critical | Never sync `.git` as dumb files; use Git adapter and atomic reconstruction |
| Data loss | Critical | Append-only local operation log, immutable snapshots, restore test matrix |
| Secret leakage | Critical | Default block, secret detection, explicit policies |
| File watcher inconsistency | High | Periodic scanner reconciliation, OS-specific test suites |
| Case-sensitivity bugs | High | Detect filesystem behavior and flag incompatible paths |
| Symlink/permission drift | High | Store metadata explicitly and test per OS |
| Generated file explosions | High | Default dev ignores, artifact suppression, policy explain view |
| Storage costs exceed pricing | High | Deduplication, chunking, generated artifact exclusions, folder limits |
| Large repo performance | Medium | Streaming manifests, incremental scanning, priority hydration |

## Business Risks

| Risk | Severity | Mitigation |
| --- | --- | --- |
| CDE vendors absorb the story | Medium | Differentiate as local-first folder continuity |
| GitHub adds similar WIP sync | High | Build cross-platform, Git-host-agnostic, local-first trust and timeline UX |
| Teams require compliance too early | Medium | Keep team preview with design partners only |
| Support burden overwhelms team | High | Narrow alpha, automated diagnostics, supported stack matrix |
| Market category unclear | Medium | Position around felt workflow, not category labels |

## Trust Requirements

Before public beta:

- documented restore guarantee
- local-first failure mode
- visible "what will sync" policy
- panic pause
- folder export
- support bundle without source content by default
- public architecture note on Git safety
