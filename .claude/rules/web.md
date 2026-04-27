# Web (Astro × 2) rules

Path-scoped rules for the `web/` directory — kino's two public
sites. **Two Astro projects, two CF Pages deployments, two
subdomains.** See [`docs/SITE_PLAN.md`](../../docs/SITE_PLAN.md)
for the locked design.

```
web/
├── site/      → kinostack.app   — marketing
├── docs/      → docs.kinostack.app — Starlight docs
└── shared/    → master brand assets (not deployed)
```

Both projects are **independent** — separate `package.json`,
`astro.config.mjs`, `tsconfig.json`, `biome.json`. Don't try to
share Astro config across them.

## Stack

Both projects:
- **Astro 6** (latest)
- **Vite 7** — pinned via direct `vite: ^7.3.2` dep so npm dedup
  doesn't float to Vite 8 (which ships Rolldown and breaks
  `@tailwindcss/vite`). Astro 6.1 itself warns on startup when it
  detects Vite 8.
- **Tailwind v4** via `@tailwindcss/vite`
- **Biome** for lint + format (separate config per project)

`docs/` additionally:
- **Starlight 0.38** for sidebar / search / theme switcher
- **Pagefind** for client-side search (built into Starlight)

## Quality gates

```sh
# Marketing
cd web/site
npm run ci       # canonical recipe — lint + build (matches CI)

# Docs
cd web/docs
npm run ci       # canonical recipe — lint + build (matches CI)
```

`npm run ci` = `lint && build`. Biome warnings (a11y, etc.) don't
fail; only errors do.

## Adding an npm dep

The `kino-web` container mounts `node_modules` as a named volume
per project (`web-site-node-modules`, `web-docs-node-modules`) so
each project holds platform-correct binaries (sharp, swc) regardless
of host OS. A host-side `npm install` writes to the host filesystem
that's hidden by the container's volume mount — the dev server
inside the container doesn't see it.

After adding a dep to `web/site/package.json` or `web/docs/package.json`:

```sh
cd backend
just web-install            # install both projects in the container
just web-install site       # only web/site
just web-install docs       # only web/docs
just restart-web            # picks up the new deps in the dev server
```

If the dev server overlay shows `Can't resolve '@<package>'`, the
host has the package but the container doesn't — `just web-install`
fixes it.

## Layout

### `web/site/`

```
site/
├── astro.config.mjs       # Astro + Tailwind (NO Starlight)
├── biome.json
├── package.json
├── tsconfig.json
├── public/                # favicons, OG images, /cast/receiver.html, /brand/*
└── src/
    ├── pages/             # 1 file per route (index.astro, features.astro, …)
    ├── layouts/           # MarketingLayout.astro
    ├── components/        # Astro components shared across pages
    └── styles/            # global.css (Tailwind + brand tokens)
```

### `web/docs/`

```
docs/
├── astro.config.mjs       # Astro + Starlight + Tailwind
├── biome.json
├── package.json
├── tsconfig.json
├── public/                # favicons, screenshots, /brand/*
└── src/
    ├── assets/wordmark.svg # Starlight logo (Vite-imported)
    ├── content.config.ts   # Starlight schema binding
    ├── content/docs/       # Markdown/MDX content collection
    │   ├── getting-started/
    │   ├── setup/
    │   ├── integrations/
    │   ├── features/
    │   ├── reference/
    │   └── troubleshooting/
    └── styles/global.css   # Tailwind + brand tokens (duplicate of site/)
```

## Conventions

- **`.astro` files** — server-rendered + hydratable; no `console.log` outside `client:*` islands
- **Tailwind v4** — utility-first; CSS custom-properties for brand tokens in `src/styles/global.css`
- **Starlight content** — `web/docs/src/content/docs/*.{md,mdx,astro}`; Starlight schema validates frontmatter (see `content.config.ts`)
- **Images** — drop into `src/assets/` and import for Astro's sharp pipeline; don't commit unoptimised originals to `public/`
- **No JavaScript-heavy pages** — both sites are static; `client:load` / `client:visible` islands only when genuinely needed
- **Cross-site links** — marketing → docs uses the absolute URL `https://docs.kinostack.app/...` (different project, different domain). Same for in-app `<HelpLink>` (when wired)

## What lives elsewhere

- **`frontend/`** is kino's in-app React SPA — different stack, different biome config, different ownership
- **`docs/` (top-level)** is the maintainer-facing spec/ADR/runbook tree — read on GitHub, not built into either site
- **`web/shared/`** is master copies of brand assets — both projects keep their own `public/brand/` copy (manual sync for v1)

## Common biome lints to pre-empt

`npm run lint` treats errors as fatal in CI; warnings allowed. Same patterns as the in-app SPA (`.claude/rules/frontend.md`):

- **Suppression comment positioning** — `// biome-ignore lint/<rule>: reason` MUST be on the line directly preceding the offending JSX element. Multi-line comment blocks where only the first line carries the directive are silently ignored
- **Stale `// biome-ignore`** suppressions become `suppressions/unused` warnings when biome renames a rule

## Astro / vendored Vite quirk

The Astro vendored vite version lags `web/`'s direct vite dep; the
`tailwindcss()` plugin's `Plugin<any>` type doesn't satisfy Astro's
vendored Vite type. Cast via `/** @type {any} */` in
`astro.config.mjs` — runtime safe (tailwind's CI tests the plugin
surface). Same fix in both projects.

## Deployment

- **`site/`** → Cloudflare Pages project `kino-site` → `kinostack.app`
- **`docs/`** → Cloudflare Pages project `kino-docs` → `docs.kinostack.app`
- Both built per push via CF Pages' GitHub integration. Per-PR
  previews on `<hash>.kino-site.pages.dev` and
  `<hash>.kino-docs.pages.dev`
- See [`docs/SITE_PLAN.md`](../../docs/SITE_PLAN.md) §9

## When adding content

- **New marketing page** → `web/site/src/pages/foo.astro` using `MarketingLayout`
- **New doc page** → `web/docs/src/content/docs/<section>/foo.md` (Starlight auto-routes; sidebar regenerates from the directory tree per buckets in `astro.config.mjs`)
- **New brand asset** → `web/shared/brand/`, then copy into both `site/public/brand/` and `docs/public/brand/` (see `web/shared/README.md`)
- Run `npm run dev` for live preview (4321 — only one site at a time on that port)
