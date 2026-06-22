# Alpha Readiness Evidence

This is the concise evidence map for the alpha readiness, live sync hardening, and hosted deployment
polish PR.

## Canonical Proof Path

For the product MVP path, run:

```text
scripts/mvp-two-device-smoke
```

On Windows, run:

```text
powershell -ExecutionPolicy Bypass -File scripts/mvp-two-device-smoke.ps1
```

The smoke builds local binaries when needed, starts a temporary `devbox-api` with in-memory
metadata, simulates two machines, and writes redacted logs under the printed evidence directory.
It does not require live R2 or Postgres.

For the workspace adapter alpha path, run:

```text
powershell -ExecutionPolicy Bypass -File scripts/alpha-workspace-adapters-smoke.ps1
```

That smoke uses a temporary in-memory `devbox-api` plus local Loom folders to prove sparse folder
intents, agent virtual sessions, materialized sandbox fallback, and filesystem adapter alpha
truthfulness. It is local-dev evidence only; it does not claim native hydrate-on-open or OS
placeholder support.

## What The Smoke Proves

- Loom can track, checkpoint, sync, clone, sparse clone, hydrate, evict, pin, cache status, and
  diagnose a shared folder.
- Sparse clones preserve remote-only object metadata and do not treat absent source files as
  deletions.
- Devbox can `login`, `share`, `clone`, push a source edit, and pull it to another local machine
  through `devbox-api`.
- Hosted metadata and object bytes are split; object uploads are hash-validated and mismatches are
  refused.
- Git metadata and generated dependency/build folders are not materialized into clones.
- Secret-looking files are blocked before sync; raw secret fixture bytes are absent from remote,
  object cache, and evidence logs.
- Divergent cursor/conflict states refuse safely instead of auto-merging.

## Workspace Adapter Alpha Proofs

- Devbox sparse clone starts metadata-only, then `status`, `hydrate`, `warm`, `keep`, and
  `free-space` expose cache intent without treating cloud-only files as deletions.
- `free-space` succeeds only for clean, unpinned local bytes with hosted proof and refuses when that
  proof is missing.
- Loom agent workspace sessions can virtual-read, virtual-exec, write overlays, diff, checkpoint,
  and discard without materializing the whole folder.
- Materialized fallback runs a real command in an isolated sandbox, captures safe source changes,
  and refuses host shared-folder mutation outside capture.
- Native filesystem adapters report unsupported mount status truthfully and record no success, while
  `--adapter local-dev` records only metadata-only simulated mount/status/unmount state.

## Focused Checks

Useful targeted checks for this PR:

```text
cargo test -p loom-daemon
cargo test -p loom-cli --test cli
cargo test -p devbox-cli --test product_cli -- --test-threads=1
cargo test -p devbox-api
```

The Loom CLI test suite also covers `loom cache warm` behavior and manifest-only warmup.
The product CLI alpha test uses local API servers and is documented as serial because the default
parallel runner is still flaky on that test target.

Operator-only hosted deployment checks can additionally run the `devbox-api` Railway/container path
with Postgres and server-side R2 configured. That is not required for the local alpha smoke.
