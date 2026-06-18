# Hosted Metadata API Foundation

This Phase 1 slice adds a production-shaped metadata service boundary without making Devbox a full
hosted SaaS backend yet.

## Scope

`crates/devbox-metadata` owns deterministic request/response models, store semantics, and HTTP
handlers for:

- health checks
- mock-dev device registration/upsert
- project registration/upsert
- published snapshot manifest metadata
- snapshot metadata lookup by snapshot id
- server-side device/project cursor reads
- server-side cursor compare-and-set updates

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

Published snapshot metadata stores object references and counts only. It does not store plaintext
file bytes, sync keys, device keys, R2 secrets, object credentials, or manifest contents.

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

## Mock-Dev Auth Boundary

Production OAuth is intentionally not implemented in this slice.

Handlers require explicit local-only mock headers:

```text
x-devbox-mock-account-id
x-devbox-mock-device-id
```

Those headers are named as mock/dev credentials and are suitable only for local tests and development
flows. They are not account ownership proof, not a billing identity, and not safe for production
deployment.

The service rejects obvious secret-bearing request material and its public CLI check prints only
configuration shape, not raw keys or object credentials.

## CLI Boundary

`devbox metadata check --endpoint <URL> [--auth-mode mock-dev-headers]` validates the local metadata
service configuration without making a network request.

The existing local-first CLI, local SQLite store, encrypted blob sync, S3-compatible provider, and
local/mock materialization flows are not wired to the hosted metadata service in this slice.

## Future Postgres Boundary

The SQLite schema is deliberately small and maps one-to-one to future Postgres tables:

- `metadata_accounts`
- `metadata_devices`
- `metadata_projects`
- `metadata_snapshots`
- `metadata_device_project_cursors`

Moving to Postgres should replace the `MetadataStore` implementation, not the API models or cursor
compare-and-set contract.

## Deferred

Remaining Phase 1 work includes:

- production sign-in and account ownership proof
- managed R2/S3 credential provisioning and rotation
- production pairing UX, recovery, and rotation
- wiring sync publish/import/materialize flows to the hosted service
- automatic conflict merge/apply resolution and user-facing conflict UI
- Electron tray/status integration
- production deployment hardening, observability, and abuse protection
