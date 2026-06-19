# Production Auth Account Boundary Foundation

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Devbox.

This Phase 1 slice adds a production-shaped account ownership proof and account session boundary
without wiring a live OAuth/OIDC provider or browser login flow.

## Scope

`crates/devbox-auth` now owns provider-agnostic proof and session semantics:

- external provider kind, issuer, and subject
- verified email and/or verified domain
- Devbox account id
- session id
- session token hash
- proof/session expiration
- session revocation

The domain model validates that ownership proofs have verified account material, are not expired,
and do not carry obvious provider-secret material in public provider fields. Session validation
checks token hash equality, expiration, and revocation state.

Raw bearer tokens, refresh tokens, OAuth codes, provider secrets, and object credentials are not
stored. The only persisted session credential material is a hash of the presented session token.

## Local SQLite Boundary

Local schema version `8` adds:

- `account_ownership_proofs`
- `account_sessions`

These tables are separate from the existing local/mock `auth_sessions` table. Local/mock login,
pairing invitations, trusted devices, key envelopes, revocation markers, and device/project cursors
continue to work as before.

The local store supports:

- ownership proof upsert
- account lookup by provider kind/issuer/subject
- session upsert
- session lookup by id
- session lookup by presented-token hash
- session revocation

## Hosted Metadata Boundary

`crates/devbox-metadata` now has matching SQLite and in-memory store primitives for:

- verified account proof upsert
- provider-subject account lookup
- account session upsert
- account session lookup by id or hash
- account session revocation
- resolving a presented session token into an authenticated account/session context
- resolving active managed object credential leases and object-access grants inside the
  authenticated account/session scope

Hosted metadata HTTP handlers now support two explicit modes. Local tests/dev can still use the
local-only mock-dev headers:

```text
x-devbox-mock-account-id
x-devbox-mock-device-id
```

Those headers remain available for tests and local development. The account-session resolver is the
production-shaped path beyond mock headers.

Production-shaped session mode accepts:

```text
Authorization: Bearer <session-token>
```

The service hashes the presented token transiently, resolves it through the hosted account session
store, rejects expired or revoked sessions, and scopes devices, projects, snapshots, cursors, and
managed object credential leases to the authenticated account id. In this mode handlers do not trust
account ids supplied in JSON request bodies. Public HTTP errors are sanitized and do not reflect raw
session tokens, token hashes, provider material, object credentials, SQLite internals, or key
material.

Hosted object-access grants are stricter than the general mock-dev metadata handlers. They require
bearer account-session auth, derive the shared-bucket prefix from the authenticated account and
requested project, and do not return raw object credentials to the client.

## CLI Smoke Surface

The CLI surface is intentionally no-network and dev/bootstrap-only:

```text
devbox auth mock-verified-bootstrap \
  --db <DB_PATH> \
  --verified-email <EMAIL> \
  --session-token <TOKEN>

devbox auth proof-check --db <DB_PATH> --session-token <TOKEN>
devbox auth revoke-session --db <DB_PATH> <SESSION_ID>
```

Output prints account/session ids and proof metadata, but it does not print raw session tokens,
session token hashes, provider secrets, OAuth credentials, or object credentials.

## Deferred

This is not production sign-in UI. Remaining Phase 1 work still includes:

- live OAuth/OIDC provider integration
- hosted login/callback handling
- production pairing UX and live recovery/rotation flows
- live managed object-storage credential provisioning and rotation against Cloudflare/AWS APIs
- multi-region deployment hardening and abuse protection
- Electron onboarding/status UI
- automatic conflict merge/apply resolution and conflict UI
