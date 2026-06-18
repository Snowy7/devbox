# Hosted Metadata Alpha

This is the first deployable Devbox API surface. It is suitable for a single-instance private alpha
where the metadata DB is mounted as a persistent SQLite file.

It is not the final production topology. Postgres, OAuth, the R2 credential broker, team billing,
and multi-region deployment hardening come later.

## Environment

```bash
DEVBOX_METADATA_DB=/data/devbox-metadata.sqlite3
DEVBOX_METADATA_LISTEN=0.0.0.0:8787
DEVBOX_SESSION_TTL_SECONDS=2592000
DEVBOX_PROOF_TTL_SECONDS=7776000
```

Mock-dev header auth is disabled by default in the server binary. Only enable it for local tests:

```bash
DEVBOX_ALLOW_MOCK_AUTH=true
```

## Run Locally

```bash
cargo run -p devbox-metadata -- --db ./metadata-alpha.sqlite3 --listen 127.0.0.1:8787
```

Readiness:

```bash
curl http://127.0.0.1:8787/ready
```

## Build Container

```bash
docker build -f infra/hosted/metadata.Dockerfile -t devbox-metadata:alpha .
mkdir -p .devbox-hosted-data
docker run --rm -p 8787:8787 \
  -v "$PWD/.devbox-hosted-data:/data" \
  devbox-metadata:alpha
```

## Create An Alpha Invite

Create a one-time invite in the metadata DB. The raw invite code is printed once; SQLite stores only
the hash.

```bash
cargo run -p devbox-cli -- metadata alpha-invite create \
  --db ./.devbox-hosted-data/devbox-metadata.sqlite3 \
  --email dev@example.com
```

## Tester Login

```bash
export DEVBOX_ALPHA_INVITE_CODE='<printed-invite-code>'

cargo run -p devbox-cli -- auth hosted-login \
  --api http://127.0.0.1:8787 \
  --email dev@example.com \
  --invite-code-env DEVBOX_ALPHA_INVITE_CODE

export DEVBOX_SESSION_TOKEN='<printed-session-token>'

cargo run -p devbox-cli -- auth hosted-status \
  --api http://127.0.0.1:8787
```

Logout revokes the stored session hash:

```bash
cargo run -p devbox-cli -- auth hosted-logout \
  --api http://127.0.0.1:8787
```

## Alpha Limits

- SQLite is for one running API instance with a persistent volume.
- Login is invite-based, not OAuth.
- Raw session tokens are returned once and should be kept in an ignored local env file or shell env.
- R2 credentials are still tester-local until the managed credential broker PR lands.
- Device pairing and project key envelopes are still local/mock until the pairing PR lands.
