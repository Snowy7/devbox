# Managed Object Credential Lease Foundation

This Phase 1 slice adds a production-shaped metadata foundation for Devbox-managed object-storage
credential leases without making live Cloudflare R2, AWS S3, or MinIO provisioning calls.

## Scope

`crates/devbox-metadata` now models account/session-scoped managed object credential leases for
future encrypted blob sync:

- provider kind: Cloudflare R2, AWS S3, or MinIO-compatible
- endpoint, bucket, region, and optional object prefix namespace
- account scope and optional project scope
- managed lease id and redacted credential reference
- opaque non-secret fingerprint reference for future provider correlation
- capabilities: read, write, list, and head
- issuance, expiration, revocation, and rotation generation

The lease model is provider-agnostic and S3-compatible in shape. It does not store raw access keys,
secret keys, session tokens, provider API tokens, OAuth tokens, or raw credential hashes.

## Store Semantics

The hosted metadata store exposes deterministic in-memory and SQLite-backed lease primitives:

- create/upsert lease
- scoped lookup by account, optional project, and lease id
- active lookup through a presented account session token
- capability validation for active use
- expiration and revocation rejection for active use
- rotation generation increment with a new redacted reference
- revocation marking

Project-scoped leases cannot be looked up from another account or project. Account session
resolution runs before active lease lookup, so future sync commands can resolve the authenticated
account boundary without trusting caller-supplied account ids.

SQLite dev/test storage adds `metadata_managed_object_credential_leases`. The schema stores only
provider configuration, redacted references, lifecycle metadata, and capability names.

## CLI Smoke Surface

The CLI adds no-network mock/dev commands:

```text
devbox metadata credential-lease mock-create ...
devbox metadata credential-lease check ...
devbox metadata credential-lease rotate ...
devbox metadata credential-lease revoke ...
```

`mock-create` bootstraps a dev verified account/session in the metadata DB when needed, upserts the
optional project scope, and writes a mock managed lease. `check` resolves the lease through the
presented session token and rejects expired, revoked, cross-scope, or under-capable leases.

Output prints only lease ids, provider kind, endpoint host, bucket, region, prefix, capabilities,
generation, expiry/revocation state, and redacted credential references. It does not print raw
session tokens, object credentials, provider API tokens, fingerprints, or hashes.

## Sync Integration Boundary

The existing local filesystem and S3-compatible object providers remain unchanged. S3-compatible
remotes can still be configured with environment variable names. The new lease model adds a clean
path for future sync commands to resolve a managed credential lease into a redacted remote config
before live provider credentials are fetched from a hosted provisioning service.

## Deferred

Remaining Phase 1 work still includes:

- live OAuth/OIDC sign-in and hosted account proof verification
- live Cloudflare/AWS/MinIO credential provisioning APIs
- production deployment hardening and abuse protection
- production pairing/recovery UX and live credential recovery flows
- Electron onboarding/status UI
- billing and storage limits
- automatic conflict merge/apply resolution
- user-facing conflict UI
