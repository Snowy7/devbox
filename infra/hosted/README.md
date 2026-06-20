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

`devbox-api` stores sessions, devices, shared-folder registry, and cursors under `DEVBOX_API_ROOT`.
The Docker image defaults that root to `/data/devbox-api`. Loom pack bytes use local files by
default, but switch to server-owned R2-compatible object storage when `DEVBOX_R2_ENDPOINT` and
`DEVBOX_R2_BUCKET` are configured.

Railway setup:

1. Deploy with the root [railway.toml](../../railway.toml).
2. Configure a Railway Volume mounted at `/data` so account/session/folder/cursor metadata persists
   across redeploys.
3. Configure server-side R2 pack storage when staging should use Cloudflare object storage:
   `DEVBOX_R2_ENDPOINT`, `DEVBOX_R2_BUCKET`, `DEVBOX_R2_ACCESS_KEY_ID`,
   `DEVBOX_R2_SECRET_ACCESS_KEY`, optional `DEVBOX_R2_REGION=auto`, optional
   `DEVBOX_R2_PREFIX`, and optional `DEVBOX_R2_SESSION_TOKEN`.
4. Deploy and confirm `/ready` returns `service: "devbox-api"` and `storage: "r2-packs"` when R2 is
   active.

The Dockerfile intentionally does not include a Docker `VOLUME` instruction. Railway rejects
Dockerfile-declared volumes; create Railway Volumes in Railway instead.

Local container smoke:

```bash
docker build -f infra/hosted/devbox-api.Dockerfile -t devbox-api:alpha .
docker run --rm -p 8787:8787 -e PORT=8787 devbox-api:alpha
curl http://127.0.0.1:8787/ready
```

`DATABASE_URL` is not consumed by `devbox-api` yet. It belongs to the legacy metadata service below
until the product API metadata layer is moved from `/data` files to Postgres.

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
