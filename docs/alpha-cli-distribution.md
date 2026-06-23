# Alpha Tool Distribution

> Legacy alpha note: this page records the pre-Loom alpha implementation and may use `project` or `snapshot` for compatibility-era concepts. New work should say shared folder, file version, folder revision, checkpoint, pin, and cursor.


Bindhub alpha testers need downloadable command-line tools and a runnable desktop control surface
before signed installers and polished hosted operations exist. For now, publishing is local: build the
macOS/Linux alpha archives on matching hosts and upload them to a GitHub Release with `gh`.

Language note: this alpha distribution guide still uses `project` in env vars, command flags, and
prefixes because the current alpha API uses that implementation term for a scoped shared folder.
Product language should say shared folder. Loom is the codename for the source-control primitive
underneath Bindhub.

GitHub Packages is not the right first home for raw native binaries. Packages is useful for npm,
NuGet, Maven, RubyGems, and containers. Bindhub alpha tools should start as GitHub Release assets.

## Tester Path

Normal alpha testers should not receive Cloudflare/R2 endpoints, bucket names, prefixes, leases, or
object credentials. A packaged CLI should include the production Bindhub API endpoint. Local/dev
packages can still point at a temporary API with `BINDHUB_API_URL` or `bindhub login --api <URL>`.

The tester flow is:

```bash
./bindhub login
./bindhub share ~/code
./bindhub clone
./bindhub clone code ./code
./bindhub status
```

`bindhub login` stores the session locally. `bindhub share` and `bindhub clone` configure Loom under the
hood, but the user does not need to know about object storage, metadata projects, buckets, prefixes,
or credential leases.

## Operator Credentials

R2 credentials are API/operator configuration. They belong in a local ignored file only when a
trusted operator is deploying the API or running direct-R2 smoke tests:

```bash
cp .env.example .env.r2.local
$EDITOR .env.r2.local
```

In a packaged release archive, copy `.env.operator.example` instead. The packaged `.env.example` is a
small user/local-dev CLI override file and intentionally does not include R2 settings.

`.env.r2.local` must never be committed. The repo ignores `.env` and `.env.*`.

Load it before running trusted-operator R2 smoke commands:

```bash
source scripts/load-r2-env.sh .env.r2.local
```

The operator variables are:

```bash
BINDHUB_R2_ENDPOINT=https://example-account-id.r2.cloudflarestorage.com
BINDHUB_R2_BUCKET=bindhub-alpha
BINDHUB_R2_ACCESS_KEY_ID=replace-me
BINDHUB_R2_SECRET_ACCESS_KEY=replace-me
BINDHUB_METADATA_API=http://127.0.0.1:8787
BINDHUB_METADATA_PROJECT=project-example
BINDHUB_METADATA_DATABASE_URL=postgres://bindhub:bindhub@127.0.0.1:5432/bindhub_metadata
BINDHUB_ALPHA_INVITE_CODE=replace-after-invite-create
BINDHUB_SESSION_TOKEN=replace-after-hosted-login
BINDHUB_LIVE_DB=./bindhub.sqlite3
BINDHUB_LIVE_CACHE=./.bindhub-cache
BINDHUB_LIVE_PROJECT_ROOT=./project
BINDHUB_LIVE_TARGET=./receiver-project
BINDHUB_REMOTE_KIND=s3
```

Pass credential variable names to the CLI and daemon:

```bash
--s3-access-key-env BINDHUB_R2_ACCESS_KEY_ID
--s3-secret-key-env BINDHUB_R2_SECRET_ACCESS_KEY
```

Do not pass raw key values as CLI arguments.

For external multi-user alpha testing, tester machines should not receive shared bucket credentials.
Use one shared bucket with per-account/folder-scope prefixes. The hosted transfer path keeps R2/S3
credentials on the bindhub metadata server and lets tester clients move encrypted object bytes through
the account-session/object-access boundary. The server enables object-access resolution and hosted
object transfer when these server-side env vars are populated:

```bash
BINDHUB_R2_ENDPOINT=https://example-account-id.r2.cloudflarestorage.com
BINDHUB_R2_BUCKET=bindhub-alpha
BINDHUB_R2_ACCESS_KEY_ID=server-side-access-key
BINDHUB_R2_SECRET_ACCESS_KEY=server-side-secret-key
# optional:
BINDHUB_R2_REGION=auto
BINDHUB_R2_SESSION_TOKEN=server-side-session-token
```

You can rename the server-side variable names with `BINDHUB_OBJECT_ACCESS_KEY_ENV`,
`BINDHUB_OBJECT_SECRET_KEY_ENV`, and `BINDHUB_OBJECT_SESSION_TOKEN_ENV`, but the server still checks
that the referenced env vars have values before enabling grants. A Cloudflare API token is not
required for the current broker; it does not call Cloudflare to mint temporary credentials.
For local/dev hosted-transfer testing without R2, set `BINDHUB_OBJECT_LOCAL_ROOT` on the metadata
server instead; this stores encrypted objects under a server-owned local object root and exercises the
same session, derived-prefix, and capability checks.

The prefix shape is the authorization boundary:

```text
accounts/<account-id>/projects/<project-id>
```

Every grant is scoped to one account session, one folder scope, and one derived prefix. A tester should
never be told to set a prefix, bucket, or R2 endpoint. External testers use
`--remote-kind hosted`; trusted operators can still use `--remote-kind s3` when they intentionally
place local bucket credentials on their own machine.

## Current Deployment Boundary

The local trusted-operator real-R2 smoke path does not require deploying the Bindhub API. The external
multi-user hosted-transfer path does: run the hosted metadata API with Railway/Postgres metadata
storage and server-side bucket credentials, then have testers use their session token against the
object-access transfer endpoints. Raw bucket credentials stay server-side.

The repo has a deployable hosted metadata alpha API with:

- `/ready`
- one-time alpha invite login
- bearer account-session status and logout
- hosted metadata handlers that reject mock-dev headers unless explicitly enabled
- Postgres metadata storage selected by `DATABASE_URL` or `BINDHUB_METADATA_DATABASE_URL`, with
  SQLite preserved for local/dev `--db` smoke tests
- server-mediated object access for one shared R2 bucket when server-managed R2 env credentials are
  configured
- hosted object transfer endpoints for encrypted put/get/head/list under the server-derived object
  scope

To test hosted login locally:

```bash
cargo run -p bindhub-metadata -- --db ./metadata-alpha.sqlite3 --listen 127.0.0.1:8787

cargo run -p bindhub-cli -- metadata alpha-invite create \
  --db ./metadata-alpha.sqlite3 \
  --email dev@example.com

export BINDHUB_ALPHA_INVITE_CODE='<printed-invite-code>'

cargo run -p bindhub-cli -- auth hosted-login \
  --api http://127.0.0.1:8787 \
  --email dev@example.com \
  --invite-code-env BINDHUB_ALPHA_INVITE_CODE

export BINDHUB_SESSION_TOKEN='<printed-session-token>'

cargo run -p bindhub-cli -- auth hosted-status \
  --api http://127.0.0.1:8787
```

For a Railway-shaped local/Postgres server, set `DATABASE_URL` or `BINDHUB_METADATA_DATABASE_URL`
instead of `--db`/`BINDHUB_METADATA_DB`:

```bash
DATABASE_URL=postgres://bindhub:bindhub@127.0.0.1:5432/bindhub_metadata \
cargo run -p bindhub-metadata -- --listen 127.0.0.1:8787
```

The hosted server owns object storage. You do not need to seed a per-user bucket, prefix, or
credential lease. If you are debugging the low-level hosted object path, resolve the server-derived
grant with the stable internal lease id:

```bash
cargo run -p bindhub-cli -- metadata object-access resolve \
  --api "$BINDHUB_METADATA_API" \
  --session-token-env BINDHUB_SESSION_TOKEN \
  --project project-bindhub \
  --lease bindhub-managed
```

For Railway/Postgres admin seeding, put the Postgres connection string in an environment variable and
reference the variable name instead of passing the raw URL on argv. This is for invites only; object
storage is configured on the API service with `BINDHUB_R2_*` env vars:

```bash
export BINDHUB_METADATA_DATABASE_URL='<railway-postgres-url>'

cargo run -p bindhub-cli -- metadata alpha-invite create \
  --postgres-url-env BINDHUB_METADATA_DATABASE_URL \
  --email dev@example.com
```

`object-access resolve` prints the authorized prefix, endpoint, bucket, capabilities, expiration,
and rotation generation. It does not print or return raw R2 credentials.

For a deterministic local two-device live-sync smoke test:

```bash
scripts/alpha-two-device-smoke.sh
```

That script initializes source and receiver DBs, runs receiver-generated pairing, proves the pending
receiver fails closed before completion, publishes a live snapshot into a local encrypted remote,
pulls the latest mock hosted snapshot, materializes it into the receiver target, and writes redacted
evidence logs under the printed `evidence=` directory.

For a lower-level local live-sync command, use daemon once mode:

```bash
BINDHUB_LIVE_DB=./bindhub.sqlite3 \
BINDHUB_LIVE_CACHE=./.bindhub-cache \
BINDHUB_LIVE_PROJECT_ROOT=./project \
BINDHUB_REMOTE_DIR=./remote \
BINDHUB_METADATA_DB=./metadata-alpha.sqlite3 \
BINDHUB_LIVE_MODE=push \
BINDHUB_LIVE_ONCE=true \
scripts/bindhub-live-sync-alpha.sh
```

For product-level hosted testing, use `bindhub login`, `bindhub share`, and `bindhub clone`; do not hand
testers metadata project ids, object leases, bucket names, prefixes, or R2 endpoints.

For low-level hosted object-transfer smoke testing, trusted operators may still set
`BINDHUB_REMOTE_KIND=hosted`, `BINDHUB_METADATA_API`, `BINDHUB_METADATA_PROJECT`, and
`BINDHUB_SESSION_TOKEN`. Do not set `BINDHUB_R2_ENDPOINT`, `BINDHUB_R2_BUCKET`,
`BINDHUB_R2_ACCESS_KEY_ID`, or `BINDHUB_R2_SECRET_ACCESS_KEY` on tester machines:

```bash
BINDHUB_REMOTE_KIND=hosted \
BINDHUB_METADATA_API=http://127.0.0.1:8787 \
BINDHUB_METADATA_PROJECT=project-bindhub \
BINDHUB_SESSION_TOKEN='<tester-session-token>' \
BINDHUB_LIVE_MODE=push \
BINDHUB_LIVE_ONCE=true \
scripts/bindhub-live-sync-alpha.sh
```

For trusted-operator shared-bucket R2 smoke testing, set `BINDHUB_REMOTE_KIND=s3`,
`BINDHUB_METADATA_API`, `BINDHUB_METADATA_DB`, `BINDHUB_METADATA_PROJECT`, `BINDHUB_SESSION_TOKEN`,
`BINDHUB_OBJECT_ACCESS_LEASE`, and `BINDHUB_R2_PREFIX=accounts/<account-id>/projects/<project-id>`.
The live daemon resolves the object-access grant before S3 work and refuses a prefix mismatch, then
the direct S3 transport still loads `BINDHUB_R2_ACCESS_KEY_ID` and `BINDHUB_R2_SECRET_ACCESS_KEY` from
the local environment.

The current object-transfer paths are split:

- external testers use hosted object transfer and need no local R2/S3 bucket keys
- object bytes go directly to R2 only in trusted-operator direct-S3 smoke mode
- trusted operators can still run direct S3-compatible CLI smoke tests with local `.env.r2.local`
  credentials and the authorized prefix
- device trust can use receiver-generated pairing with `devices join`, `devices approve-join`, and
  `devices complete`
- live daemon sync can publish current work and pull the latest hosted mock-dev snapshot with
  deterministic `--once` tests and long-running debounce mode
- the Electron app reads redacted `BINDHUB_*` config and generated command state, but does not start
  the daemon or mutate files yet

## Local Alpha Tools Package

Build a host package containing `loom`, `bindhub`, `bindhub-daemon`, `bindhub-metadata`, docs, a user
CLI env template, an operator env template, and alpha helper scripts:

```bash
scripts/package-cli.sh v0.1.0-alpha.1
```

The script writes:

```text
dist/bindhub-v0.1.0-alpha.1-<target>.tar.gz
dist/bindhub-v0.1.0-alpha.1-<target>.tar.gz.sha256
```

Supported local packaging targets:

- `x86_64-unknown-linux-gnu`
- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `x86_64-pc-windows-msvc` through `scripts/package-cli.ps1`

Set a target explicitly when needed:

```bash
BINDHUB_RELEASE_TARGET=x86_64-unknown-linux-gnu scripts/package-cli.sh v0.1.0-alpha.1
```

On Windows:

```powershell
.\scripts\package-cli.ps1 -Version v0.1.0-alpha.1
```

Packages bake in `https://bindhub-staging.up.railway.app` as the default Bindhub API unless
`BINDHUB_DEFAULT_API_URL` or `-ApiUrl` is supplied at package time.

## Local Desktop Package

Build an unsigned Electron alpha bundle on macOS/Linux:

```bash
scripts/package-desktop-alpha.sh v0.1.0-alpha.1
```

The script runs the desktop safety scan and build, then writes:

```text
dist/bindhub-desktop-v0.1.0-alpha.1.tar.gz
dist/bindhub-desktop-v0.1.0-alpha.1.tar.gz.sha256
```

This is not a signed installer. Extract it, run `pnpm install`, then `pnpm start:built`. The desktop
surface reads `BINDHUB_*` env variables and shows redacted setup/command state only.

## Publish Locally To GitHub Releases

Use a prerelease tag for alpha testers:

```bash
git switch main
git pull --ff-only
scripts/publish-cli-release.sh v0.1.0-alpha.1
```

The publish script:

1. Requires a clean working tree.
2. Creates the tag locally if it does not exist.
3. Pushes the tag.
4. Builds the alpha tools archive on the current machine.
5. Creates or updates the GitHub Release.
6. Uploads the archive and its `.sha256` file.

For the current alpha release flow:

- run `scripts/publish-cli-release.ps1` from Windows to build and upload the Windows zip
- push the same `v*` tag to trigger `.github/workflows/release-macos-cli.yml`, which builds and
  uploads both macOS archives from GitHub Actions
- run the bash publish script from Linux only if a Linux archive is needed

The publish scripts and macOS workflow use `--clobber`, so rerunning replaces the same target asset.

Windows local publish:

```powershell
git switch main
git pull --ff-only
.\scripts\publish-cli-release.ps1 -Tag v0.1.0-alpha.1
```

Monitor macOS packaging after the tag is pushed:

```bash
gh run list --workflow "Release macOS CLI" --limit 5
gh run watch <RUN_ID> --exit-status
```

## Install Or Update

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/Snowy7/bindhub/main/scripts/install-bindhub.ps1 | iex
```

macOS/Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/Snowy7/bindhub/main/scripts/install-bindhub.sh | sh
```

Rerun the same command to update. To install a specific version:

```powershell
irm https://raw.githubusercontent.com/Snowy7/bindhub/main/scripts/install-bindhub.ps1 -OutFile install-bindhub.ps1
.\install-bindhub.ps1 -Version v0.1.0-alpha.1
```

```bash
curl -fsSL https://raw.githubusercontent.com/Snowy7/bindhub/main/scripts/install-bindhub.sh | sh -s -- v0.1.0-alpha.1
```

## Tester Install Notes

Linux:

```bash
tar -xzf bindhub-v0.1.0-alpha.1-x86_64-unknown-linux-gnu.tar.gz
cd bindhub-v0.1.0-alpha.1-x86_64-unknown-linux-gnu
./bindhub --help
./loom --help
./bindhub-daemon --help
./bindhub-metadata --help
```

macOS:

```bash
tar -xzf bindhub-v0.1.0-alpha.1-aarch64-apple-darwin.tar.gz
cd bindhub-v0.1.0-alpha.1-aarch64-apple-darwin
xattr -dr com.apple.quarantine ./loom ./bindhub ./bindhub-daemon ./bindhub-metadata
./bindhub --help
./loom --help
./bindhub-daemon --help
./bindhub-metadata --help
```

## R2 Alpha Boundary

For many external testers, the API can use one shared R2 bucket with account/project object scopes,
but do not share one long-lived bucket token across tester machines.

Current safe alpha setup:

- server-side R2 credentials live only in the hosted metadata API environment for grant validation
  and hosted object transfer
- each tester can log in through the hosted alpha session flow while the API derives the exact
  `accounts/<account-id>/projects/<project-id>` object prefix internally
- external testers use `--remote-kind hosted` so encrypted object bytes travel through the Bindhub
  API without client bucket keys
- direct `--remote-kind s3` with local R2 keys is trusted-operator smoke only
- for same-user two-device tests, run the receiver-generated pairing flow before import/materialize
  so the receiver can decrypt without `--mock-key-source-db`
- `--mock-key-source-db` remains only for legacy local smoke tests where both SQLite DBs are on the
  same machine

The server-derived prefix is the hosted authorization boundary. Raw direct S3 credentials remain
outside the external-tester path; they are only for trusted-operator direct-R2 smoke.
