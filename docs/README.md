# Devbox Docs

Devbox is a folder continuity product for developers.

The promise is simple:

> Open another machine and your code folder is already there.

These docs still contain some early alpha wording such as "project", "snapshot", and "Git
replacement". Read those as implementation-era terms, not as the product language. The product is
about shared developer folders. A folder may contain many repos, one repo, no repo, nested tools, or
plain files. Devbox should still work.

The source-control primitive underneath Devbox is codenamed **Loom**. Loom is not the user-facing
product. It is the internal direction for file versions, folder revisions, checkpoints, safe
parallel sandboxes, shared local overlays, and agent-friendly folder state. Git remains important as
a compatibility surface because developers use it today, but Git is not the foundation Devbox is
trying to build.

Start with [Loom And Devbox](devbox/loom-and-devbox.md) before reading older architecture slices.
For current alpha proof, read [Alpha Readiness Evidence](evidence/alpha-readiness.md) and
[Workspace Adapter Alpha](devbox/workspace-adapters-alpha.md). Run `scripts/mvp-two-device-smoke`
for the product MVP path and `scripts/alpha-workspace-adapters-smoke.ps1` for sparse folder,
agent workspace, materialized fallback, and filesystem adapter evidence.

For the intended full-scale repository and language split, read
[Full-Scale Project Shape](architecture/full-scale-project-shape.md).

The top-level [Loom area](../loom/README.md) and [Devbox area](../devbox/README.md) are active crate
homes now. Most older architecture pages are legacy alpha records and are marked with a
compatibility note when they still use implementation terms such as `project` or `snapshot`.
