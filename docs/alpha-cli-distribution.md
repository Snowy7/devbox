# Alpha CLI Distribution

Devbox alpha testers need a downloadable CLI binary before the desktop app is wired to live daemon
state. For now, publishing is local: build the CLI on a macOS or Linux machine and upload the
archive to a GitHub Release with `gh`.

GitHub Packages is not the right first home for the raw native binary. Packages is useful for npm,
NuGet, Maven, RubyGems, and containers. A standalone CLI binary should start as a GitHub Release
asset.

## Credentials

R2 credentials belong in a local ignored file:

```bash
cp .env.example .env.r2.local
$EDITOR .env.r2.local
```

`.env.r2.local` must never be committed. The repo ignores `.env` and `.env.*`.

Load it before running R2 smoke commands:

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
DEVBOX_ALPHA_INVITE_CODE=replace-after-invite-create
DEVBOX_SESSION_TOKEN=replace-after-hosted-login
```

Pass the credential variable names to the CLI:

```bash
--s3-access-key-env DEVBOX_R2_ACCESS_KEY_ID
--s3-secret-key-env DEVBOX_R2_SECRET_ACCESS_KEY
```

Do not pass raw key values as CLI arguments.

For external multi-user alpha testing, tester machines should not receive shared bucket credentials.
Run R2 credentials on the hosted metadata server only. The server enables object-access resolution
when these server-side env vars are populated:

```bash
DEVBOX_R2_ACCESS_KEY_ID=server-side-access-key
DEVBOX_R2_SECRET_ACCESS_KEY=server-side-secret-key
# optional:
DEVBOX_R2_SESSION_TOKEN=server-side-session-token
```

You can rename the server-side variable names with `DEVBOX_OBJECT_ACCESS_KEY_ENV`,
`DEVBOX_OBJECT_SECRET_KEY_ENV`, and `DEVBOX_OBJECT_SESSION_TOKEN_ENV`, but the server still checks
that the referenced env vars have values before enabling grants. A Cloudflare API token is not
required for this PR; the broker does not call Cloudflare to mint temporary credentials.

## Current Deployment Boundary

The manual real-R2 alpha path still does not require deploying the Devbox API.

The repo now has a deployable hosted metadata alpha API with:

- `/ready`
- one-time alpha invite login
- bearer account-session status and logout
- hosted metadata handlers that can reject mock-dev headers unless explicitly enabled
- server-mediated object-access prefix grants for one shared R2 bucket when server-managed R2 env
  credentials are configured

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

For a local deterministic live-sync smoke test, use the daemon once mode:

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

For shared-bucket R2 alpha testing, set `DEVBOX_REMOTE_KIND=s3`,
`DEVBOX_METADATA_API`, `DEVBOX_METADATA_DB`, `DEVBOX_METADATA_PROJECT`, `DEVBOX_SESSION_TOKEN`,
`DEVBOX_OBJECT_ACCESS_LEASE`, and `DEVBOX_R2_PREFIX=accounts/<account-id>/projects/<project-id>`.
The live daemon resolves the object-access grant before S3 work and refuses a prefix mismatch.

The current real-R2 smoke path is split:

- object bytes go to R2
- trusted operators can still run direct S3-compatible CLI smoke tests with local `.env.r2.local`
  credentials and the authorized prefix
- external testers should use hosted auth plus the server-mediated object-access grant; direct
  shared bucket credentials are not the multi-user security boundary
- device trust can now use receiver-generated pairing with `devices join`, `devices approve-join`,
  and `devices complete`
- live daemon sync can publish current work and pull the latest hosted mock-dev snapshot with
  deterministic `--once` tests and long-running debounce mode
- hosted object proxy or signed URL data transfer is deferred to the next alpha PRs
- the Electron app is not yet wired to live daemon/API state

## Local Package

Build a host package:

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
4. Builds the CLI archive on the current machine.
5. Creates or updates the GitHub Release.
6. Uploads the archive and its `.sha256` file.

Run the same command from a Linux machine and a Mac if you want both platform archives on the same
release. The script uses `--clobber`, so rerunning replaces the same target asset.

## Tester Install Notes

Linux:

```bash
tar -xzf devbox-v0.1.0-alpha.1-x86_64-unknown-linux-gnu.tar.gz
cd devbox-v0.1.0-alpha.1-x86_64-unknown-linux-gnu
./devbox --help
```

macOS:

```bash
tar -xzf devbox-v0.1.0-alpha.1-aarch64-apple-darwin.tar.gz
cd devbox-v0.1.0-alpha.1-aarch64-apple-darwin
xattr -dr com.apple.quarantine ./devbox
./devbox --help
```

## R2 Alpha Boundary

For many external testers, use one shared R2 bucket with account/project prefixes, but do not share
one long-lived bucket token across tester machines.

Current safe alpha setup:

- server-side R2 credentials live only in the hosted metadata API environment
- each tester logs in through the hosted alpha session flow and resolves a grant for exactly one
  `accounts/<account-id>/projects/<project-id>` prefix
- direct `--remote-kind s3` with local R2 keys is trusted-operator smoke only
- for same-user two-device tests, run the receiver-generated pairing flow before import/materialize
  so the receiver can decrypt without `--mock-key-source-db`
- `--mock-key-source-db` remains only for legacy local smoke tests where both SQLite DBs are on the
  same machine

The prefix grant is now the hosted authorization boundary. Until the object proxy/signed URL path is
wired to the sync provider, raw direct S3 credentials remain outside the external-tester path.
