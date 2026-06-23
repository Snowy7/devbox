# Devbox Site

This Astro app owns the public Devbox site and lightweight docs.

## Structure

- `src/pages/index.astro` is the first public landing page.
- `src/pages/docs/` contains the docs shell and alpha pages.
- `src/components/` contains shared Astro layout components.
- `src/styles/site.css` contains the local site styles.
- `public/` contains favicons and static social or product images.

## Commands

Run from `devbox-web`:

```sh
pnpm --filter apps-site dev
pnpm --filter apps-site build
pnpm --filter apps-site preview
pnpm --filter apps-site format
```

## Content Boundaries

The public site should stay centered on folders, machines, trust, and developer
continuity. Say "folder" in user-facing copy unless a page is explicitly
describing external repo or package details.

The waitlist/contact form is UI-only in this PR. Wire it to an API, CRM, or mail
action before collecting submissions.

## Astro Notes

Astro exposes pages from `src/pages/` by file path. Static assets in `public/`
are served from the site root.

There is no site-specific Astro typecheck task yet. The workspace
`pnpm typecheck` command currently covers the TypeScript dashboard and shared UI
packages.
