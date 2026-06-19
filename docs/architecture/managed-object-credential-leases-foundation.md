# Managed Object Credential Lease Foundation

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Devbox.

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
optional project scope, and writes a mock managed lease. When the supplied session token already has
an account session, the lease is seeded under that authenticated session account; an explicit
`--account` must match it. For the Postgres admin selector, the session must already exist and
authenticate successfully, so the command cannot create or relink mock identity in the hosted store.
`check` resolves the lease through the presented session token and rejects expired, revoked,
cross-scope, or under-capable leases.

Output prints only lease ids, provider kind, endpoint host, bucket, region, prefix, capabilities,
generation, expiry/revocation state, and redacted credential references. It does not print raw
session tokens, object credentials, provider API tokens, fingerprints, or hashes.

`object-access resolve` calls the hosted API with `Authorization: Bearer <session-token>` loaded
from `DEVBOX_SESSION_TOKEN` by default. It prints the authorized shared-bucket prefix and states that
client object credentials are not returned. External testers use `--remote-kind hosted` to transfer
encrypted bytes through the server-mediated API; direct S3-compatible CLI flags remain suitable only
for trusted operator smoke tests with locally supplied env credentials.

`devbox-daemon sync --remote-kind s3` now consumes the same object-access grant as a live alpha
preflight. The daemon refuses shared-bucket live sync unless the hosted API/lease/session-token-env
are configured and the returned grant matches the requested bucket, region, account/project scope,
and prefix. The grant still does not return client credentials; the direct S3 provider loads only
environment variable names for the current transport.

`devbox-daemon sync --remote-kind hosted` consumes the same grant for encrypted object transfer. The
metadata API opens server-side object storage, applies the account/project prefix, and enforces
read/write/head/list capabilities plus lease expiration, revocation, and rotation generation on each
object operation.

## Sync Integration Boundary

The existing local filesystem and S3-compatible object providers remain unchanged. S3-compatible
remotes can still be configured with environment variable names. The grant model now gives live
daemon sync two safe shared-bucket modes: hosted transfer for external testers with no client bucket
keys, and trusted direct-S3 smoke for operators who intentionally keep bucket keys on their own
machine. This intentionally does not pretend that Cloudflare R2 can mint arbitrary per-prefix
end-user temporary credentials for us.

## Deferred

Remaining Phase 1 work still includes:

- live OAuth/OIDC sign-in and hosted account proof verification
- live Cloudflare/AWS/MinIO credential provisioning APIs
- multi-region deployment hardening and abuse protection
- production pairing/recovery UX and live credential recovery flows
- Electron onboarding/status UI
- billing and storage limits
- automatic conflict merge/apply resolution
- user-facing conflict UI
