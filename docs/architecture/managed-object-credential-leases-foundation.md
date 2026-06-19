# Managed Object Credential Lease Foundation

This Phase 1 slice adds a production-shaped metadata foundation for Devbox-managed object-storage
credential leases and account-session-scoped object-access grants without making live Cloudflare R2,
AWS S3, or MinIO provisioning calls.

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
- server-mediated object-access grant resolution through a presented account session token
- canonical shared-bucket prefix derivation: `accounts/<account-id>/projects/<project-id>`

Project-scoped leases cannot be looked up from another account or project. Account session
resolution runs before active lease lookup, so future sync commands can resolve the authenticated
account boundary without trusting caller-supplied account ids.

Object-access resolution is account-session only. Mock-dev headers are refused for this path. The
resolver checks that the project exists inside the authenticated account, the lease is active,
required capabilities are present, and any stored lease prefix exactly matches the derived
account/project prefix. It rejects wildcard project scope, parent traversal, and project-escape
prefixes.

The metadata server fails closed unless server-managed object credentials are configured in its
environment. The grant response returns provider shape, capabilities, expiration, rotation
generation, redacted credential reference, and the authorized prefix. It does not return raw access
keys, secret keys, session tokens, Cloudflare API tokens, or credential hashes.

SQLite dev/test storage adds `metadata_managed_object_credential_leases`. The schema stores only
provider configuration, redacted references, lifecycle metadata, and capability names.

## CLI Smoke Surface

The CLI adds no-network mock/dev commands:

```text
devbox metadata credential-lease mock-create ...
devbox metadata credential-lease check ...
devbox metadata credential-lease rotate ...
devbox metadata credential-lease revoke ...
devbox metadata object-access resolve ...
```

`mock-create` bootstraps a dev verified account/session in the metadata DB when needed, upserts the
optional project scope, and writes a mock managed lease. `check` resolves the lease through the
presented session token and rejects expired, revoked, cross-scope, or under-capable leases.

Output prints only lease ids, provider kind, endpoint host, bucket, region, prefix, capabilities,
generation, expiry/revocation state, and redacted credential references. It does not print raw
session tokens, object credentials, provider API tokens, fingerprints, or hashes.

`object-access resolve` calls the hosted API with `Authorization: Bearer <session-token>` loaded
from `DEVBOX_SESSION_TOKEN` by default. It prints the authorized shared-bucket prefix and states that
client object credentials are not returned. Direct S3-compatible CLI flags remain suitable only for
trusted operator smoke tests with locally supplied env credentials; external testers should use the
server-mediated object path as it is wired into sync transport.

## Sync Integration Boundary

The existing local filesystem and S3-compatible object providers remain unchanged. S3-compatible
remotes can still be configured with environment variable names. The new grant model gives future
sync commands a safe hosted authorization boundary for one shared bucket: authenticate the account
session, resolve the project prefix, then use a server-mediated object proxy or signed URL path for
encrypted object bytes. It intentionally does not pretend that Cloudflare R2 can mint arbitrary
per-prefix end-user temporary credentials for us.

## Deferred

Remaining Phase 1 work still includes:

- live OAuth/OIDC sign-in and hosted account proof verification
- hosted object proxy or signed URL data transfer through the grant
- live Cloudflare/AWS/MinIO credential provisioning APIs
- production deployment hardening and abuse protection
- production pairing/recovery UX and live credential recovery flows
- Electron onboarding/status UI
- billing and storage limits
- automatic conflict merge/apply resolution
- user-facing conflict UI
