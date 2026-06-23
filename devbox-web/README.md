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

The public site content lives in `apps/site/src/pages`. The landing page is
`apps/site/src/pages/index.astro`; docs live under `apps/site/src/pages/docs`;
shared Astro shells live in `apps/site/src/components`; local styling lives in
`apps/site/src/styles/site.css`.

The site waitlist/contact form is UI-only for now. Wire it to an API, CRM, or
mail action before collecting submissions.

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
- `/auth/cli`
- `/auth/sign-out`

Configure the WorkOS sign-in endpoint as
`http://localhost:3000/auth/sign-in` and the redirect URI as
`http://localhost:3000/auth/callback`.

## CLI browser login

`devbox login` starts a short-lived machine login flow with `devbox-api`, opens
`/auth/cli?code=...`, and polls until the browser route approves the machine.
The browser route must verify the WorkOS/AuthKit session first, then it calls
`devbox-api` with `DEVBOX_HOSTED_API_SERVICE_TOKEN`. The browser never receives
or displays the Devbox session token; the CLI stores it locally after polling.

Local deterministic smoke mode is explicit:

```sh
DEVBOX_LOCAL_API_URL=http://127.0.0.1:3001
DEVBOX_HOSTED_API_SERVICE_TOKEN=local-dev-service-token
DEVBOX_LOCAL_DEV_CLI_AUTH=1
DEVBOX_LOCAL_DEV_AUTH_EMAIL=local-dev@example.test
```

This bypass is only for CI/local-dev when live WorkOS is unavailable. Production
must leave `DEVBOX_LOCAL_DEV_CLI_AUTH` unset and use AuthKit.

## Dashboard data

Authenticated dashboard routes are under `/dashboard`. The loaders require a
WorkOS session before reading any account, folder, or machine data.

Dashboard data modes:

```sh
DEVBOX_DASHBOARD_DATA_MODE=hosted-workos
DEVBOX_HOSTED_API_URL=https://api.example.com
DEVBOX_HOSTED_API_SERVICE_TOKEN=
```

Uses the AuthKit-verified WorkOS session on the web server, exchanges it with
the hosted API over a server-to-server service token, then uses the returned
Devbox session token and device id for dashboard API reads. Browser code should
never send WorkOS access tokens directly to `devbox-api`.

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
