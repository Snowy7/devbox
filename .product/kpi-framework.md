# KPI Framework

## Measurement Goal

The metric system should prove that Bindhub makes developer folder continuity reliable, fast, and
trusted.

The first dashboard should answer:

1. Are users reaching a second-machine "it just worked" moment?
2. Are we preserving work safely?
3. Are sync decisions correct for developer folders?
4. Are users returning because the product reduced real friction?

## North Star

### Successful Workspace Continuations

Count of sessions where a developer resumes work on a different device and successfully opens,
edits, runs, or restores a folder state that was created elsewhere.

Formula:

```text
successful_workspace_continuations =
  cross_device_resume_sessions
  where folder_ready = true
  and required_files_available = true
  and no_blocking_conflict = true
  and user_action_within_30_minutes in (edit, test, run, commit, snapshot, restore)
```

Why it matters:

This measures the actual promise: close one machine, open another, keep coding.

## Primary KPIs

| KPI | Definition | Why it matters | MVP target |
| --- | --- | --- | --- |
| Activation to second device | Percent of new users who connect a second device and open a synced folder within 7 days | Proves the wedge | 40% beta, 60% guided install |
| Time to folder ready | Time from second-device sign-in to first selected folder ready | Measures magic | P50 under 10 minutes, P90 under 30 minutes |
| Snapshot capture latency | Time from filesystem change to durable local snapshot | Protects trust | P50 under 5 seconds, P95 under 30 seconds |
| Restore success rate | Percent of restore attempts that complete without manual support | Proves recovery | 95%+ |
| Weekly active synced folders | Shared folders with meaningful synced activity in a week | Measures retained utility | Up and to the right, segmented by folder size |

## Driver Metrics

| Driver | Definition | Use |
| --- | --- | --- |
| Folder analysis accuracy | Percent of folders correctly classified by stack and policy | Improves ignore and rehydration behavior |
| Generated artifact suppression rate | Bytes/files avoided by excluding generated directories | Controls cost and noise |
| Rehydration hint success | Percent of hints that lead to successful install/build | Makes ignored dependencies feel safe |
| Cross-device freshness | Percent of resumes where latest snapshot is available within target SLA | Measures continuity |
| Conflict rate | Divergent snapshots per 1,000 synced change events | Tracks multi-device correctness |
| Conflict resolution success | Percent of conflicts resolved or accepted without support | Tracks UX quality |
| Secret policy coverage | Percent of detected sensitive files with explicit policy | Prevents accidental exposure |

## Trust Guardrails

These metrics should be treated as executive-level guardrails.

| Guardrail | Definition | Target |
| --- | --- | --- |
| Data loss incidents | Confirmed cases where user work is unrecoverable due to Bindhub | 0 |
| Corrupt Git repo incidents | Repos made unusable by bindhub sync/restore | 0 |
| Secret leak incidents | Sensitive file synced contrary to policy | 0 |
| CPU overhead | Daemon CPU while idle and while syncing | Idle under 1%, sync bounded by policy |
| Battery impact | Battery drain attributable to daemon | Low enough for laptop trust |
| Disk amplification | Local cache bytes / source bytes | Keep visible and controllable |
| Support tickets per active user | Trust and complexity proxy | Declining by cohort |

## Team Expansion KPIs

When teams arrive:

- onboarding time saved
- laptop replacement recovery time
- policy compliance rate
- shared workspace open success
- number of support tickets avoided
- percent of shared folders with managed policies
- admin trust score

## Agent Expansion KPIs

When agents arrive:

- agent sandbox creation latency
- sandbox storage amplification
- agent change review success
- agent rollback success
- agent work accepted into human folder state
- agent-caused conflict rate
- provenance coverage for generated changes

## Instrumentation Events

Minimum event set:

- `device_connected`
- `folder_selected`
- `folder_analyzed`
- `policy_applied`
- `snapshot_created`
- `snapshot_uploaded`
- `snapshot_applied`
- `folder_ready`
- `resume_detected`
- `restore_started`
- `restore_succeeded`
- `restore_failed`
- `conflict_created`
- `conflict_resolved`
- `secret_detected`
- `secret_policy_set`
- `git_state_captured`
- `git_state_reconstructed`
- `rehydration_hint_shown`
- `rehydration_hint_succeeded`

## Review Cadence

Daily during alpha:

- data loss, corruption, secret incidents
- sync lag
- crash rate
- support tickets

Weekly during beta:

- activation
- time to folder ready
- successful continuations
- retained synced folders
- conflict and restore quality

Monthly:

- conversion
- retention
- storage cost per active folder
- pricing sensitivity
- segment performance
