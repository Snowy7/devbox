# Bindhub Hosted Loom Remote

Bindhub is a hosted storage boundary for Loom remotes without moving folder-state
decisions out of Loom.

Bindhub API owns:

- local-dev session creation and bearer session validation
- device registration and device-scoped requests
- shared folder registry and account membership checks
- Loom metadata pack byte storage
- Loom object byte storage for metadata/data split sync
- shared-folder cursor metadata with compare-and-set semantics

Bindhub hides the server-owned storage layout from clients. Hosted remotes address
metadata packs and object bytes through the Bindhub API; they do not receive
bucket names, object keys, or direct storage credentials.

Loom owns:

- object identity, object hydration state, and pack contents
- file versions and folder revisions
- checkpoints, pins, restore safety, and divergent-state refusal
- secret blocking, generated ignores, and Git metadata protection

The MVP local API is deterministic and file-backed. It is suitable for tests and
local development:

```text
bindhub-api --root <storage> --bind 127.0.0.1:0
loom remote add Bindhub http://127.0.0.1:<port> <folder>
loom sync <folder>
loom clone bindhub://<shared-folder-id>?api=...&session=...&device=... <target>
```

The `bindhub://...` URL is intentionally engine-facing. Product flows should use
`bindhub login`, `bindhub share`, `bindhub clone`, pause/resume, and unlink instead
of asking users to handle remote details.

Known follow-up work:

- replace deterministic local-dev sessions with production auth
- issue clone/share tokens for engine URLs that still need delegated access
- harden durable cloud storage behind the API boundary
- add richer folder permissions beyond owner access
- hide remote details from normal Bindhub users
