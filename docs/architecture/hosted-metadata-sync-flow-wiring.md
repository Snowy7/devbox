# Hosted Metadata Sync Flow Wiring

This Phase 1 slice wires the existing local/mock second-device sync flows to the hosted metadata
foundation without requiring a network service in normal development or CI.

## Scope

`crates/devbox-materialize` now exposes a small `HostedMetadataClient` boundary implemented by any
`devbox-metadata::MetadataStore`. The current CLI opt-in mode uses `SqliteMetadataStore` in-process:

```text
--metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>
```

Import and materialize also require:

```text
--metadata-project <PROJECT_ID>
--metadata-account <ACCOUNT_ID>  # or legacy-derive from --mock-key-source-db <PUBLISHER_DB>
```

That account/project scope lets the receiver discover the manifest object key from hosted metadata
by `(account_id, project_id, snapshot_id)` before decrypting the encrypted bundle. In the current
mock-dev CLI path, `--mock-key-source-db <PUBLISHER_DB>` can provide the publisher account id for
the same legacy local trust bootstrap that provides the decryption key. The paired-receiver alpha
path should omit `--mock-key-source-db`: after `devices join -> approve-join -> complete`, the
receiver DB already has a local device-key envelope created from the token-wrapped completion. The
default path remains local/mock only and still derives the manifest object key locally. Hosted
metadata handlers now support production-shaped account-session bearer auth, but these local sync
commands still use the in-process mock-dev SQLite metadata mode and do not perform live
OAuth-backed network auth. Managed object credential lease records and object-access grants now
provide a hosted path for resolving an authenticated account/session/project prefix inside one shared
R2/S3/MinIO-compatible bucket, but sync commands do not yet transfer object bytes through the hosted
grant.

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

- upserts the receiver mock-dev device
- looks up the published snapshot metadata by hosted account, project, and snapshot id
- downloads/decrypts the manifest from the hosted metadata object key
- keeps local preflight conflict refusal before blob download/materialization
- advances the hosted device/project cursor with compare-and-set before writing the local cursor

If hosted compare-and-set returns a stale cursor conflict, the local cursor is not advanced. This
keeps a receiver from blindly overwriting newer server-side cursor state.

The hosted cursor uses the hosted/publisher account scope and the receiving device id. The local
cursor remains stored under the receiver DB's local account/device ids. For paired receivers, that
local account id is the publisher account id because pairing completion installs the receiver device
inside that account boundary.

## CLI Boundary

`devbox metadata check` remains a no-network validation command. Sync commands can optionally accept
`--metadata-endpoint <URL>` as a sanitized label for the mock-dev metadata boundary, but sync metadata
mode remains in-process for this slice and does not perform a live endpoint call.

Output states whether hosted mock-dev metadata wiring is active or whether the command is using
local/mock metadata only. It does not print raw mock header values, raw keys, object credentials, or
unsafe endpoint material.

`devbox metadata object-access resolve` is the hosted/API-side counterpart. It uses
`DEVBOX_SESSION_TOKEN` by default, calls the metadata API, and returns a redacted server-mediated
grant for `accounts/<account-id>/projects/<project-id>`. Direct S3-compatible sync flags remain a
trusted-operator smoke path until sync has a hosted object proxy or signed URL transport.

## Deferred

This is personal-alpha wiring, not a production SaaS backend. Deferred work remains:

- live OAuth/OIDC sign-in and hosted account ownership proof verification
- hosted object proxy or signed URL transport for encrypted object bytes
- live managed object credential provisioning and rotation against provider APIs
- production deployment hardening and observability
- Electron tray/status UI
- automatic conflict merge/apply resolution
- user-facing conflict UI
