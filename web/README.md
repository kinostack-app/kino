# web/

Public-facing kino websites. Two Astro projects, two Cloudflare
Pages deployments, two subdomains.

| Path | Site | Domain | Stack |
|---|---|---|---|
| [`site/`](./site/) | Marketing — landing, download, releases, about, etc. | `kinostack.app` | Astro 6 + Tailwind v4 |
| [`docs/`](./docs/) | Docs — install guides, setup, integrations, reference, troubleshooting | `docs.kinostack.app` | Astro 6 + Starlight 0.38 + Tailwind v4 |
| [`shared/`](./shared/) | Brand tokens + master logo/icon copies | — | (not deployed) |

The split keeps the marketing voice and the docs voice physically separate. See
[`../docs/SITE_PLAN.md`](../docs/SITE_PLAN.md) for the locked
design — IA, page set, releases-page implementation, deploy
strategy, etc.

The maintainer-facing `../docs/` tree (subsystems, architecture,
ADRs, runbooks) is **not** built by either of these projects —
those stay markdown-only and are read in-place on GitHub.

## Commands

Each project is independent:

```bash
# Marketing site
cd web/site
npm install
npm run dev          # http://localhost:4321 (when standalone)
npm run build        # static build → dist/
npm run lint
npm run ci           # lint + build (canonical recipe — same as CI)

# Docs site
cd web/docs
npm install
npm run dev          # http://localhost:4321 (when standalone)
npm run build
npm run lint
npm run ci
```

In the dev container both sites run together — marketing on
`localhost:4321`, docs on `localhost:4322` — under the single
`kino-web` container.

### Adding an npm dep

Both projects mount `node_modules` as named docker volumes
(`web-site-node-modules`, `web-docs-node-modules`) so the container
holds platform-correct binaries (sharp, swc) independent of the host
OS. This keeps the dev container portable across macOS / Windows /
Linux contributors.

Side effect: a host-side `npm install` doesn't reach the container.
After adding a dep, run from the repo root:

```sh
cd backend
just web-install            # install both projects in the container
just web-install site       # only web/site
just web-install docs       # only web/docs
just restart-web            # dev server re-optimises
```

If the dev-server error overlay shows `Can't resolve '@<package>'`,
that's the symptom — `just web-install` fixes it.

## Cast receiver

`site/public/cast/receiver.html` is the Custom Web Receiver
registered with Google Cast Console as App ID `407178D2`. The
URL `https://kinostack.app/cast/receiver.html` is pinned against
that App ID — it must continue to be served verbatim from the
marketing site root forever (or until we re-register a new App
ID, which would force every cast-capable client to update).
See `../docs/subsystems/11-cast.md` and
`../docs/subsystems/32-cast-sender.md`.

## Cloudflare Pages — two projects

| CF Pages project | Build root | Build cmd | Output | Custom domain |
|---|---|---|---|---|
| `kino-site` | `web/site` | `npm run build` | `dist` | `kinostack.app`, `www.kinostack.app` |
| `kino-docs` | `web/docs` | `npm run build` | `dist` | `docs.kinostack.app` |

Per-PR previews land at `<hash>.kino-site.pages.dev` and
`<hash>.kino-docs.pages.dev`. See
[`../docs/SITE_PLAN.md`](../docs/SITE_PLAN.md) §9 for the deploy
checklist.

## Brand assets

Master copies live in `shared/brand/`. Each project keeps its own
copy in `public/brand/` (synced manually for v1; can be replaced
with a build-step copy or symlink if drift becomes an issue).

## Adding pages

- **Marketing page** → Astro file under `web/site/src/pages/`,
  using `@/layouts/MarketingLayout.astro`.
- **Docs page** → Markdown / MDX under
  `web/docs/src/content/docs/<section>/`. Starlight picks it up
  automatically; the sidebar autogenerates from the directory tree
  per the buckets configured in `web/docs/astro.config.mjs`.
- **Static asset** (cast receiver, install scripts, etc.) → drop
  under the appropriate project's `public/` and reference by
  absolute URL.
