# Auth and Device Pairing Foundation

This slice adds the next Phase 1 trust foundation after local identity and encrypted object sync:

- local/mock account ownership session state
- pairing invitation creation and approval
- receiver-generated pairing join and completion payloads
- approved-device trust records
- encrypted account key envelopes for approved devices
- revocation markers
- production-shaped recovery grant references
- device rotation intents and key-envelope rotation generation
- device/project cursor records
- scriptable CLI smoke commands

## Boundary

This is not production authentication.

The implementation uses the existing local SQLite metadata store and a local/mock provider boundary.
It does not add OAuth, hosted sign-in, account billing identity, production recovery flows, live
R2/S3 credential provisioning, or second-device project materialization. A later hosted metadata API
foundation now models server-side device/project cursors separately, but this local/mock auth
boundary remains non-production. A later production-shaped account proof/session foundation now
models provider subject/email/domain proof, token-hash sessions, expiration, and revocation, but it
still does not add live OAuth login. A later service-trust slice now adds account-session hosted
metadata request auth plus no-network production-shaped recovery/rotation primitives, but it still
does not add Electron pairing UX, live provider recovery, or live OAuth.

A mock auth session proves that local account ownership state can be represented and queried. It is
not a cloud session and must not be treated as durable proof outside the local metadata boundary.

## Domain Boundary

`crates/devbox-auth` owns deterministic auth and device-pairing state transitions:

- create and parse pairing invitation tokens
- reject malformed, expired, reused, or account-mismatched invitations
- approve many devices for one account
- create receiver-generated join requests and completion payloads
- create encrypted key envelopes for approved devices
- create redacted recovery grant references with expiry and revocation
- create/complete/revoke device rotation intents
- rotate key envelopes by generation without printing key plaintext
- reject repeated revocation
- model local/mock auth sessions and device/project cursors

The crate deliberately does not open SQLite, talk to a network service, or print secrets.

## Local Metadata

Schema versions `5` and `6` add local-only auth/device-pairing state:

- `auth_sessions`
- `pairing_invitations`
- `trusted_devices`
- `key_envelopes`
- `revocation_markers`
- `device_project_cursors`

These rows let the CLI and future daemon code exercise trust-state semantics without a hosted
backend. Raw local account/device key material remains local SQLite state and is never printed by the
CLI. Pairing tokens, join requests, and completion payloads are secret-bearing manual alpha handoff
values; prefer the `--token-env`, `--join-request-env`, and `--completion-env` commands so they do
not land in shell history. They are not a production pairing UX.

Schema version `6` adds a unique invitation claim index so a pairing invitation can approve at most
one trusted device. `Store::persist_pairing_approval` also claims invitations with
`WHERE status = 'pending'` inside the approval transaction.

Schema version `9` adds recovery grants, device rotation intents, and key-envelope rotation
generation. Recovery grants store redacted references only, not recovery code plaintext. Rotation
updates the encrypted key envelope for an approved device and records the completed generation.
Recovery consumption is pending-only, consumed grants reject later revoke/consume attempts, and
rotation completion claims a persisted pending intent by expiry and key-envelope generation.

## CLI Smoke Path

The manual path is:

```text
devbox init --db <DB_PATH> [--device-name <NAME>]
devbox auth mock-login --db <DB_PATH>
devbox auth status --db <DB_PATH>
devbox devices invite --db <DB_PATH> [--ttl-seconds <SECONDS>]
devbox devices approve --db <DB_PATH> --token <TOKEN> --device-name <NAME>
devbox devices join --db <RECEIVER_DB> --token-env <ENV> --device-name <NAME>
devbox devices approve-join --db <SOURCE_DB> --token-env <ENV> --join-request-env <ENV> --device-name <NAME>
devbox devices complete --db <RECEIVER_DB> --completion-env <ENV>
devbox devices list --db <DB_PATH>
devbox devices revoke --db <DB_PATH> <DEVICE_ID> [--reason <TEXT>]
devbox devices recovery create --db <DB_PATH> --device <DEVICE_ID> --recovery-ref <REDACTED_REF>
devbox devices recovery revoke --db <DB_PATH> <GRANT_ID>
devbox devices rotate-key-envelope --db <DB_PATH> --device <DEVICE_ID>
devbox sync cursor set --db <DB_PATH> --project <PROJECT_ID> --value <CURSOR>
devbox sync cursor get --db <DB_PATH> --project <PROJECT_ID>
```

`devices approve` is the older single-DB smoke path where the inviter generates the approved device
record directly. The `join -> approve-join -> complete` path is the current alpha
receiver-generated path: `devices join` can create a fresh receiver DB with pending local key state,
the source encrypts the account sync key into a token-wrapped completion payload without learning the
receiver local device key, and the receiver opens that completion and installs its own local
device-key envelope.

Existing commands such as `devbox init`, `devbox devices list`, `devbox sync upload/download`,
`devbox sync publish-snapshot/import-snapshot/materialize`, `devbox snapshot`, and
`devbox snapshot restore` continue to work. Local/mock import and materialize now reconcile the
receiving device/project cursor with the latest local and incoming snapshots before downloading file
blobs or applying workspace bytes. Divergent local and incoming snapshots create a local conflict
record and do not advance the cursor.

## Deferred

Remaining Phase 1 work includes:

- live OAuth/OIDC account ownership verification and hosted login UI
- live cloud object storage credential provisioning
- production pairing/recovery UI and live recovery-secret exchange
- production second-device project materialization UX
- automatic conflict merge/apply resolution and user-facing conflict flows
