# Devbox Web

This is the web foundation for Devbox.

- `apps/web` is the TanStack Start dashboard app.
- `apps/site` is the Astro public site for landing pages and docs.
- `packages/ui` contains shared shadcn/ui primitives.

The dashboard is wired for WorkOS/AuthKit as the authentication direction. Copy
`.env.example` to `.env.local` before building real auth flows.

```sh
pnpm install
pnpm dev
```

## Apps

```sh
pnpm --filter web dev
pnpm --filter apps-site dev
```

## Auth

WorkOS environment variables:

```sh
WORKOS_CLIENT_ID=
WORKOS_API_KEY=
WORKOS_COOKIE_PASSWORD=
WORKOS_REDIRECT_URI=http://localhost:3000/auth/callback
WORKOS_SIGN_OUT_REDIRECT_URI=http://localhost:3000/
```

The app owns these auth routes:

- `/auth/sign-in`
- `/auth/sign-up`
- `/auth/callback`
- `/auth/sign-out`

Configure the WorkOS sign-in endpoint as
`http://localhost:3000/auth/sign-in` and the redirect URI as
`http://localhost:3000/auth/callback`.

## Dashboard data

Authenticated dashboard routes are under `/dashboard`. The loaders require a
WorkOS session before reading any account, folder, or machine data.

Dashboard data modes:

```sh
DEVBOX_DASHBOARD_DATA_MODE=hosted-workos
DEVBOX_HOSTED_API_URL=https://api.example.com
```

Uses the WorkOS access token as the hosted API bearer token.

```sh
DEVBOX_DASHBOARD_DATA_MODE=local-dev-api
DEVBOX_LOCAL_API_URL=http://127.0.0.1:3001
DEVBOX_LOCAL_API_SESSION_TOKEN=
DEVBOX_LOCAL_API_DEVICE_ID=
```

Uses a real local `devbox-api` session token and device id. Create one with the
local API's `/v1/auth/dev-session` endpoint; do not put arbitrary account ids in
web app code.

```sh
DEVBOX_DASHBOARD_DATA_MODE=local-dev-fixtures
```

Uses typed local-dev fixtures after WorkOS authentication. This is the default
outside production so the product shell can run before hosted API credentials
exist.
