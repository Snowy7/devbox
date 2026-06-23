# Hosted Deploys

This folder contains hosted/container entrypoints.

Current container entrypoints:

- `bindhub-api.Dockerfile`: Rust product API.
- `web.Dockerfile`: TanStack Start dashboard server. Runtime env is read from container env.
- `site.Dockerfile`: Astro static site/docs server. Public dashboard links are read from runtime env.

Railway services must use explicit service config files. Do not put a root `railway.toml` back in
the repo unless the repo goes back to one Railway service; Railway will auto-apply a root config to
services that do not override it.

Use these Railway config file paths:

```text
API:       /infra/hosted/railway-api.toml
Dashboard: /infra/hosted/railway-web.toml
Site:      /infra/hosted/railway-site.toml
```

The API config builds `bindhub-api`, which is the MVP product API used by:

```text
bindhub login
bindhub share <folder>
bindhub clone <name>
```

`bindhub-metadata` is still kept for legacy alpha metadata/object-access smoke paths.

## Local Alpha Path

Local alpha testing does not require Postgres or R2. Run `bindhub-api` with in-memory metadata and
the local file pack store, then use the product CLI:

```bash
BINDHUB_API_METADATA_MODE=memory bindhub-api --root .bindhub-api --bind 127.0.0.1:8787
bindhub login --api http://127.0.0.1:8787 --account local-dev --device-name "Desktop"
bindhub share ./source --no-background-sync

BINDHUB_CONFIG_DIR=.bindhub-laptop \
  bindhub login --api http://127.0.0.1:8787 --account local-dev --device-name "Laptop"
BINDHUB_CONFIG_DIR=.bindhub-laptop \
  bindhub clone source ./target --no-background-sync
```

The canonical local proof remains `scripts/mvp-two-device-smoke`, which automates this flow and
adds Loom sparse/cache, secret-block, conflict refusal, and object hash validation evidence.

## Product API On Railway

`bindhub-api` stores sessions, devices, shared-folder registry, memberships, and cursors in
Postgres. Loom pack bytes use server-owned R2-compatible object storage when `BINDHUB_R2_ENDPOINT`
and `BINDHUB_R2_BUCKET` are configured. `BINDHUB_API_ROOT` is only a scratch/local-pack fallback path;
it is not the durable product metadata store.

Railway setup:

1. Create a Railway service for the API.
2. In the API service settings, set the Railway config file path to
   `/infra/hosted/railway-api.toml`.
3. Attach Railway Postgres and set `DATABASE_URL` on the `bindhub-api` service. You can also use
   `BINDHUB_API_DATABASE_URL`; `DATABASE_URL` is the normal Railway path.
4. Configure server-side R2 pack storage when staging should use Cloudflare object storage:
   `BINDHUB_R2_ENDPOINT`, `BINDHUB_R2_BUCKET`, `BINDHUB_R2_ACCESS_KEY_ID`,
   `BINDHUB_R2_SECRET_ACCESS_KEY`, optional `BINDHUB_R2_REGION=auto`, optional
   `BINDHUB_R2_PREFIX`, and optional `BINDHUB_R2_SESSION_TOKEN`.
5. Deploy and confirm `/ready` returns `service: "bindhub-api"`, `metadata: "postgres"`, and
   `storage: "r2-packs"` when R2 is active.

Do not attach a Railway Volume for the product API. Durable API metadata lives in Postgres, and pack
bytes live in R2. The Dockerfile intentionally does not include a Docker `VOLUME` instruction.

Local container smoke:

```bash
docker build -f infra/hosted/bindhub-api.Dockerfile -t bindhub-api:alpha .
docker run --rm -d -p 5432:5432 --name bindhub-api-postgres \
  -e POSTGRES_USER=bindhub \
  -e POSTGRES_PASSWORD=bindhub \
  -e POSTGRES_DB=bindhub \
  postgres:16-alpine
docker run --rm -p 8787:8787 \
  -e PORT=8787 \
  -e DATABASE_URL=postgres://bindhub:bindhub@host.docker.internal:5432/bindhub \
  -e BINDHUB_R2_ENDPOINT=https://<cloudflare-account-id>.r2.cloudflarestorage.com \
  -e BINDHUB_R2_BUCKET=bindhub-alpha \
  -e BINDHUB_R2_ACCESS_KEY_ID=<server-side-r2-access-key> \
  -e BINDHUB_R2_SECRET_ACCESS_KEY=<server-side-r2-secret-key> \
  bindhub-api:alpha
curl http://127.0.0.1:8787/ready
```

These values stay on the server. End-user machines should only know the Bindhub API URL and their
session/device state; they should never configure Cloudflare endpoints, bucket names, prefixes, or
R2 credentials.

## Dashboard Container

Build:

```bash
docker build -f infra/hosted/web.Dockerfile -t bindhub-web:staging .
```

Run locally:

```bash
docker run --rm -p 3000:3000 \
  -e PORT=3000 \
  -e WORKOS_CLIENT_ID=client_... \
  -e WORKOS_API_KEY=sk_... \
  -e WORKOS_COOKIE_PASSWORD=<32-plus-character-secret> \
  -e WORKOS_REDIRECT_URI=http://localhost:3000/auth/callback \
  -e WORKOS_SIGN_OUT_REDIRECT_URI=http://localhost:3000/ \
  -e VITE_BINDHUB_DOCS_URL=http://localhost:3002/docs \
  -e BINDHUB_DASHBOARD_DATA_MODE=hosted-workos \
  -e BINDHUB_HOSTED_API_URL=http://host.docker.internal:3001 \
  -e BINDHUB_HOSTED_API_SERVICE_TOKEN=<same-token-as-api> \
  bindhub-web:staging
```

For staging, point the redirect and API values at the deployed domains:

```bash
WORKOS_REDIRECT_URI=https://app-staging.bindhub.com/auth/callback
WORKOS_SIGN_OUT_REDIRECT_URI=https://app-staging.bindhub.com/
VITE_BINDHUB_DOCS_URL=https://staging.bindhub.com/docs
BINDHUB_HOSTED_API_URL=https://api-staging.bindhub.com
```

Railway setup:

1. Create a Railway service for the dashboard.
2. Set the Railway config file path to `/infra/hosted/railway-web.toml`.
3. Set the WorkOS and hosted API env vars on this dashboard service. `BINDHUB_HOSTED_API_URL` and
   `BINDHUB_HOSTED_API_SERVICE_TOKEN` are server-only runtime values.

## Site Container

Build:

```bash
docker build -f infra/hosted/site.Dockerfile -t bindhub-site:staging .
```

Run locally:

```bash
docker run --rm -p 3002:3002 -e PORT=3002 bindhub-site:staging
```

Railway setup:

1. Create a Railway service for the public site/docs.
2. Set the Railway config file path to `/infra/hosted/railway-site.toml`.
3. Set `PUBLIC_BINDHUB_DASHBOARD_URL` when the dashboard lives on a separate domain. The site
   server applies this value at runtime while serving HTML.

## Legacy Metadata API

`bindhub-metadata` is the deployable compatibility API for the older hosted metadata/object-access
alpha path. It supports Railway/Postgres metadata storage and server-owned R2-compatible object
storage for low-level smoke commands.

Environment:

```bash
DATABASE_URL=${{Postgres.DATABASE_URL}}
BINDHUB_SESSION_TTL_SECONDS=2592000
BINDHUB_PROOF_TTL_SECONDS=7776000
BINDHUB_ALLOW_MOCK_AUTH=false
BINDHUB_R2_ENDPOINT=https://<cloudflare-account-id>.r2.cloudflarestorage.com
BINDHUB_R2_BUCKET=bindhub-alpha
BINDHUB_R2_ACCESS_KEY_ID=<server-side-r2-access-key>
BINDHUB_R2_SECRET_ACCESS_KEY=<server-side-r2-secret-key>
# optional:
BINDHUB_R2_REGION=auto
BINDHUB_R2_SESSION_TOKEN=<server-side-r2-session-token>
```

Build locally:

```bash
docker build -f infra/hosted/metadata.Dockerfile -t bindhub-metadata:alpha .
docker run --rm -p 8787:8787 \
  -e DATABASE_URL=postgres://bindhub:bindhub@host.docker.internal:5432/bindhub_metadata \
  -e BINDHUB_OBJECT_LOCAL_ROOT=/tmp/bindhub-hosted-objects \
  bindhub-metadata:alpha
```

The metadata image also intentionally avoids Docker `VOLUME`. For local SQLite smoke only, pass
`-e BINDHUB_METADATA_DB=/data/bindhub-metadata.sqlite3` and mount `/data` explicitly with Docker or
Railway.

To deploy the legacy metadata API on Railway instead of the product API, create a separate Railway
service and configure it to use `infra/hosted/metadata.Dockerfile`, attach Railway Postgres, set
`DATABASE_URL`, and set server-side object storage env vars. These values stay on the server; users
should never configure R2 endpoints, bucket names, prefixes, or object-access lease ids.
