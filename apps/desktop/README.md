# Devbox Desktop

This is the private-alpha Electron + React + TypeScript shell for Devbox.

The app is a no-network local control surface for the desktop-to-laptop alpha loop. It shows status,
watched projects, sync activity, manual conflict records, devices/pairing state, explicit secret
policy records, and redacted settings. The current bridge returns fixture-backed alpha state so the
app can build and run without cloud credentials, browser login, Docker, Postgres, or production
services.

The desktop app must call the Rust daemon or a narrow local bridge for workspace mutations. It must
never write, delete, restore, or merge project files directly from renderer code.

## Commands

```text
npm install
npm run typecheck
npm run test:safety
npm run build
npm run start
```

`npm run dev` starts the Vite renderer for local UI work. `npm run start` builds the renderer and
Electron main/preload files, then opens the desktop shell.
