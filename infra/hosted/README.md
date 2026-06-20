# Hosted Deploys

This folder contains hosted/container entrypoints.

The default Railway deploy in [railway.toml](../../railway.toml) now builds `devbox-api`, which is
the MVP product API used by:

```text
devbox login
devbox share <folder>
devbox clone <name>
```

`devbox-metadata` is still kept for legacy alpha metadata/object-access smoke paths.

## Product API On Railway

`devbox-api` stores sessions, devices, shared-folder registry, memberships, and cursors in
Postgres. Loom pack bytes use server-owned R2-compatible object storage when `DEVBOX_R2_ENDPOINT`
and `DEVBOX_R2_BUCKET` are configured. `DEVBOX_API_ROOT` is only a scratch/local-pack fallback path;
it is not the durable product metadata store.

Railway setup:

1. Deploy with the root [railway.toml](../../railway.toml).
2. Attach Railway Postgres and set `DATABASE_URL` on the `devbox-api` service. You can also use
   `DEVBOX_API_DATABASE_URL`; `DATABASE_URL` is the normal Railway path.
3. Configure server-side R2 pack storage when staging should use Cloudflare object storage:
   `DEVBOX_R2_ENDPOINT`, `DEVBOX_R2_BUCKET`, `DEVBOX_R2_ACCESS_KEY_ID`,
   `DEVBOX_R2_SECRET_ACCESS_KEY`, optional `DEVBOX_R2_REGION=auto`, optional
   `DEVBOX_R2_PREFIX`, and optional `DEVBOX_R2_SESSION_TOKEN`.
4. Deploy and confirm `/ready` returns `service: "devbox-api"`, `metadata: "postgres"`, and
   `storage: "r2-packs"` when R2 is active.

Do not attach a Railway Volume for the product API. Durable API metadata lives in Postgres, and pack
bytes live in R2. The Dockerfile intentionally does not include a Docker `VOLUME` instruction.

Local container smoke:

```bash
docker build -f infra/hosted/devbox-api.Dockerfile -t devbox-api:alpha .
docker run --rm -d -p 5432:5432 --name devbox-api-postgres \
  -e POSTGRES_USER=devbox \
  -e POSTGRES_PASSWORD=devbox \
  -e POSTGRES_DB=devbox \
  postgres:16-alpine
docker run --rm -p 8787:8787 \
  -e PORT=8787 \
  -e DATABASE_URL=postgres://devbox:devbox@host.docker.internal:5432/devbox \
  -e DEVBOX_R2_ENDPOINT=https://<cloudflare-account-id>.r2.cloudflarestorage.com \
  -e DEVBOX_R2_BUCKET=devbox-alpha \
  -e DEVBOX_R2_ACCESS_KEY_ID=<server-side-r2-access-key> \
  -e DEVBOX_R2_SECRET_ACCESS_KEY=<server-side-r2-secret-key> \
  devbox-api:alpha
curl http://127.0.0.1:8787/ready
```

These values stay on the server. End-user machines should only know the Devbox API URL and their
session/device state; they should never configure Cloudflare endpoints, bucket names, prefixes, or
R2 credentials.

## Legacy Metadata API

`devbox-metadata` is the deployable compatibility API for the older hosted metadata/object-access
alpha path. It supports Railway/Postgres metadata storage and server-owned R2-compatible object
storage for low-level smoke commands.

Environment:

```bash
DATABASE_URL=${{Postgres.DATABASE_URL}}
DEVBOX_SESSION_TTL_SECONDS=2592000
DEVBOX_PROOF_TTL_SECONDS=7776000
DEVBOX_ALLOW_MOCK_AUTH=false
DEVBOX_R2_ENDPOINT=https://<cloudflare-account-id>.r2.cloudflarestorage.com
DEVBOX_R2_BUCKET=devbox-alpha
DEVBOX_R2_ACCESS_KEY_ID=<server-side-r2-access-key>
DEVBOX_R2_SECRET_ACCESS_KEY=<server-side-r2-secret-key>
# optional:
DEVBOX_R2_REGION=auto
DEVBOX_R2_SESSION_TOKEN=<server-side-r2-session-token>
```

Build locally:

```bash
docker build -f infra/hosted/metadata.Dockerfile -t devbox-metadata:alpha .
docker run --rm -p 8787:8787 \
  -e DATABASE_URL=postgres://devbox:devbox@host.docker.internal:5432/devbox_metadata \
  -e DEVBOX_OBJECT_LOCAL_ROOT=/tmp/devbox-hosted-objects \
  devbox-metadata:alpha
```

The metadata image also intentionally avoids Docker `VOLUME`. For local SQLite smoke only, pass
`-e DEVBOX_METADATA_DB=/data/devbox-metadata.sqlite3` and mount `/data` explicitly with Docker or
Railway.

To deploy the legacy metadata API on Railway instead of the product API, point `railway.toml` at
`infra/hosted/metadata.Dockerfile`, attach Railway Postgres, set `DATABASE_URL`, and set server-side
object storage env vars. These values stay on the server; users should never configure R2 endpoints,
bucket names, prefixes, or object-access lease ids.
