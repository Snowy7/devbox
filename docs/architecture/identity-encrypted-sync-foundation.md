# Local Identity and Encrypted Object Sync Foundation

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Devbox.

This slice introduces the first Phase 1 sync foundation after the local watcher daemon:

- local account and current-device identity in SQLite
- generated local key material for encrypted object transport
- immutable remote blob provider interface
- local filesystem remote provider for tests and manual smoke
- S3-compatible remote provider for R2, AWS S3, and MinIO-style object storage
- encrypted upload/download of existing local blob-cache objects

## Boundary

This PR does not implement cloud authentication.

It does not provide sign-in, account ownership proof, backend auth, device approval, device
revocation, recovery, pairing UX, server-side project cursors, or a hosted metadata service. A local
identity row means this installation can encrypt and address Devbox sync objects; it does not mean
the user is authenticated to any cloud service.

The auth and pairing foundation slice adds local/mock primitives for:

- local/mock account session state
- device approval and trust establishment
- encrypted key envelopes for approved devices
- revocation markers
- local device/project cursors

Production-shaped account/session proof primitives and server-side device/project cursors were added
in later foundations. The private-alpha Electron shell now covers local status and safety visibility.
Live sign-in, hosted OAuth/OIDC verification, recovery, and production second-device materialization
UX remain deferred.

## Local Identity Model

`devbox init --db <DB_PATH>` migrates the local SQLite database and creates one local account plus
one current local device if they do not already exist.

Repeated init is idempotent. It returns the existing account/device identifiers and does not mint new
keys on every run. The CLI prints local identifiers and status, but it never prints raw key material.

The schema is multi-device-ready even though this PR only initializes the current local device. The
`local_devices` table can hold many known devices for an account. A partial unique index allows only
one `is_local = 1` row in one installation, while allowing any number of non-local known/trusted
device rows. Store tests explicitly insert multiple non-local devices for one account and verify
`list_devices` returns all devices while only one is marked current local.

## Encrypted Object Transport

`devbox/crates/devbox-sync` defines a small remote object provider boundary:

- `put`
- `get`
- `head`
- immutable object keys
- idempotent same-byte uploads

The local filesystem provider stores objects under a provider-owned `objects/` directory and rejects
unsafe object keys such as absolute paths, parent traversal, empty path segments, and Windows
separator escapes. It remains the default no-network provider for tests and local smoke runs.

The S3-compatible provider now uses the same object-key contract with optional safe prefixes,
path-style endpoint/bucket URLs, AWS Signature V4, and credential values loaded only from
environment variables. CLI output prints endpoint, bucket, prefix, and env variable names, but not
raw access keys, secret keys, or session tokens.

Payload encryption uses XChaCha20-Poly1305 with a random 24-byte nonce per object write. The object
key is authenticated as associated data. Remote provider bytes are an encrypted envelope containing
a version marker, nonce, and ciphertext; plaintext blob bytes are never written to the remote
provider by the sync crate.

## CLI Smoke Path

The manual local path is intentionally narrow:

```text
devbox init --db <DB_PATH> [--device-name <NAME>]
devbox devices list --db <DB_PATH>
devbox sync upload --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <BLOB_ID>
devbox sync download --db <DB_PATH> --cache <CACHE_ROOT> --remote <REMOTE_DIR> <BLOB_ID>
```

Upload reads plaintext from the local content-addressed blob cache, encrypts it, and writes the
encrypted object to the remote provider. Download reads the encrypted remote object, decrypts it,
verifies the expected BLAKE3 blob id, and writes the plaintext back into a local blob cache.

Download targets the blob cache, not a project directory. Second-device project materialization now
has a local/mock foundation in `devbox-materialize`; production key exchange, live hosted auth
enforcement, managed cloud credential provisioning, automatic conflict resolution, and restore UI
flows remain deferred.
