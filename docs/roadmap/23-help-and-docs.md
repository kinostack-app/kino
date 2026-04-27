# Help and documentation

> **Status (2026-04-27): docs site shipped; in-app anchors pending.**
> `web/docs/` is a fully scaffolded Starlight site covering
> getting-started, setup, integrations, features, reference, and
> troubleshooting (~36 pages). Cloudflare Pages target
> `docs.kinostack.app` is wired (see `web.md`). **Outstanding:**
> in-app `<HelpLink>` component + `/help` route in the React SPA —
> tracked as Phase 6 of the site rebuild.

How end-user documentation is structured, hosted, and linked from the app. Splits cleanly into three surfaces: a public docs site, small in-app anchors to that site, and the maintainer-facing subsystem specs we already keep in this directory.

## Philosophy

**If a setting needs a paragraph to explain, the setting is wrong.** Most of Kino's UX should be self-evident — clear labels, sensible defaults, progressive disclosure. Help pages aren't a substitute for UX. They exist for the things no amount of UI polish can make obvious:

- Concepts (what Kino does vs doesn't do, how the acquire pipeline flows)
- Complex-by-nature features (VPN setup, Trakt device-code auth, list URL formats)
- Troubleshooting (why a download stalled, what an error means)
- Platform-specific install and service management

Everything else — the day-to-day UX — shouldn't need a help page. If we find ourselves writing one, the signal is "fix the UI", not "document the workaround".

## Scope

**In scope:**
- A public documentation site (`docs.kinostack.app` or equivalent) built from Markdown in the repo
- Small `?` icon anchors from specific in-app surfaces to specific docs sections
- An in-app `/help` page that's a link farm + version info, not an embedded docs viewer
- Per-release doc versioning (latest stable + previous versions accessible)
- Contribution flow for community-authored doc PRs

**Out of scope:**
- Embedded Markdown rendering inside the Kino app — users read docs in a browser tab
- In-app full-text search over documentation
- Chatbot / AI help surface
- Tooltip novels on every setting (sign of bad UX, not a useful doc feature)
- Translated docs at v1 (English-only; i18n is a later concern)
- Auto-generated docs from schema or OpenAPI — shallow content, skip

## Three surfaces, clean separation

| Surface | Audience | Location | Format |
|---|---|---|---|
| **Public docs site** | End users | `docs.kinostack.app` | Markdown via Starlight |
| **In-app `?` anchors** + `/help` page | End users in context | Inside the app | Links out to the docs site |
| **Maintainer subsystem specs** | Kino contributors + us | `docs/subsystems/` (unchanged — this directory) | Not published; dev-facing only |

The current `docs/subsystems/` content stays internal. It's design specs, not user documentation, and the audience is people building Kino. Publishing these files would confuse users.

## Tool choice: Starlight

[Starlight](https://starlight.astro.build/) (Astro-based) is the pick over VitePress / Docusaurus / mdBook.

Reasons:

- Best-in-class default theme out of the box — clean typography, dark mode, responsive, accessible
- Built-in full-text search via Pagefind (indexed at build time, runs entirely client-side, no backend)
- Multi-version sites supported via subpaths (`docs.kinostack.app` vs `docs.kinostack.app/v0.4`)
- Fast static builds, cheap to host (Cloudflare Pages / GitHub Pages)
- Takes Markdown files directly — no special syntax required for contributors
- Astro's component model is there if we ever want interactive examples, but nothing requires using it

Alternatives considered:

- **VitePress** — Vue/Vite native. Docs aren't coupled to the Vite/React app code so the alignment argument is weak. Starlight's defaults are better.
- **Docusaurus** — React-based, heavier, more plugins to manage.
- **mdBook** — Rust affinity, but weaker theme and navigation for a product docs site.

## Directory layout

```
docs/
├── site/                   ← published to docs.kinostack.app (new, this doc)
│   ├── astro.config.mjs
│   ├── package.json
│   ├── public/
│   └── src/
│       └── content/
│           └── docs/
│               ├── index.md            ← landing page
│               ├── quickstart/
│               │   ├── linux.md
│               │   ├── macos.md
│               │   ├── windows.md
│               │   └── raspberry-pi.md
│               ├── setup/
│               │   ├── first-run.md
│               │   ├── indexers.md
│               │   ├── vpn.md
│               │   └── quality-profiles.md
│               ├── integrations/
│               │   ├── trakt.md
│               │   └── lists.md
│               ├── features/
│               │   ├── intro-skip.md
│               │   ├── home-customisation.md
│               │   └── backup-restore.md
│               ├── troubleshooting/
│               │   ├── downloads.md
│               │   ├── playback.md
│               │   ├── vpn.md
│               │   └── index.md
│               ├── faq.md
│               └── release-notes/
│                   ├── v0.5.md
│                   └── v0.4.md
│
└── subsystems/             ← unchanged — maintainer specs, not published
    ├── 01-schema.md
    ├── ...
    └── 23-help-and-docs.md (this file)
```

`docs/site/` is the source of truth for the public site. `docs/subsystems/` stays in place as maintainer-facing specs. Both committed to the same repo; separate build pipelines.

## Initial content scope

Ten pages at launch. Reflects "opinionated, fewer knobs, less to document".

| Page | Purpose | Primary source |
|---|---|---|
| **Quickstart (per platform)** | Install → setup → first download. Tabs for Linux / macOS / Windows / Pi | `21-cross-platform-deployment.md` (user-facing extract) |
| **Installation** | Detailed install + service management + uninstall | `21-cross-platform-deployment.md` |
| **First-run setup** | Wizard walkthrough with screenshots | Wizard implementation + `16/17/19` onboarding notes |
| **VPN configuration** | ProtonVPN walkthrough, BYO WireGuard, troubleshooting | `03-download.md` + VPN section of `21` |
| **Indexers** | Torznab setup, Prowlarr integration, built-in Cardigann | `02-search.md` + `14-indexer-engine.md` |
| **Trakt** | Connect flow, what syncs, dry-run, disconnect | `16-trakt.md` |
| **Lists** | Add by URL, soft-cap, pinning | `17-lists.md` |
| **Intro skipping** | How it works, when it kicks in, accuracy caveats | `15-intro-skipper.md` |
| **Home customisation** | Customise drawer, pinning, Up Next explainer | `18-ui-customisation.md` |
| **Backup & restore** | Automatic backups, manual creation, restore flow | `19-backup-restore.md` |
| **Troubleshooting** | Download stalled / import failed / playback won't start / VPN won't connect — FAQ-style | Various subsystems |
| **FAQ** | "Why no multi-user?", "Can I run this without a VPN?", "Does it work on a Pi?" | Compiled from user questions |
| **Release notes** | Per-version changelog, upgrade notes | Generated from commits + manual polish |

The user-facing pages **summarise** what's in the subsystem specs — they don't mirror them. Subsystem docs have implementation detail users don't need; user docs have phrasing and screenshots that would be noise in a spec.

## In-app anchors

### `?` icons

Placed sparingly, only next to things that genuinely need context. Not next to every setting.

| Location | Anchor target |
|---|---|
| Settings → VPN page | `docs.kinostack.app/setup/vpn` |
| Settings → Indexers page | `docs.kinostack.app/setup/indexers` |
| Settings → Integrations → Trakt (disconnected pitch card) | `docs.kinostack.app/integrations/trakt` |
| `/lists` page → "Add list" modal | `docs.kinostack.app/integrations/lists` |
| Settings → Quality profiles | `docs.kinostack.app/setup/quality-profiles` |
| Settings → Backup page | `docs.kinostack.app/features/backup-restore` |
| Any health-dashboard panel in degraded/critical state | `docs.kinostack.app/troubleshooting/{panel}` |

Click behaviour: opens the anchored URL in a new tab. No in-app modal, no embedded viewer.

Pattern in code: small `<HelpLink to="/setup/vpn" />` component rendered inline with the setting label. Resolves `/setup/vpn` → `${DOCS_BASE_URL}/setup/vpn` at render time.

### `/help` page

Single page inside the app at the route `/help`. Doesn't render docs content; it's a landing page:

```
Help
────────────────────────────────────────
Getting started
  → Quickstart guide
  → Installation
  → First-time setup

Features
  → Downloading & importing
  → Trakt integration
  → Following curated lists
  → Home customisation
  → Backup & restore

Troubleshooting
  → Common issues
  → VPN problems
  → Playback issues
  → FAQ

────────────────────────────────────────
Version 0.4.2   [Check for updates]
Report a problem  [→ GitHub Issues]
```

Each "→" opens the corresponding external docs page in a new tab. `Check for updates` hits the GitHub Releases API and surfaces whether a newer version is available (no auto-update mechanism v1, just a nudge).

Zero embedded Markdown. Zero in-app search. Minimal maintenance — the list of links lives as a static config, not generated.

## Versioning

Starlight supports multi-version sites via subpaths:

- `docs.kinostack.app` → latest stable release
- `docs.kinostack.app/v0.4` → docs for v0.4 (preserved after 0.5 ships)
- `docs.kinostack.app/v0.3` → etc.

A banner on non-latest versions: *"This documentation is for Kino 0.4. [View latest →]"*

### CI and publication

Docs site builds on every push to `main` (for a draft preview) and on every release tag (promoted to the live stable URL). Two actions:

- **`docs-preview.yml`** — builds the site on every push/PR, deploys to a preview URL (Cloudflare Pages / Netlify deploy previews)
- **`docs-publish.yml`** — triggers on release tag, builds, deploys to production at `docs.kinostack.app`

Older versions aren't rebuilt — once `/v0.4` is published, it's frozen. Corrections to historical docs happen by bumping the patch version (`v0.4.1`) and re-deploying that subpath, not by editing in place.

Hosting: **Cloudflare Pages** (free, unlimited bandwidth, Cloudflare R2 cache) or **GitHub Pages** (free, simpler, slightly less polished). Cloudflare Pages is the pick — free, fast, and won't hit the GitHub Pages bandwidth limits if the docs get popular.

## Contribution flow

Docs PRs should have lower friction than code PRs:

- No Rust toolchain required — Markdown files only
- No tests to pass — only the site build (checks that all internal links resolve and Markdown parses)
- Preview deploys on every PR so reviewer + author can see the rendered result
- First-time-contributor-friendly: clear `CONTRIBUTING_DOCS.md` with "how to edit docs locally" + "what makes a good docs PR"
- Typo / small-fix PRs accepted liberally

Maintainer review is lighter than on code — docs don't cause runtime failures, so approvals come faster.

## In-app configuration

A single Config field:

| Column | Type | Default |
|---|---|---|
| docs_base_url | TEXT | `https://docs.kinostack.app` |

Override for users running air-gapped installs who mirror the docs locally. Rare case, documented as an escape hatch.

## Entities touched

- **Reads:** Config (`docs_base_url` for link rendering)
- **Writes:** none

No new tables. The help subsystem is a configuration + UI layer; all content lives as Markdown in the repo.

## Dependencies

- **Starlight** + **Astro** — docs site framework
- **Pagefind** — built-in search (ships with Starlight)
- **Cloudflare Pages** (or GitHub Pages) — hosting
- Existing GitHub Actions CI for build + deploy

No new Kino runtime dependencies. The app's only addition is a `<HelpLink>` React component and a `/help` route.

## Error states

- **Broken link in docs** → caught by the build step (Starlight validates internal links); PR fails until fixed
- **Anchor target missing** (in-app `?` points at a docs page that doesn't exist) → the docs page 404s gracefully via Starlight's default 404 page, with a link back to the index
- **User's network blocks external docs** → in-app help degrades gracefully; the `?` links still work offline if the user has mirrored docs (via `docs_base_url` override) but otherwise the tab fails to load with the browser's normal offline indicator
- **Release note for a version not yet written** → page shows placeholder "Release notes for this version are being prepared" rather than a raw 404

## Known limitations

- **English only in v1.** Starlight supports i18n out of the box, but translation adds content-maintenance cost we don't want early on. Revisit if user base grows internationally.
- **No offline docs bundle.** Users running fully air-gapped need to mirror `docs.kinostack.app` themselves and point `docs_base_url` at the mirror. No tooling shipped to automate this; documented as a known pattern.
- **Manual sync between subsystem specs and public docs.** A change in `16-trakt.md` doesn't auto-propagate to `docs/site/src/content/docs/integrations/trakt.md`. Maintainers must update both when behaviour changes. Acceptable — the docs have a different voice and scope than the specs; auto-sync would produce bad user docs.
- **Versioned docs don't back-port fixes.** If we find a typo in `/v0.4`, we don't re-deploy `v0.4`; the typo stays until the user upgrades to a version where it's fixed. Trade-off for the frozen-once-published simplicity.
- **Preview deploys may leak pre-release content.** PR preview URLs are unauthenticated. Not a secret-leakage risk because nothing secret goes in docs, but worth knowing that draft doc changes are briefly public via the preview URL.
- **In-app `/help` page is static.** Contextual content ("help for the thing you were just doing") isn't generated; users navigate from topic headings. Acceptable — contextual `?` icons handle the per-page case.
