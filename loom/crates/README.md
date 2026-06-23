# Loom Crates

Compileable Loom crates live here.

Keep source-control and sync engine concepts in this area. Bindhub-specific hosted product behavior
belongs in [`../../bindhub/crates`](../../bindhub/crates).

`loom-workspace` owns adapter/session behavior for virtual views over folder revisions. It is not an
OS filesystem mount layer; native host adapters belong in a later arc step.
