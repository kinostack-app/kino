# Site plan

Single source of truth for the kino public website. Supersedes the
implementation parts of
[`roadmap/23-help-and-docs.md`](./roadmap/23-help-and-docs.md).

**Status:** decided 2026-04-27, partial implementation in `web/`.

---

## 1. Locked decisions

| Decision | Resolution | Why |
|---|---|---|
| **License** | **GPL-3.0-or-later** | Standard for self-hosted media-server projects in this category. |
| **Marketing domain** | `kinostack.app` | The domain we own. |
| **Docs domain** | `docs.kinostack.app` | Subdomain split keeps the marketing voice and the docs voice physically separate. Eradicate `kino.sh` everywhere — that domain is not ours. |
| **Project layout** | Two Astro projects: `web/site/` + `web/docs/` + `web/shared/` | Cleanest URL story (`docs.kinostack.app/foo` IS the URL the user sees, not a redirect). Independent deploy cycles. |
| **Hosting** | Cloudflare Pages × 2 projects | One CF Pages project per Astro project. Both behind the same CF zone. |
| **Compare/vs pages** | None for v1 | Don't position as a competitor to peer OSS by name. The hero "one binary instead of seven" implies the category-replacement story without naming. |
| **Live demo** | None for v1 — screenshots only | Hosting a public read-only kino instance is real ops work. Defer; revisit post-launch. |
| **Roadmap surface** | `/roadmap` page on the marketing site, placeholder for v1 | Public roadmap commits us to keeping it fresh. v1 = placeholder; populate properly post-launch. **Not** linked to the internal `docs/roadmap/` folder — that stays private maintainer-facing content. |
| **Releases handling** | Build-time fetch from GitHub Releases API → custom Astro page | Beautiful + functional. No runtime API calls. OS-grouped download buttons. Detail in §6. |
| **Wording posture** | "Media automation" / "personal media server" | Never "torrent" prominently. Never name peer projects. Frame as automation — emphasis on the user choosing what content to acquire. |
| **Telemetry** | None on either site beyond Cloudflare Web Analytics (cookieless) | Mirrors the in-app no-telemetry commitment ([ADR 0008](./decisions/0008-privacy-posture.md)). |

---

## 2. Information architecture

### Marketing site — `kinostack.app`

**Top nav** (5 items + GitHub):

```
Logo · Features · Docs↗ · Download · Releases · Roadmap · GitHub↗
```

`Docs` and `GitHub` are external (other subdomain / external repo) so they get an arrow indicator.

**Footer** (4 columns):

```
Product            Docs              Community          Project
─ Features         ─ Get started     ─ GitHub           ─ About
─ Download         ─ Configuration   ─ Discussions      ─ Brand
─ Releases         ─ Reference       ─ Contributing     ─ Privacy
─ Roadmap          ─ Troubleshooting ─ Sponsor          ─ License
```

Plus a license attribution line at the very bottom: `kino · GPL-3.0-or-later · MIT for the kinostack.app site source`.

**Pages** (real, not stubs):

| Route | Status target for v1 | Notes |
|---|---|---|
| `/` | Real | Hero, screenshots, feature grid, call-out, install CTA |
| `/features` | Real | Long-form feature tour, screenshots per pillar |
| `/download` | Real | Per-platform install matrix (Docker first, then Linux distros, macOS, Windows, Pi appliance). Links to docs install pages for detail. |
| `/releases` | Real | Built from GH API at build time — see §6 |
| `/roadmap` | Placeholder | "What's coming" with 3-5 broad themes; full content post-launch |
| `/about` | Real | Project values, no-telemetry commitment, GPL stance, single maintainer |
| `/brand` | Real | Logo + wordmark + colors for press / community use |
| `/privacy` | Real | Cookieless analytics statement, what the site collects (nothing) |
| `/community` | Real | GitHub Discussions / sponsor / contribution links |
| `/attributions` | Real | TMDB / Trakt / OpenSubtitles / FFmpeg integration credits + dep license summary (see [`subsystem 24`](./roadmap/24-attributions.md)) |

**Pages explicitly NOT for v1**:
- `/blog` (no content yet, don't ship empty)
- `/compare/*` (no peer-naming per decision)
- `/api` (OpenAPI explorer — defer until first real release)
- `/install.sh` + `/install.ps1` (one-line installers — defer)
- Live demo

### Docs site — `docs.kinostack.app`

**Top nav**: Starlight default chrome (sidebar + search + theme toggle + GitHub link).

**Sidebar** (5 buckets, uv's taxonomy):

```
Getting started
─ Quickstart
─ Install on Linux
─ Install on macOS
─ Install on Windows
─ Install on Raspberry Pi
─ Install with Docker
─ First-run setup

Setup
─ Indexers
─ Quality profiles
─ VPN
─ Storage layout
─ Reverse proxy

Integrations
─ Trakt
─ Lists (TMDB / MDBList)
─ OpenSubtitles
─ Notifications & webhooks

Features
─ Library + automation
─ Streaming + transcode
─ Casting (Chromecast)
─ Intro skipping
─ Backup & restore
─ Home customisation

Reference
─ Configuration
─ CLI flags
─ Environment variables
─ API (OpenAPI)
─ Diagnostic bundle

Troubleshooting
─ Common issues
─ FAQ
```

Built today (`web/docs/src/content/docs/`):
- ✅ `getting-started/` (5 install pages + index)
- ✅ `troubleshooting/` (index + faq)

Missing (priority order for v1):
- ⛔ `getting-started/quickstart` (the umbrella "go from zero to first-download" page)
- ⛔ `getting-started/first-run-setup` (wizard walkthrough)
- ⛔ `setup/{indexers,vpn,quality-profiles,storage,reverse-proxy}`
- ⛔ `integrations/{trakt,lists,opensubtitles,notifications}`
- ⛔ `features/{library,streaming,casting,intro-skipping,backup-restore,home-customisation}`
- ⛔ `reference/{configuration,cli,env-vars,api,diagnostic-bundle}`

The missing pages are content work, not infra work. Most can be drafted by extracting + rephrasing the relevant `docs/subsystems/*.md` (which is maintainer-facing — needs voice + scope conversion). Tractable as a content sprint pre-launch.

---

## 3. Per-page brief

### `/` (home)

**Above the fold**: tagline + 1-line subtitle + 2 CTAs + ONE proof artefact (real screenshot).

```
One binary. The whole media stack.

Self-hosted media automation and streaming, in a single Rust binary.
Discover, acquire, organise, transcode, stream, and cast — without a
docker-compose file the size of a novel.

[ Install ]   [ View on GitHub ]

[ ─ Hero screenshot of the library view ─ ]
```

**Below the fold** (in order):

1. "Why one binary" — three cards (Single binary / No telemetry / Built-in privacy networking) — already drafted, keep
2. Feature tour — 3-4 alternating screenshot+text rows (Library / Downloads / Streaming / Casting)
3. "What's in the box" — bullet list of replaced concerns (no peer-naming)
4. Install one-liner — install snippet (when one-liner installers exist; until then, `[See install options]` button)
5. Footer

### `/features`

Long-form pillar pages. Roughly:

```
Library → Discover, follow, organise. (Screenshots: home, search, follow flow)
Downloads → Indexers, quality profiles, VPN integration.
Streaming → Direct play, hardware transcode, mobile-friendly UI.
Casting → Chromecast support out of the box.
Backup & restore → Automatic snapshots, one-click restore.
Privacy → BoringTun WireGuard, IP leak detection, killswitch.
```

Each pillar = a real screenshot + a paragraph + a "learn more" link to the relevant docs page.

### `/download`

Platform matrix:

```
┌─ Recommended ──────────────────────────┐
│  Docker / docker-compose               │
│  [docker-compose.yml]   [docs ↗]       │
└────────────────────────────────────────┘

Linux
┌─ .deb (Debian / Ubuntu)  [download] ┐
┌─ .rpm (Fedora / RHEL)    [download] ┐
┌─ AppImage (any glibc)    [download] ┐
┌─ AUR (Arch)              [docs ↗]   ┐

macOS
┌─ Homebrew                [docs ↗]   ┐
┌─ .pkg (Apple Silicon)    [download] ┐
┌─ .pkg (Intel)            [download] ┐

Windows
┌─ winget                  [docs ↗]   ┐
┌─ MSI                     [download] ┐

Raspberry Pi
┌─ kino-rpi-arm64.img.xz   [download] ┐
```

Each download button hits the latest GitHub Release asset URL (resolved at build time, same plumbing as the releases page). "[docs ↗]" links to the relevant `docs.kinostack.app/getting-started/install-*` page for the multi-step flows.

### `/releases`

See §6 below — implementation has its own section.

### `/roadmap`

Placeholder for v1:

```
What's coming

We're focused on getting kino's first stable release out the door.
After that, the roadmap broadly covers:

• [ ] Native mobile clients (iOS / Android)
• [ ] Multi-user profiles
• [ ] Live TV / DVR
• [ ] More integrations (Last.fm scrobbling, Calibre library import)

Want something not on the list? Open a discussion on GitHub.
```

Post-launch, this becomes a real roadmap page sourced from a curated subset of `docs/roadmap/` content (manually surfaced, not auto-imported — internal roadmap notes have rough wording).

### `/about`

```
About kino

Why this exists.
What it doesn't do (no telemetry, no SaaS, no upsells).
Who's behind it (single maintainer, UK-based).
How it's funded (sponsorships welcome, no commercial tier ever).
License (GPL-3.0-or-later) + brief explainer of what that means for users.
```

### `/brand`

Logo / wordmark / app icon downloads + color palette + usage guidelines (don't tint, don't recolor, etc.). One short page; this is for community / press / packagers reusing the mark.

### `/privacy`

```
What this site collects: nothing personally identifying. Cloudflare's
cookieless Web Analytics counts page views. No third-party scripts,
no tracking pixels, no cross-site identifiers.

What kino itself collects: nothing. The application makes no outbound
requests except to services you explicitly configure (TMDB, your
indexers, your VPN, etc.).

See ADR 0008 (no-telemetry commitment) for the in-app posture.
```

### `/community`

Link farm:
- GitHub repo + Discussions + Issues
- Sponsorship link (GitHub Sponsors when set up)
- Contributing guide link
- Code of Conduct link
- Security disclosure (link to SECURITY.md)

### `/attributions`

Subsystem 24 deliverables:
- TMDB credit (required by their TOS)
- Trakt credit (similar)
- OpenSubtitles credit
- MDBList credit
- FFmpeg attribution + GPL component note + source-offer link
- Dep license summary (top 30 by depth, generated from `cargo-about` output via build step)
- Link to full SBOM (separately published)

---

## 4. Project layout

```
web/
├── site/                      # marketing — kinostack.app
│   ├── package.json
│   ├── astro.config.mjs       # Astro 6 + Tailwind v4 (no Starlight)
│   ├── public/                # OG images, favicons, brand-mark assets
│   └── src/
│       ├── pages/             # 9 marketing pages
│       ├── layouts/
│       ├── components/
│       └── styles/
│
├── docs/                      # docs — docs.kinostack.app
│   ├── package.json
│   ├── astro.config.mjs       # Astro 6 + Starlight 0.38 + Tailwind v4
│   ├── public/                # screenshots, diagrams
│   └── src/
│       ├── content/docs/...   # Starlight content collections
│       ├── components/        # Astro components used inside MDX
│       └── styles/
│
├── shared/                    # cross-project tokens
│   ├── tokens.css             # CSS custom-properties (color, spacing, type)
│   ├── brand/                 # logo SVGs, wordmark, app icon sources
│   └── README.md              # how to consume this from each project
│
└── README.md                  # top-level web/ readme
```

`shared/` is referenced from both Astro projects via relative imports
(`import "../shared/tokens.css"`). Cloudflare Pages supports per-project
"build root" so each project builds in its own subdirectory.

The current `web/` (single-project layout) gets reorganised into `web/site/`
(retiring `pages/install.astro`, `pages/api.astro`) and `web/docs/`
(content collections move from `web/src/content/` to `web/docs/src/content/`).

---

## 5. Visual / brand

### What we have

- `web/public/brand/icon-32.png`, OG image, apple-touch-icon
- `web/src/assets/wordmark.svg` (Starlight logo)
- Tailwind v4 dark theme via `@tailwindcss/vite`
- CSS custom-properties in `web/src/styles/global.css` (will move to `web/shared/tokens.css`)

### What we need

- **Real screenshots** of every major surface (library, search, download monitor, settings, casting). Currently we have a placeholder div on `/`. Highest-impact missing asset by far.
- **Light/dark screenshot pairs** (uv pattern) — `*-light.png` + `*-dark.png` swapped via CSS. Doubles the asset count but looks 10× more polished.
- A **logo refresh** if the current SVG isn't final
- An **OG image** that isn't the placeholder

Screenshots are blocked on having a populated demo database to take them from. Worth investing in a "demo seed" runbook (`just demo-seed` → load fixture content for screenshot purposes) so screenshots can be regenerated when the UI changes.

---

## 6. Releases page

### Source

GitHub Releases API: `GET /repos/kinostack-app/kino/releases`. Returns
JSON array, each entry has:

```json
{
  "tag_name": "v0.5.0",
  "name": "v0.5.0 — Discover & monitor",
  "published_at": "2026-04-27T18:30:00Z",
  "body": "## What's new\n\n…",
  "html_url": "https://github.com/kinostack-app/kino/releases/tag/v0.5.0",
  "assets": [
    {
      "name": "kino-0.5.0-x86_64-unknown-linux-gnu.tar.gz",
      "size": 38291024,
      "browser_download_url": "https://github.com/.../kino-0.5.0-x86_64-unknown-linux-gnu.tar.gz",
      "download_count": 142
    }
  ]
}
```

### Build-time fetch

Astro endpoint or `astro.config.mjs` integration that runs at build:

```js
// web/site/src/lib/releases.ts
export async function fetchReleases() {
  const res = await fetch(
    "https://api.github.com/repos/kinostack-app/kino/releases?per_page=50",
    { headers: { Accept: "application/vnd.github+json" } }
  );
  return res.json();
}
```

Called from `pages/releases.astro` at build time (Astro's default
`output: "static"` runs `.astro` page bodies once at build). No runtime
API calls = no rate-limit concern + no failure mode.

GH API rate limit unauthenticated is 60/hour per IP; CF Pages build
runners are not shared so we won't hit it. If we ever do, gate behind a
`GITHUB_TOKEN` env var (no scopes needed for public repo reads, gets
5000/hour).

### Asset → OS mapping

cargo-dist asset names follow `kino-{version}-{target-triple}.{ext}`:

| Triple regex | OS group | Display |
|---|---|---|
| `x86_64-unknown-linux-gnu` | Linux | Linux x64 |
| `aarch64-unknown-linux-gnu` | Linux | Linux ARM64 |
| `x86_64-apple-darwin` | macOS | macOS Intel |
| `aarch64-apple-darwin` | macOS | macOS Apple Silicon |
| `x86_64-pc-windows-msvc` | Windows | Windows x64 |
| ends `.deb` | Linux | Debian/Ubuntu |
| ends `.rpm` | Linux | Fedora/RHEL |
| ends `.AppImage` | Linux | AppImage |
| ends `.msi` | Windows | MSI installer |
| ends `.pkg` | macOS | .pkg installer |
| `kino-rpi-*.img.xz` | Raspberry Pi | Pi appliance |
| `SHA256SUMS*` | (skip) | not user-facing |

### Layout (final)

```
┌─ Latest: v0.5.0 ─────────────────────────────────────────┐
│  Released April 27, 2026                                 │
│                                                          │
│  ── Headline ─────────────────────────────────────────   │
│  Discover & monitor — the first stable release.          │
│                                                          │
│  Download                                                │
│  Linux:    [.deb] [.rpm] [.AppImage] [.tar.gz x64/ARM]   │
│  macOS:    [.pkg Apple Silicon] [.pkg Intel] [Homebrew↗] │
│  Windows:  [.msi] [winget↗]                              │
│  Pi:       [appliance image]                             │
│  Docker:   [docker pull ghcr.io/kinostack-app/kino:0.5]  │
│                                                          │
│  ▾ What's new (rendered markdown body)                   │
└──────────────────────────────────────────────────────────┘

Earlier releases ──────────────────────────────────────────
▸ v0.4.2  Apr 15  patch       [show notes]
▸ v0.4.1  Apr 8   patch       [show notes]
▸ v0.4.0  Mar 22  features    [show notes]
…

[Subscribe to RSS] [GitHub releases ↗]
```

`<details>` / `<summary>` for the collapsed older releases.
`/releases.atom` link points at `https://github.com/.../releases.atom`
(GitHub provides this for free).

### Tagging convention

For the page to render nicely, releases need to follow:

- Tag = `v<semver>` (already what cargo-dist + release-please do)
- Release name = `v<semver> — <one-line headline>` (capture in release-please's PR title)
- Release body = release-please-generated markdown (categorised: Features / Fixes / Performance / etc.)

If a release doesn't follow this, the page degrades gracefully (just shows the tag + body).

---
## 9. Build / deploy

### Cloudflare Pages

Two projects under the same CF account:

| CF Pages project | Build root | Build cmd | Output dir | Custom domain |
|---|---|---|---|---|
| `kino-site` | `web/site` | `npm run build` | `dist` | `kinostack.app` (apex) + `www.kinostack.app` (redirect) |
| `kino-docs` | `web/docs` | `npm run build` | `dist` | `docs.kinostack.app` |

Both gated on the same GitHub repo, both use CF's GitHub integration for
auto-deploy on push to `main` + preview deploys on PRs.

### GitHub Actions

We could let CF Pages do everything (it has its own build system), or
mirror to a GH Action so we get one CI status per project visible on
PRs. Recommend: CF Pages for actual deploy, GH Action just for the build
step (so PRs show docs/site build status alongside backend/frontend
status).

```
.github/workflows/web-site.yml   — build web/site/ on push + PR
.github/workflows/web-docs.yml   — build web/docs/ on push + PR
```

Both small (Node + npm install + build), ~2-3 minutes each. Path-gated
so unrelated changes don't trigger them.

### Cache + invalidation

CF Pages handles cache automatically. Build outputs are content-hashed.
No manual purges expected.

### Pre-build content steps

For `web/site/`:
- Fetch GH releases → `web/site/src/data/releases.json` (input to `/releases`)
- Fetch latest release asset URLs → `web/site/src/data/download-matrix.json` (input to `/download`)

For `web/docs/`:
- (Eventually) fetch + render `cargo-about` output → `web/docs/src/content/docs/reference/dependencies.md`

These run as part of `npm run build` via Astro's
[`astro:build:setup`](https://docs.astro.build/en/reference/integrations-reference/) hook.

---

## 12. Out of scope for v1

Defer until launch + first user feedback:

- Live demo instance
- Compare/vs pages
- `/blog` (no content cadence yet)
- `/api` OpenAPI explorer (Scalar / Redoc)
- One-line install scripts (`kinostack.app/install.sh` + `.ps1`)
- Multi-version Starlight (`docs.kinostack.app/v0.4` etc.)
- i18n / translations
- Customer / community-instance logo band
- RSS for the marketing blog (we have RSS for releases via GH)

---

## 13. References

- [`docs/decisions/0008-privacy-posture.md`](./decisions/0008-privacy-posture.md) — no-telemetry commitment (applies to website too)
- [`docs/roadmap/23-help-and-docs.md`](./roadmap/23-help-and-docs.md) — original docs-site spec
- [`docs/roadmap/24-attributions.md`](./roadmap/24-attributions.md) — attribution + licensing surfaces
