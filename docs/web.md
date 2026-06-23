# Bindhub Web Apps

Bindhub has two web apps and one API process in local development.

- `apps/web`: TanStack Start dashboard and authenticated product UI.
- `apps/site`: Astro landing page and docs.
- `bindhub-api`: Rust hosted API used by the dashboard and CLI flows.
- `packages/ui`: shared shadcn/ui primitives.

## Local Ports

- Dashboard: `http://localhost:3000`
- API: `http://127.0.0.1:3001`
- Landing/docs: `http://localhost:3002`

## First Setup

```sh
pnpm install
cp apps/web/.env.example apps/web/.env.local
cp bindhub/.env.example bindhub/.env.local
```

Fill `apps/web/.env.local` with WorkOS values and the API connection:

```sh
WORKOS_CLIENT_ID=client_...
WORKOS_API_KEY=sk_...
WORKOS_COOKIE_PASSWORD=<32+ character secret>
WORKOS_REDIRECT_URI=http://localhost:3000/auth/callback
WORKOS_SIGN_OUT_REDIRECT_URI=http://localhost:3000/
VITE_BINDHUB_DOCS_URL=http://localhost:3002/docs

BINDHUB_DASHBOARD_DATA_MODE=local-dev-fixtures
BINDHUB_LOCAL_API_URL=http://127.0.0.1:3001
BINDHUB_HOSTED_API_SERVICE_TOKEN=<same value as bindhub/.env.local>
```

Fill `bindhub/.env.local` for the API:

```sh
BINDHUB_API_BIND=127.0.0.1:3001
BINDHUB_API_ROOT=.bindhub-api
BINDHUB_API_METADATA_MODE=memory
BINDHUB_API_SERVICE_TOKEN=<local shared service token>
```

## Run Everything

Windows:

```powershell
pnpm dev:stack
```

macOS/Linux:

```sh
./start.sh
```

This starts the API, dashboard, and public site. Add the desktop renderer only
when needed:

```powershell
.\start.ps1 -WithDesktop
```

```sh
./start.sh --with-desktop
```

## Run One App

Dashboard:

```sh
pnpm dev:web
```

Landing/docs:

```sh
pnpm dev:site
```

API:

```sh
pnpm dev:api
```

Desktop renderer:

```sh
pnpm dev:desktop
```

## Auth

The dashboard renders Bindhub-owned sign-in and sign-up UI. WorkOS handles the
identity checks, password verification, Google OAuth exchange, and sealed app
session.

Dashboard auth routes:

- `/auth/sign-in`
- `/auth/sign-up`
- `/auth/google`
- `/auth/callback`
- `/auth/cli`
- `/auth/sign-out`

In WorkOS development settings, configure:

- Sign-in URL: `http://localhost:3000/auth/sign-in`
- Redirect URI: `http://localhost:3000/auth/callback`
- Sign-out redirect: `http://localhost:3000/`

Enable Google OAuth in WorkOS. The dashboard still calls `/auth/google`; WorkOS
owns the provider connection and returns through `/auth/callback`.

## Dashboard Data Modes

Local fixtures:

```sh
BINDHUB_DASHBOARD_DATA_MODE=local-dev-fixtures
```

This is the safest default while UI work is in progress. It still requires a
WorkOS web session, but folder and machine data come from typed fixtures.

Local API:

```sh
BINDHUB_DASHBOARD_DATA_MODE=local-dev-api
BINDHUB_LOCAL_API_URL=http://127.0.0.1:3001
BINDHUB_LOCAL_API_SESSION_TOKEN=<real local session token>
BINDHUB_LOCAL_API_DEVICE_ID=<real local device id>
```

Hosted WorkOS/API:

```sh
BINDHUB_DASHBOARD_DATA_MODE=hosted-workos
BINDHUB_HOSTED_API_URL=https://api.example.com
BINDHUB_HOSTED_API_SERVICE_TOKEN=<server-to-server token>
```

In hosted mode, the web server verifies the WorkOS session, calls the hosted API
with the service token, and receives the Bindhub session token it needs for
dashboard reads. Browser code must not receive the service token.

## CLI Browser Login

`bindhub login` creates a short-lived machine login flow with `bindhub-api`, opens
`/auth/cli?code=...`, and polls until the browser route approves the machine.

Local deterministic CLI auth smoke mode is explicit:

```sh
BINDHUB_LOCAL_API_URL=http://127.0.0.1:3001
BINDHUB_HOSTED_API_SERVICE_TOKEN=<local shared service token>
BINDHUB_LOCAL_DEV_CLI_AUTH=1
BINDHUB_LOCAL_DEV_AUTH_EMAIL=local-dev@example.test
```

Leave `BINDHUB_LOCAL_DEV_CLI_AUTH` unset outside local/CI smoke tests.

## Hosted Deployment Checklist

Deploy the three services separately:

- `apps/site`: static public site and docs.
- `apps/web`: dashboard server.
- `bindhub-api`: Rust API service.

Each service has a Docker path:

```sh
docker build -f infra/hosted/site.Dockerfile -t bindhub-site:staging .
docker build -f infra/hosted/web.Dockerfile -t bindhub-web:staging .
docker build -f infra/hosted/bindhub-api.Dockerfile -t bindhub-api:staging .
```

Run them locally with normal runtime env:

```sh
docker run --rm -p 3002:3002 -e PORT=3002 bindhub-site:staging
docker run --rm -p 3000:3000 --env-file apps/web/.env.local -e PORT=3000 bindhub-web:staging
docker run --rm -p 3001:3001 --env-file bindhub/.env.local -e PORT=3001 bindhub-api:staging
```

The dashboard image does not bake secrets into the image. `WORKOS_*`, `BINDHUB_*`, and `PORT` are
read from the container environment at runtime. The site image currently requires no secrets.

Dashboard hosted env:

```sh
WORKOS_CLIENT_ID=client_...
WORKOS_API_KEY=sk_...
WORKOS_COOKIE_PASSWORD=<32+ character secret>
WORKOS_REDIRECT_URI=https://dashboard.example.com/auth/callback
WORKOS_SIGN_OUT_REDIRECT_URI=https://dashboard.example.com/
VITE_BINDHUB_DOCS_URL=https://www.example.com/docs
BINDHUB_DASHBOARD_DATA_MODE=hosted-workos
BINDHUB_HOSTED_API_URL=https://api.example.com
BINDHUB_HOSTED_API_SERVICE_TOKEN=<same value as API service token>
```

API hosted env:

```sh
BINDHUB_API_BIND=0.0.0.0:<platform port>
BINDHUB_API_METADATA_MODE=<hosted metadata mode>
BINDHUB_API_SERVICE_TOKEN=<same value used by dashboard>
BINDHUB_API_DATABASE_URL=<database URL when not using memory>
BINDHUB_R2_ENDPOINT=<optional blob endpoint>
BINDHUB_R2_BUCKET=<optional blob bucket>
BINDHUB_R2_ACCESS_KEY_ID=<optional blob key id>
BINDHUB_R2_SECRET_ACCESS_KEY=<optional blob secret>
```

Site hosted env:

```sh
PUBLIC_BINDHUB_DASHBOARD_URL=https://app-staging.bindhub.com
```

This is public and build-time only. It drives the static site's dashboard/sign-in
links when the landing/docs site and dashboard are separate services.

Before calling the deployment alpha-ready:

```sh
pnpm build
cargo check -p bindhub-api
```
