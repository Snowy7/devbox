# Production Auth Account Boundary Foundation

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

Existing hosted metadata HTTP handlers still require the local-only mock-dev headers:

```text
x-devbox-mock-account-id
x-devbox-mock-device-id
```

Those headers remain available for tests and local development. Future production handlers can use
the new account-session resolver instead of trusting mock headers.

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
- production pairing UX, recovery, and rotation
- managed object-storage credential provisioning and rotation
- production deployment hardening and abuse protection
- Electron onboarding/status UI
- automatic conflict merge/apply resolution and conflict UI
