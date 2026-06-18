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
```

Pass the credential variable names to the CLI:

```bash
--s3-access-key-env DEVBOX_R2_ACCESS_KEY_ID
--s3-secret-key-env DEVBOX_R2_SECRET_ACCESS_KEY
```

Do not pass raw key values as CLI arguments.

## Current Deployment Boundary

The manual real-R2 alpha path does not require deploying the Devbox API.

The repo has hosted metadata and account/session foundations, but the current real-R2 smoke path is
still local/manual:

- object bytes go to R2
- snapshot metadata lives in local SQLite
- device trust is bootstrapped with `--mock-key-source-db`
- production credential leasing and production key exchange are deferred
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
