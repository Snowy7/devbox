# Devbox Hosted Loom Remote

PR6 makes Devbox a hosted storage boundary for Loom remotes without moving
folder-state decisions out of Loom.

Devbox API owns:

- local-dev session creation and bearer session validation
- device registration and device-scoped requests
- shared folder registry and account membership checks
- Loom metadata pack byte storage
- Loom object byte storage for metadata/data split sync
- shared-folder cursor metadata with compare-and-set semantics

Devbox hides the server-owned storage layout from clients. Hosted remotes address
metadata packs and object bytes through the Devbox API; they do not receive
bucket names, object keys, or direct storage credentials.

Loom owns:

- object identity, object hydration state, and pack contents
- file versions and folder revisions
- checkpoints, pins, restore safety, and divergent-state refusal
- secret blocking, generated ignores, and Git metadata protection

The MVP local API is deterministic and file-backed. It is suitable for tests and
local development:

```text
devbox-api --root <storage> --bind 127.0.0.1:0
loom remote add devbox http://127.0.0.1:<port> <folder>
loom sync <folder>
loom clone devbox://<shared-folder-id>?api=...&session=...&device=... <target>
```

The `devbox://...` URL is intentionally engine-facing. It includes local-dev
session material so PR6 can prove the hosted remote path without adding product
commands. PR7 should replace this with product UX such as `devbox login`,
`devbox share`, `devbox clone`, pause/resume, and unlink.

Known follow-up work:

- replace deterministic local-dev sessions with production auth
- issue clone/share tokens instead of embedding session material in URLs
- harden durable cloud storage behind the API boundary
- add richer folder permissions beyond owner access
- hide remote details from normal Devbox users
