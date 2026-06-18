# Hosted Metadata API Foundation

This Phase 1 slice adds a production-shaped metadata service boundary without making Devbox a full
hosted SaaS backend yet.

## Scope

`crates/devbox-metadata` owns deterministic request/response models, store semantics, and HTTP
handlers for:

- health checks
- mock-dev and account-session-authenticated device registration/upsert
- project registration/upsert
- project-scoped published snapshot manifest metadata
- project-scoped snapshot metadata lookup by snapshot id
- server-side device/project cursor reads
- server-side cursor compare-and-set updates
- account/session/project-scoped managed object credential lease records for future hosted
  R2/S3/MinIO-compatible credential provisioning

The service can run locally with SQLite and has an in-memory store for fast unit tests. That keeps
normal CI free of Docker, Postgres, cloud credentials, and network services while preserving a clear
future boundary for Postgres-backed hosted metadata.

## Metadata Model

The first service schema tracks:

- accounts
- devices
- projects
- published snapshot manifests
- device/project cursors

Published snapshot metadata is keyed by account, project, and snapshot id. The HTTP surface uses
project-scoped routes:

```text
PUT /v1/projects/:project_id/snapshots
GET /v1/projects/:project_id/snapshots/:snapshot_id
```

Snapshot records store object references and counts only. They do not store plaintext file bytes,
sync keys, device keys, R2 secrets, object credentials, or manifest contents.

Managed object credential lease records store redacted provider references, endpoint/bucket/region
shape, optional project scope, capabilities, expiration, revocation, and rotation generation only.
They do not store raw access keys, secret keys, session tokens, provider API tokens, OAuth tokens, or
raw credential hashes.

## Cursor Safety

Cursor updates use compare-and-set semantics:

```text
expected_cursor -> next_cursor
```

The server accepts the update only when the stored cursor exactly matches `expected_cursor`. If a
newer cursor has already been written, the service returns a conflict and reports the current
cursor. Clients therefore cannot blindly overwrite a newer device/project cursor.

This is the hosted counterpart to the local sync preflight foundation: server-side cursor state is
now shaped for arbitration, while automatic conflict merge/apply resolution remains deferred.

## Auth Boundary

Production OAuth is intentionally not implemented in this slice.

Handlers support explicit local-only mock headers for tests and development:

```text
x-devbox-mock-account-id
x-devbox-mock-device-id
```

Those headers are named as mock/dev credentials and are suitable only for local tests and development
flows. They are not account ownership proof, not a billing identity, and not safe for production
deployment.

Handlers also support production-shaped account-session auth:

```text
Authorization: Bearer <session-token>
```

The service hashes the presented bearer token transiently, resolves it through the hosted account
session store, refuses missing, expired, or revoked sessions, and uses the authenticated account id
for device, project, snapshot, cursor, and managed lease scoping. Mock-dev mode still rejects
header/body identity mismatches so tests and local sync flows stay explicit. Session-auth mode does
not trust caller-supplied account ids in request bodies.

The hosted alpha API can mint those sessions through one-time invite login:

```text
GET /ready
POST /v1/auth/alpha/login
GET /v1/auth/session
DELETE /v1/auth/session
```

The server binary defaults to account-session auth only. Mock-dev headers are accepted only when
`--allow-mock-auth` or `DEVBOX_ALLOW_MOCK_AUTH=true` is set. Alpha invite rows store only the invite
code hash; login returns the raw session token once, and the session table stores only its hash.

The service rejects obvious secret-bearing request material and its public CLI check prints a
sanitized endpoint, not raw input, keys, or object credentials.

Public API errors are sanitized. Client-domain ordering mistakes, such as writing a project before
registering the account/device or writing a cursor before registering the project, return 4xx
precondition errors rather than raw SQLite messages.

## CLI Boundary

`devbox metadata check --endpoint <URL> [--auth-mode mock-dev-headers|account-session]` validates
the local metadata service configuration without making a network request.

`devbox metadata alpha-invite create --db <METADATA_DB> --email <EMAIL>|--domain <DOMAIN>` creates
a one-time invite in the hosted metadata SQLite DB. `devbox auth hosted-login --api <URL> --email
<EMAIL> --invite-code-env <ENV>` exchanges it for a bearer session token.

The local/mock publish, import, and materialize flows can now opt into an in-process mock-dev SQLite
metadata store. That wiring registers published snapshot metadata, discovers manifest object keys by
project/snapshot id, and advances device/project cursors with hosted compare-and-set semantics while
keeping normal CI free of live network services.

## Future Postgres Boundary

The SQLite schema is deliberately small and maps one-to-one to future Postgres tables:

- `metadata_accounts`
- `metadata_devices`
- `metadata_projects`
- `metadata_snapshots`
- `metadata_device_project_cursors`

Moving to Postgres should replace the `MetadataStore` implementation, not the project-scoped API
models or cursor compare-and-set contract.

## Deferred

Remaining Phase 1 work includes:

- OAuth/OIDC sign-in and hosted provider proof verification beyond one-time alpha invites
- live managed R2/S3 credential provisioning and rotation against provider APIs
- production pairing UX and live recovery/rotation flows
- automatic conflict merge/apply resolution and user-facing conflict UI
- Electron tray/status integration
- production deployment hardening, observability, and abuse protection
