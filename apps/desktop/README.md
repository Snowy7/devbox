# Bindhub Desktop

This is the private-alpha Electron + React + TypeScript shell for Bindhub.

The app is a local control surface for the desktop-to-laptop alpha loop. It shows local DB/cache and
folder paths, hosted API/session/folder config, remote kind and shared bucket prefix, object-access
lease state, receiver pairing handoff, live sync command state, manual conflict records, explicit
secret policy records, and redacted settings. The Electron bridge derives state from `BINDHUB_*`
environment variables when present and otherwise falls back to safe placeholders, so it can build and
run without cloud credentials, browser login, Docker, Postgres, or production services.

The desktop app must call the Rust daemon or a narrow local bridge for folder mutations. It must
never write, delete, restore, or merge shared-folder files directly from renderer code.

## Commands

```text
pnpm install
pnpm typecheck
pnpm test:safety
pnpm build
pnpm start
```

`pnpm dev` starts the Vite renderer for local UI work. `pnpm start` builds the renderer and
Electron main/preload files, then opens the desktop shell.

For an unsigned alpha bundle on macOS/Linux:

```text
scripts/package-desktop-alpha.sh v0.1.0-alpha.1
```

The bundle is not a signed installer. It is a runnable Electron control surface for alpha testing:
extract it, run `pnpm install`, then `pnpm start:built`.
