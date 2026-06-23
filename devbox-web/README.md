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
