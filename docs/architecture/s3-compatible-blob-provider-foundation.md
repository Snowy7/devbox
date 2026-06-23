# S3-Compatible Blob Provider Foundation

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Bindhub.

This Phase 1 slice adds a production-shaped encrypted object remote for the existing sync pipeline.
`bindhub-sync` can now target S3-compatible object storage such as Cloudflare R2, AWS S3, or MinIO
behind the same `RemoteBlobProvider` boundary used by the local filesystem provider.

## Boundary

This is object transport only.

It does not add production sign-in, managed R2 credential provisioning, production pairing UX,
Electron UI, or conflict resolution. Clients still need a trusted local identity and key material
before encrypted objects can be published or read. A hosted metadata API foundation now models
metadata discovery and server-side compare-and-set cursors, and a production-shaped account/session
boundary now models token-hash account sessions without live OAuth. A managed object credential
lease and object-access grant foundation now models redacted R2/S3/MinIO-shaped provider
references, account/session/project scope, canonical shared-bucket prefixes, capabilities,
expiration, revocation, and rotation generation. This direct S3-compatible object provider still
does not make live Cloudflare/AWS provisioning calls or load raw managed credentials from the hosted
service.

## Provider Model

The S3-compatible provider implements the existing remote blob contract:

- `put`
- `get`
- `head`
- immutable object keys
- idempotent same-byte uploads
- collision refusal for different bytes at an existing key

Objects are addressed with the same safe relative `ObjectKey` model used by local sync. An optional
remote prefix namespaces Bindhub objects inside a bucket. Prefixes and object keys reject parent
traversal, absolute paths, empty path segments, Windows separators, and accidental double slashes.

The provider uses path-style URLs:

```text
<endpoint>/<bucket>/<optional-prefix>/<object-key>
```

That keeps Cloudflare R2, AWS S3, and MinIO-compatible local endpoints behind one configuration
shape.

## Credentials

The CLI accepts credential environment variable names, not raw secret values:

```text
--s3-access-key-env BINDHUB_R2_ACCESS_KEY_ID
--s3-secret-key-env BINDHUB_R2_SECRET_ACCESS_KEY
--s3-session-token-env BINDHUB_R2_SESSION_TOKEN
```

If explicit env names are omitted, the provider uses the standard AWS-compatible environment names:

- `AWS_ACCESS_KEY_ID`
- `AWS_SECRET_ACCESS_KEY`
- `AWS_SESSION_TOKEN`

Config/debug/status output is redacted and prints env variable names only. Missing credentials report
which env var is missing without printing secret material.

## Upload Semantics

`upload_blob_from_cache` still decrypts existing remote objects before treating an upload as
idempotent. Because encrypted object writes use random nonces, byte-for-byte ciphertext equality is
not required for same-plaintext idempotency at the sync layer.

The S3 provider also uses conditional create semantics for `put` so a concurrent writer cannot
silently replace an object between existence check and upload. If the object appears first, the
provider re-reads it and either returns idempotent success for identical encrypted bytes or refuses
the immutable-object collision.

## CLI Surface

Local filesystem remotes remain the default and keep the existing shape:

```text
bindhub sync publish-snapshot --db <DB> --cache <CACHE> --remote <REMOTE_DIR> <SNAPSHOT_ID>
```

S3-compatible remotes opt in with `--remote-kind s3`:

```text
bindhub sync publish-snapshot \
  --db <DB> \
  --cache <CACHE> \
  --remote-kind s3 \
  --s3-endpoint https://<account>.r2.cloudflarestorage.com \
  --s3-bucket bindhub-alpha \
  --s3-region auto \
  --s3-prefix accounts/<account-id>/projects \
  --s3-access-key-env BINDHUB_R2_ACCESS_KEY_ID \
  --s3-secret-key-env BINDHUB_R2_SECRET_ACCESS_KEY \
  <SNAPSHOT_ID>
```

`bindhub sync remote check --validate-only` validates and prints redacted configuration without a
network request. Without `--validate-only`, it loads credentials and performs a lightweight `head`
probe against the remote.

Hosted metadata sync wiring is separate from object transport. Sync commands can opt into
`--metadata-mode mock-dev-sqlite --metadata-db <METADATA_DB>` so publish registers manifest metadata
and import/materialize discover the manifest object key through metadata before reading from the
configured local or S3-compatible object provider.

For a multi-user shared bucket, the production-shaped authorization path is
`bindhub metadata object-access resolve`, which returns the allowed
`accounts/<account-id>/projects/<project-id>` prefix through a server-mediated broker boundary.
Supplying `--s3-access-key-env` and `--s3-secret-key-env` directly remains a trusted-operator smoke
path, not the external-tester permission boundary.

## Deferred

Remaining Phase 1 work includes:

- live OAuth/OIDC sign-in and hosted account ownership proof verification
- live managed R2/S3 credential provisioning and rotation against provider APIs
- production pairing UX and recovery flows
- conflict UI and automatic merge/apply resolution
- Electron tray/status integration
