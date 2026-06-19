# Secret Detection and Block Policy

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Historical terminology note: this architecture slice may use `project` for an implementation-scoped
shared folder. New product language should say shared folder. Loom is the codename for the deeper
source-control primitive underneath Devbox.

This Phase 1 slice adds local high-confidence secret blocking to snapshot and local/mock sync
foundations. It protects the future cloud path by preventing obvious secrets from becoming included
snapshot blobs by default.

## Boundary

This is a local-first safety boundary.

It does not implement:

- a full DLP classifier
- team policy administration
- live OAuth/OIDC cloud authentication
- hosted metadata enforcement
- managed R2/S3 credential provisioning or hosted object indexing
- encrypted personal/team secret envelope payload storage

Explicit alpha policy is path-scoped and deliberate. Detected secrets are still blocked by default
before blob-cache writes. Local policy records let the alpha UI and CLI distinguish:

- `block`: keep blocking a detected path and require user action.
- `template`: intentionally sync only template/example shape later; raw detected bytes are not made
  uploadable by this alpha record.
- `envelope`: point at an opaque encrypted envelope reference with the `secret-envelope-ref:` scheme;
  the reference is metadata, not envelope plaintext.

## Detector Rules

`devbox-core` owns the detector rules in `secrets`.

The initial rule set is intentionally conservative and high-confidence:

- AWS access key ids with `AKIA` or `ASIA` shape
- GitHub classic and fine-grained token prefixes
- OpenAI API key prefix
- Stripe secret key prefixes
- private-key PEM headers
- dotenv-style uppercase secret assignments with high-entropy values

The detector scans regular text-like files using a bounded prefix. Binary-looking content and bytes
past the initial scan bound are not classified by this first local detector. Placeholder suppression
is limited to dotenv-style high-entropy assignment checks so clear provider-token shapes still block
even in synthetic tests or examples.

## Snapshot Semantics

`devbox-snapshot` applies secret detection before writing file bytes to `BlobCache`.

If a regular file triggers a high-confidence rule:

- the file bytes are not written to the local blob cache
- the manifest entry has no blob id and no object ref
- the manifest entry is marked `RequiresUserDecision`
- the policy reason contains rule id, line number, and redacted evidence
- raw secret values are not printed, logged, or stored in manifest policy reasons

Generated directory exclusions such as `.git`, `node_modules`, `target`, `.venv`, and `.cache` still
run as directory-boundary policy and are not descended into.

## Change Feed and Sync Semantics

The local change feed only persists uploadable operations for included regular files with blob refs.
Blocked-secret files are counted as skipped/deferred and do not become pending created or modified
operations.

`devbox sync publish-snapshot` only uploads included file blobs from persisted snapshot metadata. A
blocked-secret entry has no blob ref, so it cannot publish a file blob or materialize as a restored
file on another local/mock device.

## CLI Surface

`devbox snapshot` prints:

- `Blocked secrets: <count>`
- `SECRET<TAB><path><TAB><redacted policy reason>` rows for blocked entries

`snapshot show`, restore, import, and materialize keep using manifest policy decisions, so blocked
entries remain visible as deferred/skipped policy entries without exposing raw secret values.

Explicit alpha policy records are managed with:

```text
devbox secrets policy add --db <DB_PATH> --project <PROJECT_ID> --path <REL_PATH> --action block|template|envelope [--envelope-ref <REF>] [--note <TEXT>]
devbox secrets policy list --db <DB_PATH> [--project <PROJECT_ID>]
```

The CLI and store reject secret-looking envelope references or notes. Envelope references must use
the `secret-envelope-ref:` opaque scheme, and store reads fail closed for unsafe legacy values rather
than returning them to list/detail output. Normal output prints only rule ids, project ids, paths,
actions, safe opaque envelope refs, notes, and timestamps. These commands are local/no-network
records for the private alpha; hosted/team policy enforcement remains deferred.
