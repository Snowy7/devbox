# Production Pairing Recovery Rotation Foundation

This Phase 1 slice adds production-shaped pairing recovery and device rotation primitives without
building Electron UI, live OAuth login, provider-backed recovery, or hosted deployment hardening.

## Scope

`crates/devbox-auth` now models deterministic lifecycle state for:

- recovery grant references with account/device scope
- grant expiry, consumption, revocation, and audit labels
- device rotation intents with optional account-session linkage
- key-envelope rotation generation

`crates/devbox-store` persists those primitives in local SQLite schema version `9`:

- `recovery_grants`
- `device_rotation_intents`
- `key_envelopes.rotation_generation`

Recovery grants store redacted references such as `recovery-ref:...` or `grant-ref:...`. They do not
store recovery-code plaintext, pairing secrets, device keys, account sync keys, bearer tokens, token
hashes, or object credentials. Key-envelope rotation rewrites the encrypted envelope for an approved
device and increments the generation while keeping plaintext keys local and unprinted.

## CLI Smoke Surface

The CLI exposes no-network dev commands:

```text
devbox devices recovery create \
  --db <DB_PATH> \
  --device <DEVICE_ID> \
  --recovery-ref <REDACTED_REF>

devbox devices recovery revoke --db <DB_PATH> <GRANT_ID>
devbox devices rotate-key-envelope --db <DB_PATH> --device <DEVICE_ID>
```

Output may show account/device ids, grant ids, redacted references, statuses, expiry/revocation
timestamps, rotation intent ids, and key-envelope generations. It must not print raw recovery
secrets, pairing secrets, device keys, account sync keys, session tokens, token hashes, object
credentials, or key-envelope plaintext.

## Deferred

Remaining Phase 1 work still includes:

- live OAuth/OIDC sign-in and hosted account ownership proof verification
- Electron pairing/recovery UX
- live recovery-secret exchange and production device approval UX
- live managed Cloudflare/AWS/MinIO credential provisioning
- production deployment hardening and abuse protection
- automatic conflict merge/apply resolution and conflict UI
