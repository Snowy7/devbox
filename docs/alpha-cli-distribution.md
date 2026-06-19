# Alpha Tool Distribution

Devbox alpha testers need downloadable command-line tools and a runnable desktop control surface
before signed installers and hosted deployment hardening exist. For now, publishing is local: build the
macOS/Linux alpha archives on matching hosts and upload them to a GitHub Release with `gh`.

GitHub Packages is not the right first home for raw native binaries. Packages is useful for npm,
NuGet, Maven, RubyGems, and containers. Devbox alpha tools should start as GitHub Release assets.

## Credentials

R2 credentials belong in a local ignored file:

```bash
cp .env.example .env.r2.local
$EDITOR .env.r2.local
```

`.env.r2.local` must never be committed. The repo ignores `.env` and `.env.*`.

Load it before running trusted-operator R2 smoke commands:

```bash
source scripts/load-r2-env.sh .env.r2.local
```

The current variables are:

```bash
DEVBOX_R2_ENDPOINT=https://example-account-id.r2.cloudflarestorage.com
DEVBOX_R2_BUCKET=devbox-alpha
DEVBOX_R2_PREFIX=accounts/account-example/projects/project-example
DEVBOX_R2_ACCESS_KEY_ID=replace-me
DEVBOX_R2_SECRET_ACCESS_KEY=replace-me
DEVBOX_METADATA_API=http://127.0.0.1:8787
DEVBOX_METADATA_ACCOUNT=account-example
DEVBOX_METADATA_PROJECT=project-example
DEVBOX_OBJECT_ACCESS_LEASE=lease-alpha
DEVBOX_ALPHA_INVITE_CODE=replace-after-invite-create
DEVBOX_SESSION_TOKEN=replace-after-hosted-login
DEVBOX_LIVE_DB=./devbox.sqlite3
DEVBOX_LIVE_CACHE=./.devbox-cache
DEVBOX_LIVE_PROJECT_ROOT=./project
DEVBOX_LIVE_TARGET=./receiver-project
DEVBOX_REMOTE_KIND=s3
```

Pass credential variable names to the CLI and daemon:

```bash
--s3-access-key-env DEVBOX_R2_ACCESS_KEY_ID
--s3-secret-key-env DEVBOX_R2_SECRET_ACCESS_KEY
```

Do not pass raw key values as CLI arguments.

For external multi-user alpha testing, tester machines should not receive shared bucket credentials.
Use one shared bucket with per-account/project prefixes. The hosted transfer path keeps R2/S3
credentials on the Devbox metadata server and lets tester clients move encrypted object bytes through
the account-session/object-access boundary. The server enables object-access resolution and hosted
object transfer when these server-side env vars are populated:

```bash
DEVBOX_R2_ACCESS_KEY_ID=server-side-access-key
DEVBOX_R2_SECRET_ACCESS_KEY=server-side-secret-key
# optional:
DEVBOX_R2_SESSION_TOKEN=server-side-session-token
```

You can rename the server-side variable names with `DEVBOX_OBJECT_ACCESS_KEY_ENV`,
`DEVBOX_OBJECT_SECRET_KEY_ENV`, and `DEVBOX_OBJECT_SESSION_TOKEN_ENV`, but the server still checks
that the referenced env vars have values before enabling grants. A Cloudflare API token is not
required for the current broker; it does not call Cloudflare to mint temporary credentials.
For local/dev hosted-transfer testing without R2, set `DEVBOX_OBJECT_LOCAL_ROOT` on the metadata
server instead; this stores encrypted objects under a server-owned local object root and exercises the
same session, lease, prefix, and capability checks.

The prefix shape is the authorization boundary:

```text
accounts/<account-id>/projects/<project-id>
```

Every grant is scoped to one account session, one project, one lease, and one prefix. A tester should
never be told to set a prefix outside their own account/project path. External testers use
`--remote-kind hosted`; trusted operators can still use `--remote-kind s3` when they intentionally
place local bucket credentials on their own machine.

## Current Deployment Boundary

The local trusted-operator real-R2 smoke path does not require deploying the Devbox API. The external
multi-user hosted-transfer path does: run the hosted metadata API with server-side bucket credentials
and have testers use their session token against the object-access transfer endpoints. Raw bucket
credentials stay server-side.

The repo has a deployable hosted metadata alpha API with:

- `/ready`
- one-time alpha invite login
- bearer account-session status and logout
- hosted metadata handlers that reject mock-dev headers unless explicitly enabled
- server-mediated object-access prefix grants for one shared R2 bucket when server-managed R2 env
  credentials are configured
- hosted object transfer endpoints for encrypted put/get/head/list under the authorized prefix

To test hosted login locally:

```bash
cargo run -p devbox-metadata -- --db ./metadata-alpha.sqlite3 --listen 127.0.0.1:8787

cargo run -p devbox-cli -- metadata alpha-invite create \
  --db ./metadata-alpha.sqlite3 \
  --email dev@example.com

export DEVBOX_ALPHA_INVITE_CODE='<printed-invite-code>'

cargo run -p devbox-cli -- auth hosted-login \
  --api http://127.0.0.1:8787 \
  --email dev@example.com \
  --invite-code-env DEVBOX_ALPHA_INVITE_CODE

export DEVBOX_SESSION_TOKEN='<printed-session-token>'

cargo run -p devbox-cli -- auth hosted-status \
  --api http://127.0.0.1:8787
```

Create a managed object lease in the metadata DB and resolve the hosted shared-bucket grant:

```bash
cargo run -p devbox-cli -- metadata credential-lease mock-create \
  --db ./metadata-alpha.sqlite3 \
  --session-token "$DEVBOX_SESSION_TOKEN" \
  --verified-email dev@example.com \
  --project project-devbox \
  --lease lease-alpha \
  --endpoint "$DEVBOX_R2_ENDPOINT" \
  --bucket "$DEVBOX_R2_BUCKET" \
  --prefix "accounts/<printed-account-id>/projects/project-devbox"

cargo run -p devbox-cli -- metadata object-access resolve \
  --api "$DEVBOX_METADATA_API" \
  --session-token-env DEVBOX_SESSION_TOKEN \
  --project project-devbox \
  --lease lease-alpha
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
DEVBOX_LIVE_DB=./devbox.sqlite3 \
DEVBOX_LIVE_CACHE=./.devbox-cache \
DEVBOX_LIVE_PROJECT_ROOT=./project \
DEVBOX_REMOTE_DIR=./remote \
DEVBOX_METADATA_DB=./metadata-alpha.sqlite3 \
DEVBOX_LIVE_MODE=push \
DEVBOX_LIVE_ONCE=true \
scripts/devbox-live-sync-alpha.sh
```

For external tester hosted object transfer, set `DEVBOX_REMOTE_KIND=hosted`,
`DEVBOX_METADATA_API`, `DEVBOX_METADATA_DB`, `DEVBOX_METADATA_PROJECT`, `DEVBOX_SESSION_TOKEN`, and
`DEVBOX_OBJECT_ACCESS_LEASE`. Do not set `DEVBOX_R2_ACCESS_KEY_ID` or
`DEVBOX_R2_SECRET_ACCESS_KEY` on tester machines:

```bash
DEVBOX_REMOTE_KIND=hosted \
DEVBOX_METADATA_API=http://127.0.0.1:8787 \
DEVBOX_METADATA_DB=./metadata-alpha.sqlite3 \
DEVBOX_METADATA_PROJECT=project-devbox \
DEVBOX_SESSION_TOKEN='<tester-session-token>' \
DEVBOX_OBJECT_ACCESS_LEASE=lease-alpha \
DEVBOX_LIVE_MODE=push \
DEVBOX_LIVE_ONCE=true \
scripts/devbox-live-sync-alpha.sh
```

For trusted-operator shared-bucket R2 smoke testing, set `DEVBOX_REMOTE_KIND=s3`,
`DEVBOX_METADATA_API`, `DEVBOX_METADATA_DB`, `DEVBOX_METADATA_PROJECT`, `DEVBOX_SESSION_TOKEN`,
`DEVBOX_OBJECT_ACCESS_LEASE`, and `DEVBOX_R2_PREFIX=accounts/<account-id>/projects/<project-id>`.
The live daemon resolves the object-access grant before S3 work and refuses a prefix mismatch, then
the direct S3 transport still loads `DEVBOX_R2_ACCESS_KEY_ID` and `DEVBOX_R2_SECRET_ACCESS_KEY` from
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
- the Electron app reads redacted `DEVBOX_*` config and generated command state, but does not start
  the daemon or mutate files yet

## Local Alpha Tools Package

Build a host package containing `devbox`, `devbox-daemon`, `devbox-metadata`, docs, env template,
and alpha helper scripts:

```bash
scripts/package-cli.sh v0.1.0-alpha.1
```

The script writes:

```text
dist/devbox-v0.1.0-alpha.1-<target>.tar.gz
dist/devbox-v0.1.0-alpha.1-<target>.tar.gz.sha256
```

Supported local packaging targets:

- `x86_64-unknown-linux-gnu`
- `aarch64-apple-darwin`
- `x86_64-apple-darwin`

Set a target explicitly when needed:

```bash
DEVBOX_RELEASE_TARGET=x86_64-unknown-linux-gnu scripts/package-cli.sh v0.1.0-alpha.1
```

## Local Desktop Package

Build an unsigned Electron alpha bundle on macOS/Linux:

```bash
scripts/package-desktop-alpha.sh v0.1.0-alpha.1
```

The script runs the desktop safety scan and build, then writes:

```text
dist/devbox-desktop-v0.1.0-alpha.1.tar.gz
dist/devbox-desktop-v0.1.0-alpha.1.tar.gz.sha256
```

This is not a signed installer. Extract it, run `npm ci`, then `npm run start:built`. The desktop
surface reads `DEVBOX_*` env variables and shows redacted setup/command state only.

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

Run the same command from a Linux machine and a Mac if you want both platform archives on the same
release. Build/upload the desktop archive separately with `scripts/package-desktop-alpha.sh` until a
single release orchestrator exists. The CLI publish script uses `--clobber`, so rerunning replaces
the same target asset.

## Tester Install Notes

Linux:

```bash
tar -xzf devbox-v0.1.0-alpha.1-x86_64-unknown-linux-gnu.tar.gz
cd devbox-v0.1.0-alpha.1-x86_64-unknown-linux-gnu
./devbox --help
./devbox-daemon --help
./devbox-metadata --help
```

macOS:

```bash
tar -xzf devbox-v0.1.0-alpha.1-aarch64-apple-darwin.tar.gz
cd devbox-v0.1.0-alpha.1-aarch64-apple-darwin
xattr -dr com.apple.quarantine ./devbox ./devbox-daemon ./devbox-metadata
./devbox --help
./devbox-daemon --help
./devbox-metadata --help
```

## R2 Alpha Boundary

For many external testers, use one shared R2 bucket with account/project prefixes, but do not share
one long-lived bucket token across tester machines.

Current safe alpha setup:

- server-side R2 credentials live only in the hosted metadata API environment for grant validation
  and hosted object transfer
- each tester can log in through the hosted alpha session flow and resolve a grant for exactly one
  `accounts/<account-id>/projects/<project-id>` prefix
- external testers use `--remote-kind hosted` so encrypted object bytes travel through the Devbox
  API without client bucket keys
- direct `--remote-kind s3` with local R2 keys is trusted-operator smoke only
- for same-user two-device tests, run the receiver-generated pairing flow before import/materialize
  so the receiver can decrypt without `--mock-key-source-db`
- `--mock-key-source-db` remains only for legacy local smoke tests where both SQLite DBs are on the
  same machine

The prefix grant is now the hosted authorization boundary. Raw direct S3 credentials remain outside
the external-tester path; they are only for trusted-operator direct-R2 smoke.
