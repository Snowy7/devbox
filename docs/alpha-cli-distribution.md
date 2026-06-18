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
DEVBOX_R2_BUCKET=devbox-alpha-your-name
DEVBOX_R2_PREFIX=v1/projects/manual-test-001
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

## Current Deployment Boundary

The manual real-R2 alpha path still does not require deploying the Devbox API.

The repo now has a deployable hosted metadata alpha API with:

- `/ready`
- one-time alpha invite login
- bearer account-session status and logout
- hosted metadata handlers that can reject mock-dev headers unless explicitly enabled

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

The current real-R2 smoke path is still local/manual:

- object bytes go to R2
- snapshot metadata can live in local SQLite, with hosted auth now available separately
- device trust is bootstrapped with `--mock-key-source-db`
- production credential leasing and production key exchange are deferred to the next alpha PRs
- the Electron app is not yet wired to live daemon/API state

When production credential leasing is built, the backend will need deployment. It will own the
parent R2 credential and issue short-lived scoped credentials to clients.

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

For many external testers, use one R2 bucket per tester until the hosted credential broker can issue
short-lived prefix-scoped credentials.

Current safe alpha setup:

- each tester gets their own bucket or tightly controlled credential
- each tester keeps credentials in `.env.r2.local`
- do not share one long-lived bucket token across untrusted testers
- `--mock-key-source-db` is still required so the receiver can decrypt publisher objects

Prefixes are useful for organization, but they are not the security boundary yet.
