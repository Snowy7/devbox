# Auth and Device Pairing Foundation

This slice adds the next Phase 1 trust foundation after local identity and encrypted object sync:

- local/mock account ownership session state
- pairing invitation creation and approval
- approved-device trust records
- encrypted account key envelopes for approved devices
- revocation markers
- device/project cursor records
- scriptable CLI smoke commands

## Boundary

This is not production authentication.

The implementation uses the existing local SQLite metadata store and a local/mock provider boundary.
It does not add OAuth, hosted sign-in, account billing identity, a deployed metadata service,
production recovery flows, real R2/S3 credentials, or second-device project materialization.

A mock auth session proves that local account ownership state can be represented and queried. It is
not a cloud session and must not be treated as durable proof outside the local metadata boundary.

## Domain Boundary

`crates/devbox-auth` owns deterministic auth and device-pairing state transitions:

- create and parse pairing invitation tokens
- reject malformed, expired, reused, or account-mismatched invitations
- approve many devices for one account
- create encrypted key envelopes for approved devices
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
CLI. Pairing tokens are intentionally printed for the manual mock approval path; they are not a
production pairing UX.

Schema version `6` adds a unique invitation claim index so a pairing invitation can approve at most
one trusted device. `Store::persist_pairing_approval` also claims invitations with
`WHERE status = 'pending'` inside the approval transaction.

## CLI Smoke Path

The manual path is:

```text
devbox init --db <DB_PATH> [--device-name <NAME>]
devbox auth mock-login --db <DB_PATH>
devbox auth status --db <DB_PATH>
devbox devices invite --db <DB_PATH> [--ttl-seconds <SECONDS>]
devbox devices approve --db <DB_PATH> --token <TOKEN> --device-name <NAME>
devbox devices list --db <DB_PATH>
devbox devices revoke --db <DB_PATH> <DEVICE_ID> [--reason <TEXT>]
devbox sync cursor set --db <DB_PATH> --project <PROJECT_ID> --value <CURSOR>
devbox sync cursor get --db <DB_PATH> --project <PROJECT_ID>
```

Existing commands such as `devbox init`, `devbox devices list`, `devbox sync upload/download`,
`devbox snapshot`, and `devbox snapshot restore` continue to work.

## Deferred

Remaining Phase 1 work includes:

- production account ownership proof
- hosted metadata API and server-side cursors
- real cloud object storage credentials
- production pairing UX and recovery
- second-device project materialization
- conflict-as-divergent-snapshot flows
