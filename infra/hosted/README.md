# Hosted Metadata Alpha

This is the deployable Devbox alpha API surface. Railway-hosted multi-user alpha deployments should
run `devbox-metadata` against managed Postgres through `DATABASE_URL`.

Language note: the hosted API still uses `project` in routes and storage because the alpha schema
uses that word for a scoped shared folder. Product docs should say shared folder.

SQLite remains supported for local development and tests. Do not use SQLite for the shared external
tester backend.

## Environment

```bash
DATABASE_URL=${{Postgres.DATABASE_URL}}
DEVBOX_SESSION_TTL_SECONDS=2592000
DEVBOX_PROOF_TTL_SECONDS=7776000
DEVBOX_ALLOW_MOCK_AUTH=false
DEVBOX_R2_ACCESS_KEY_ID=<server-side-r2-access-key>
DEVBOX_R2_SECRET_ACCESS_KEY=<server-side-r2-secret-key>
# optional:
DEVBOX_R2_SESSION_TOKEN=<server-side-r2-session-token>
```

Railway injects service variables as environment variables at build/runtime. Bind a Railway Postgres
service and set the metadata service variable `DATABASE_URL=${{Postgres.DATABASE_URL}}`. The server
also accepts `DEVBOX_METADATA_DATABASE_URL`, but `DATABASE_URL` is the Railway-shaped default.

The server listens on `0.0.0.0:$PORT` when Railway provides `PORT`; `PORT` takes precedence over
`DEVBOX_METADATA_LISTEN` so Railway can route to the injected runtime port. `/ready` reports the
storage label, auth policy, and object broker status.

Mock-dev header auth is disabled by default in the server binary. Only enable it for local tests:

```bash
DEVBOX_ALLOW_MOCK_AUTH=true
```

## Run Locally

SQLite local/dev:

```bash
cargo run -p devbox-metadata -- --db ./metadata-alpha.sqlite3 --listen 127.0.0.1:8787
```

Postgres local/dev:

```bash
DATABASE_URL=postgres://devbox:devbox@127.0.0.1:5432/devbox_metadata \
cargo run -p devbox-metadata -- --listen 127.0.0.1:8787
```

If both `DATABASE_URL`/`DEVBOX_METADATA_DATABASE_URL` and `DEVBOX_METADATA_DB`/`--db` are configured,
startup fails so the backend cannot accidentally write to the wrong store.

Readiness:

```bash
curl http://127.0.0.1:8787/ready
```

## Build Container

```bash
docker build -f infra/hosted/metadata.Dockerfile -t devbox-metadata:alpha .
docker run --rm -p 8787:8787 \
  -e DATABASE_URL=postgres://devbox:devbox@host.docker.internal:5432/devbox_metadata \
  -e DEVBOX_OBJECT_LOCAL_ROOT=/tmp/devbox-hosted-objects \
  devbox-metadata:alpha
```

The image intentionally does not default to an internal SQLite file. For a local SQLite container
smoke, pass `-e DEVBOX_METADATA_DB=/data/devbox-metadata.sqlite3` and mount `/data` explicitly.

## Railway

This repo includes [railway.toml](../../railway.toml), which builds
[metadata.Dockerfile](metadata.Dockerfile) and uses `/ready` as the healthcheck.

1. Create or connect a Railway PostgreSQL service.
2. Set `DATABASE_URL=${{Postgres.DATABASE_URL}}` on the `devbox-metadata` service.
3. Set server-side object broker env:
   `DEVBOX_R2_ACCESS_KEY_ID`, `DEVBOX_R2_SECRET_ACCESS_KEY`, and optionally
   `DEVBOX_R2_SESSION_TOKEN`. These values stay on the server.
4. Keep `DEVBOX_ALLOW_MOCK_AUTH=false` or unset.
5. Deploy and confirm `/ready` returns `storage: "postgres-railway-alpha"`,
   `mock_auth_enabled: false`, and `object_access_broker_enabled: true`.

## Create An Alpha Invite

Create a one-time invite in the metadata DB. The raw invite code is printed once; metadata stores
only the hash.

```bash
cargo run -p devbox-cli -- metadata alpha-invite create \
  --db ./.devbox-hosted-data/devbox-metadata.sqlite3 \
  --email dev@example.com
```

For a Railway/Postgres backend, do not pass the raw database URL on argv. Reference an env var:

```bash
export DEVBOX_METADATA_DATABASE_URL='<railway-postgres-url>'

cargo run -p devbox-cli -- metadata alpha-invite create \
  --postgres-url-env DEVBOX_METADATA_DATABASE_URL \
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

- Railway/Postgres is the shared alpha backend for external multi-user testers.
- SQLite is local/dev only.
- Login is invite-based, not OAuth.
- Raw session tokens are returned once and should be kept in an ignored local env file or shell env.
- R2 credentials stay on the metadata server for hosted object transfer; trusted operators can still
  run direct-S3 smoke tests with local bucket keys.
- Device pairing and folder key envelopes are still local alpha flows, not hosted onboarding UX.
- OAuth/OIDC provider login, team billing, multi-region hardening, and signed installers remain
  future work.
