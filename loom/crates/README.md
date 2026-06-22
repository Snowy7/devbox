# Loom Crates

Compileable Loom crates live here.

Keep source-control and sync engine concepts in this area. Devbox-specific hosted product behavior
belongs in [`../../devbox/crates`](../../devbox/crates).

`loom-workspace` owns adapter/session behavior for virtual views over folder revisions. It is not an
OS filesystem mount layer; native host adapters belong in a later arc step.
