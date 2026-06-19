# Hosted Metadata Sync Flow Wiring

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Devbox.

This Phase 1 slice wires the existing local/mock second-device sync flows to the hosted metadata
foundation without requiring a network service in normal development or CI.

## Scope

`crates/devbox-materialize` now exposes a small `HostedMetadataClient` boundary implemented by any
`devbox-metadata::MetadataStore` and by an account-session HTTP client. The local deterministic
opt-in mode uses `SqliteMetadataStore` in-process:

```text
--metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>
```

Import and materialize also require:

```text
--metadata-project <PROJECT_ID>
--metadata-account <ACCOUNT_ID>  # or legacy-derive from --mock-key-source-db <PUBLISHER_DB>
```

That account/project scope lets the receiver discover the manifest object key from mock-dev metadata
by `(account_id, project_id, snapshot_id)` before decrypting the encrypted bundle. In the current
mock-dev CLI path, `--mock-key-source-db <PUBLISHER_DB>` can provide the publisher account id for
the same legacy local trust bootstrap that provides the decryption key. The paired-receiver alpha
path should omit `--mock-key-source-db`: after `devices join -> approve-join -> complete`, the
receiver DB already has a local device-key envelope created from the token-wrapped completion. The
default path remains local/mock only and still derives the manifest object key locally.

External hosted alpha sync uses the live API instead:

```text
--metadata-mode hosted-api --metadata-api <URL> --metadata-project <PROJECT_ID>
--metadata-session-token-env DEVBOX_SESSION_TOKEN
```

Hosted API mode calls the account-session routes for snapshot registration, latest discovery, and
cursor compare-and-set. It does not accept `--metadata-account`; the server derives account scope
from the authenticated bearer session and overwrites any client payload account id. Managed object
credential lease records and object-access grants provide the companion hosted path for resolving an
authenticated account/session/project prefix inside one shared R2/S3/MinIO-compatible bucket. Sync
object bytes can use `--remote-kind hosted`, which proxies encrypted object put/get/head/list
through the metadata API without returning raw bucket credentials to the client.

The live daemon adds latest-snapshot discovery for the same account/project scope. Receivers can
omit a pasted snapshot id and let `devbox-daemon sync --pull` resolve the latest published metadata
record before import or materialization.

## Publish Semantics

After encrypted blob objects and the encrypted snapshot bundle are written, metadata-enabled publish:

- upserts the local mock-dev device
- upserts the project
- records project-scoped published snapshot metadata, including the manifest object key, manifest
  hash, entry count, total size, publisher device, and publish timestamp

Snapshot metadata stores object references and counts only. It does not store plaintext source bytes,
raw sync keys, device keys, R2 secrets, object credentials, or manifest contents.

## Import and Materialize Semantics

Metadata-enabled import/materialize:

- upserts the receiver device under the mock-dev store or authenticated hosted session
- looks up the published snapshot metadata by hosted account, project, and snapshot id
- or, in daemon live pull mode, first resolves the latest published snapshot for the hosted
  account/project
- downloads/decrypts the manifest from the hosted metadata object key
- keeps local preflight conflict refusal before blob download/materialization
- advances the hosted device/project cursor with compare-and-set before writing the local cursor

If hosted compare-and-set returns a stale cursor conflict, the local cursor is not advanced. This
keeps a receiver from blindly overwriting newer server-side cursor state.

The hosted API cursor uses the authenticated session account scope and the receiving device id. The
local cursor remains stored under the receiver DB's local account/device ids. For paired receivers,
that local account id is the publisher account id because pairing completion installs the receiver
device inside that account boundary.

## CLI Boundary

`devbox metadata check` remains a no-network validation command. Sync commands can optionally accept
`--metadata-endpoint <URL>` as a sanitized label for the mock-dev metadata boundary. Hosted alpha
sync uses `--metadata-api <URL>` with account-session bearer auth and performs live endpoint calls.

Output states whether hosted mock-dev metadata wiring is active or whether the command is using
local/mock metadata only. It does not print raw mock header values, raw keys, object credentials, or
unsafe endpoint material.

`devbox metadata object-access resolve` is the hosted/API-side counterpart. It uses
`DEVBOX_SESSION_TOKEN` by default, calls the metadata API, and returns a redacted server-mediated
grant for `accounts/<account-id>/projects/<project-id>`. `--remote-kind hosted` consumes the same
API/session/lease boundary for encrypted object transfer. Direct S3-compatible sync flags remain a
trusted-operator smoke path.

`devbox-daemon sync --remote-kind s3` is stricter than the manual CLI smoke path: it requires an
object-access API, lease id, session token environment variable name, and an `--s3-prefix` that
matches the grant. The grant is used as an authorization/prefix preflight; raw object credentials
are still loaded only from environment variable names for the current alpha transport.

`devbox-daemon sync --remote-kind hosted` requires an object-access API, lease id, session token
environment variable name, `--metadata-mode hosted-api`, `--metadata-api`, and
`--metadata-project` for external alpha use. The daemon resolves the grant at startup and the hosted
object provider enforces read/write/head/list capabilities, lease activity, account/project scope,
and object-key safety on every transfer. Local `mock-dev-sqlite` mode remains available for
deterministic no-network smoke tests.

## Deferred

This is personal-alpha wiring, not a production SaaS backend. Deferred work remains:

- live OAuth/OIDC sign-in and hosted account ownership proof verification
- live managed object credential provisioning and rotation against provider APIs
- multi-region deployment hardening and observability
- Electron tray/status UI
- automatic conflict merge/apply resolution
- user-facing conflict UI
