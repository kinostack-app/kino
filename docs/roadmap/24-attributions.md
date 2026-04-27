# Attributions and licensing

> **Status (2026-04-27): mostly shipped.**
> Shipped: `LICENSE` (GPL-3.0-or-later), `NOTICE` (Apache-2.0
> §4(d)), `CONTRIBUTING.md` with DCO instructions, PR template
> with DCO sign-off checkbox, `THIRD_PARTY_LICENSES.md` (cargo-about
> generated at the repo root, bundled in every release archive via
> cargo-dist `include`), `backend/about.toml` allow-list mirroring
> `deny.toml`, `just licenses` recipe, FFmpeg source-offer at
> [`docs/third-party/ffmpeg-source-offer.md`](../third-party/ffmpeg-source-offer.md),
> `web/site/src/pages/attributions.astro` public page covering
> TMDB / Trakt / OpenSubtitles / MDBList disclaimers + stack
> credits, in-app TMDB disclaimer in
> `frontend/src/routes/settings/MetadataSettings.tsx`.
>
> Outstanding: in-app `/about` route + version display, Settings →
> Trakt logo on connection card, subtitle picker "via OpenSubtitles"
> label, DCO GitHub App enabled on the org, SPDX header sweep
> across .rs / .ts / .tsx files, npm-side per-package licence
> aggregation in THIRD_PARTY_LICENSES.md.

How Kino respects and credits the projects and services it integrates with. Covers Kino's own licence, third-party dependency attributions, integration-service credit requirements, and the concrete files/strings that need to exist in the repo and UI to comply with upstream terms.

## Scope

**In scope:**
- Kino's own licence choice and rationale
- Third-party code attributions (Rust crates, FFmpeg, frontend dependencies)
- Integration-service attributions (TMDB, Trakt, OpenSubtitles) per their published terms
- Trademark usage principles for service names and logos
- Contributor terms (DCO, commit sign-off)
- Response procedure for takedown requests received by the project
- Concrete file deliverables (`LICENSE`, `NOTICE`, `THIRD_PARTY_LICENSES.md`)
- Concrete UI surfaces for required credits

**Out of scope:**
- User responsibility for content acquired via Kino — that's a short README paragraph, not a subsystem concern
- Privacy policy for the docs site (we don't collect user data; a short static page suffices)
- Export control analysis (WireGuard / boringtun use standard published crypto, not an issue)

## 1. Kino project licence

**Kino is licensed under GPL-3.0.**

Rationale:
- Standard licence for self-hosted media-server projects in this category
- Compatible with `librqbit` (Apache-2.0), `boringtun` (BSD-3-Clause), `chromaprint`/`rusty-chromaprint` (MIT), and the `tun` crate (MIT/Apache-2.0) — all GPL-compatible
- Permits bundling GPL-licensed FFmpeg builds with x264/x265 if we need those codecs
- Protects against proprietary forks; anyone extending Kino must keep derivative work open

File deliverables:
- `LICENSE` at repo root — verbatim GPL-3.0 text
- SPDX identifier `SPDX-License-Identifier: GPL-3.0-or-later` in every source file header (Rust + TypeScript)
- `README.md` has a Licence section linking to `LICENSE`

## 2. FFmpeg licensing

FFmpeg is dual-natured: default LGPL, but GPL when built with
`--enable-gpl` to pull in x264/x265 and other GPL components.

**Kino does NOT statically link FFmpeg, NOR does it redistribute
FFmpeg in release archives.** Two separate facts:

1. We invoke `ffmpeg` as a subprocess via `std::process::Command`.
   The [FSF GPL FAQ on MereAggregation](https://www.gnu.org/licenses/gpl-faq.html#MereAggregation)
   classifies subprocess invocation as "separate programs" — GPL
   does not propagate across the process boundary.
2. We don't ship FFmpeg with kino. Instead, kino offers an in-app
   download of upstream `jellyfin-ffmpeg` portable builds via
   **Settings → Playback → Download jellyfin-ffmpeg**.
   Implementation: `backend/crates/kino/src/playback/ffmpeg_bundle.rs`,
   pinned to a specific `JELLYFIN_FFMPEG_VERSION` with per-platform
   SHA256 checksums. Users on the `apt` / `brew` / `pacman` / `choco`
   path use their distribution's FFmpeg instead.

We chose GPL-3.0-or-later for kino itself for reasons unrelated to
FFmpeg — see [`SITE_PLAN.md`](../SITE_PLAN.md) §1.

**Compliance posture under the current model:**
- We're not the FFmpeg distributor — Jellyfin is (the `jellyfin-ffmpeg` project is the redistributor of the binary kino fetches on the user's behalf). Their GPL §6
  source-offer obligation covers the binaries kino fetches on the
  user's behalf
- [`docs/third-party/ffmpeg-source-offer.md`](../third-party/ffmpeg-source-offer.md)
  documents this posture, points at the matching jellyfin-ffmpeg
  tag, and spells out which conditions would flip the obligation
  back to us (any of: mirroring binaries on a kino-controlled host;
  bundling FFmpeg into release archives; statically linking)
- FFmpeg credit + GPL acknowledgement appears on the public
  `/attributions` page and (when shipped) the in-app About page

**If we ever bundle FFmpeg directly** (e.g. for offline-install
ergonomics on Pi appliance images), bumping the obligation back to
us is a one-file update of the source-offer doc to point at a
kino-hosted source archive — the bundling happens in
`release.yml`'s archive-staging step.

## 3. Third-party code attributions

### Rust crates

Generated via `cargo-about` (or `cargo-deny`) at release time:

```
cargo about generate about.hbs > THIRD_PARTY_LICENSES.md
```

Output: one Markdown file listing every Rust crate dependency, its licence, and the full licence text. Committed to the repo and also shipped in every release archive as `THIRD_PARTY_LICENSES.md`.

Configuration: `about.toml` pins the licence allow-list to GPL-compatible licences (MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, LGPL-2.1+, LGPL-3.0+, MPL-2.0). Any crate with an incompatible licence (AGPL, SSPL, proprietary) fails the build — catches licence drift early.

### Frontend dependencies

Generated via `license-checker-rseidelsohn` (or equivalent) over `node_modules`:

```
npx license-checker --production --json > frontend/third-party-licenses.json
```

Output merged into the top-level `THIRD_PARTY_LICENSES.md` at release time. Same GPL-compatible allow-list.

### Notable direct-use credits

These get individual entries in the app's in-UI About page (§5) in addition to being in `THIRD_PARTY_LICENSES.md`:

- **FFmpeg** (GPL) — `ffmpeg.org` — "This software uses libraries from the FFmpeg project under the GPLv3."
- **librqbit** (Apache-2.0) — `github.com/ikatson/rqbit`
- **boringtun** (BSD-3-Clause) — `github.com/cloudflare/boringtun`
- **Chromaprint / rusty-chromaprint** (MIT) — `acoustid.org/chromaprint` and `github.com/darksv/rusty-chromaprint` — required for the intro-skipper feature
- **wintun** (signed DLL by WireGuard LLC) — `wintun.net` — required on Windows — include the wintun licence text

### NOTICE file at repo root

Consolidates the minimal credits that need to be visible on casual browsing. Not a full licence listing — that's `THIRD_PARTY_LICENSES.md`. One page, human-readable, lists the directly-embedded projects and credits per their own requirements.

Apache-2.0 dependencies (librqbit, others) that include a NOTICE file require us to preserve theirs — the `cargo-about` template concatenates them into our NOTICE automatically.

## 4. Service integration attributions

Three service integrations have published attribution requirements. All are complied with via the in-app About page (§5) plus specific per-feature surfaces.

### TMDB

Per TMDB's API Terms of Use (`themoviedb.org/api-terms-of-use`):

**Required strings:**
- *"This product uses the TMDB API but is not endorsed or certified by TMDB"* — verbatim
- Referred to as "TMDB" or "The Movie Database" — no other names

**Required visuals:**
- TMDB logo displayed, less prominent than Kino's own branding
- Placement: About / Credits section

**Deliverables:**
- Bundle the TMDB logo asset in `frontend/src/assets/attributions/tmdb.svg`
- In-app About page renders the logo + disclaimer
- Settings → Metadata page shows a small "Powered by TMDB" line below the API key field

### Trakt

Per Trakt API terms (`trakt.tv/terms`):

**Usage scope:**
- Personal, non-commercial use — Kino qualifies (self-hosted single-user)
- Respect rate limits: 1000 GET / 5min, 1 POST-PUT-DELETE per second per user (already handled in `16-trakt.md`)

**Attribution:**
- Credit Trakt in the About / Credits section
- Link to trakt.tv
- Don't imply endorsement

**Deliverables:**
- Trakt logo in `frontend/src/assets/attributions/trakt.svg`
- In-app About page: "Trakt integration powered by Trakt.tv" with link
- Settings → Integrations → Trakt page displays the logo next to the connection card

### OpenSubtitles

Per OpenSubtitles REST API terms:

**Usage model:**
- Users provide their own API key (documented in `04-import.md`)
- Rate limits are tied to user's own account, not a shared Kino key

**Attribution:**
- Credit OpenSubtitles in the About page
- Users downloading subtitles see "Subtitles by OpenSubtitles" in the subtitle selection UI

**Deliverables:**
- OpenSubtitles credit line in the About page
- Subtitle picker in player: small "via OpenSubtitles" label when a fetched subtitle is active

### MDBList

No formal attribution requirement in ToS. Courtesy credit in About page: *"List support via MDBList"*.

## 5. In-app About page

Single route `/about` inside the web UI. Rendered as a static page; content lives as a component tree, not in the DB.

Sections in order:

```
Kino version 0.X.Y
Licensed under GPL-3.0-or-later
github.com/{owner}/kino

─── Built with ────────────────────────────
  Rust · React · FFmpeg · librqbit · boringtun · Chromaprint

─── Metadata and integrations ────────────
  [TMDB logo]  This product uses the TMDB API but is not endorsed
  or certified by TMDB.

  [Trakt logo] Trakt integration powered by Trakt.tv

  Subtitles via OpenSubtitles
  List support via MDBList

─── Third-party licences ─────────────────
  View full dependency licences →
  (links to THIRD_PARTY_LICENSES.md in the install directory,
   or to the GitHub-hosted copy)
```

Also linked from:
- `/help` page footer
- Settings → About sub-page
- Footer of the docs site

## 6. Trademark usage

Rule: **nominative use only**. We reference the services we
integrate with by their real names where accurate — TMDB, Trakt,
OpenSubtitles, MDBList. We don't use their logos or branding in
kino's own identity.

Specific guardrails:
- Kino's logo, icon, name, and branding are our own — no resemblance to integrated services
- Service logos appear only in attribution contexts (About page, integration settings)
- We don't position kino against named peer projects in marketing copy
- Screenshots or marketing materials don't include service logos as decoration

"Kino" itself as a name: a common word meaning "cinema" in several languages, not trademarked in the media-server space as far as we've checked. If a conflict ever surfaces, we rename — not worth fighting.

## 7. No pre-configured indexers

Kino ships with **zero pre-configured indexers**. Users add their own via Settings → Indexers.

The acquire pipeline is a tool; users provide the sources. Shipping with tracker URLs built-in would be indirect endorsement; leaving the list empty is neutral.

The indexer engine (Cardigann) supports YAML definitions in `ref/prowlarr-indexers/`, but those are referenced for parsing testing — not bundled into Kino's release as a preset list.

## 8. Default BitTorrent trackers

When a magnet link contains no embedded trackers, librqbit needs a fallback announce list. Standard practice across BitTorrent clients.

**Use the widely-adopted neutral public-tracker set:**

```
udp://tracker.opentrackr.org:1337/announce
udp://open.demonii.com:1337/announce
udp://tracker.torrent.eu.org:451/announce
udp://explodie.org:6969/announce
udp://open.stealth.si:80/announce
udp://tracker.openbittorrent.com:6969/announce
```

A widely-adopted, neutrally-named, operationally-maintained set used as defaults across mainstream BitTorrent clients.

**Remove from the current list** (`backend/crates/kino/src/indexers/downloader.rs:41–44`):
- `udp://9.rarbg.to:2710/announce` — rarbg was shut down in 2023; defunct
- `udp://9.rarbg.me:2780/announce` — same
- `udp://9.rarbg.to:2730/announce` — same
- `udp://tracker.pirateparty.gr:6969/announce` — politically-named tracker; replace with neutral equivalent

## 9. Contributor terms

**Developer Certificate of Origin (DCO)** — not a CLA.

Every commit must include a `Signed-off-by: Name <email>` line (git's `-s` flag). The DCO is a lightweight assertion that the contributor has the right to submit the code under the project's licence. Used by the Linux kernel, Docker, GitLab.

No contributor licence agreement (CLA). CLAs are friction for casual contributors and most comparable OSS projects don't need them.

File deliverables:
- `CONTRIBUTING.md` explains DCO and `git commit -s`
- GitHub Actions check: `probot/dco` or equivalent enforces sign-off on PRs
- PR template includes DCO reminder

## 10. Takedown request response procedure

If a takedown notice is received (GitHub DMCA, direct email to maintainers, or similar):

1. **Acknowledge receipt** — reply within 48 hours confirming we've received it, without committing to action
2. **Evaluate the claim**:
   - Is it a valid copyright claim on code *we actually wrote*? Extremely unlikely given the codebase origin, but possible — act on it
   - Is it a DMCA §1201 claim arguing the tool *could* be used for infringement? Not valid grounds for takedown (youtube-dl 2020 precedent) — prepare a counter-notice
   - Is it a trademark claim? Review against our §6 principles — fix if we overstepped
3. **If legitimate**: remedy the specific issue in a commit with clear notes
4. **If overreaching**: file a DMCA counter-notice via GitHub's process; request EFF review via their intake form (`eff.org/issues/coders/legal-defense-fund`); GitHub has a $1M defence fund for §1201 misuse
5. **Never silently comply with overreach** — it sets precedent against open-source tooling broadly

`docs/SECURITY.md` includes a short "Legal notices" section pointing at a dedicated email (`legal@...` or similar) for formal communications, distinct from the security-report address.

## 11. User responsibility

One paragraph in README.md:

> Kino is a media server and automation tool. Users are responsible for complying with copyright law and applicable regulations in their own jurisdiction. The Kino project does not host, distribute, or endorse any particular content — Kino is a tool; what you do with it is your choice and your responsibility.

Same paragraph in the setup wizard's final screen and in the docs site Quickstart page. Consistent framing across surfaces.

## 12. Concrete TODO list

From this doc, the files and actions that need to exist in the repo before a public release:

### Repo root
- [ ] `LICENSE` — GPL-3.0 verbatim text
- [ ] `NOTICE` — consolidated credits (auto-generated, checked in)
- [ ] `THIRD_PARTY_LICENSES.md` — full dependency licence listing (auto-generated, checked in)
- [ ] `CONTRIBUTING.md` — DCO instructions
- [ ] `SECURITY.md` — security + legal contact
- [ ] `README.md` — user-responsibility paragraph + licence section

### Source file headers
- [ ] SPDX identifier `SPDX-License-Identifier: GPL-3.0-or-later` on every Rust + TypeScript file (enforceable via a pre-commit hook)

### Tooling
- [ ] `about.toml` + `cargo-about` integrated into CI; fails build on incompatible licences
- [ ] Frontend licence check (`license-checker-rseidelsohn`) integrated into CI
- [ ] `probot/dco` GitHub App enabled on the repo
- [ ] PR template with DCO reminder

### Assets bundled
- [ ] TMDB logo at `frontend/src/assets/attributions/tmdb.svg`
- [ ] Trakt logo at `frontend/src/assets/attributions/trakt.svg`
- [ ] OpenSubtitles credit (text only, no logo requirement)
- [ ] MDBList credit (text only)
- [ ] FFmpeg source-offer document at `docs/third-party/ffmpeg-source-offer.md`

### In-app UI
- [ ] `/about` route with version, licence, integration credits, third-party link
- [ ] Settings → Metadata page — "Powered by TMDB" line with logo
- [ ] Settings → Integrations → Trakt — Trakt logo visible on connection card
- [ ] Subtitle picker — "via OpenSubtitles" label when fetched subtitle active

### Code fixes from the repo scan
- [x] Replace default trackers in `backend/crates/kino/src/indexers/downloader.rs:41–44` with the neutral public-tracker set listed in §8
- [x] Change `{group}` example in `docs/subsystems/04-import.md:135` from `FraMeSToR` to a neutral placeholder (`GROUP` or `RELEASE-TAG`)

### Release process
- [ ] CI on release tag generates fresh `THIRD_PARTY_LICENSES.md` + `NOTICE`
- [ ] Release archives include `LICENSE`, `NOTICE`, `THIRD_PARTY_LICENSES.md` at their root
- [ ] MSI installer includes the same files in a `licenses/` subfolder

## Entities touched

- **Reads:** nothing at runtime — attribution is a build-time and UI-time concern
- **Writes:** none
- **Deliverables:** the repo files listed in §12; no schema changes; no new API endpoints

## Dependencies

- `cargo-about` — build-time licence aggregation
- `license-checker-rseidelsohn` (or equivalent) — frontend licence aggregation
- `probot/dco` GitHub App — contributor sign-off enforcement

No runtime dependencies. This subsystem is documentation + static assets + release-process configuration.

## Known limitations

- **Licence drift risk** — a new transitive dependency with an incompatible licence could sneak in between releases. Mitigated by CI enforcement via `about.toml` allow-list.
- **TMDB commercial-use line** — TMDB distinguishes personal from commercial use. Users self-hosting Kino are personal. If anyone ever runs Kino as a paid service, they're on their own — that's a their-problem, not a Kino-problem.
- **Translated attributions** — the disclaimer *"This product uses the TMDB API..."* is English-only. TMDB's ToS doesn't require translation, but if we ever localise the app we'll want to include localised versions alongside the English canonical text.
- **Wintun DLL redistribution** — the WireGuard signed `wintun.dll` has its own licence. Verified at time of writing that redistribution is permitted; re-verify when updating wintun versions.
- **User-contributed Cardigann definitions** — if users share indexer YAML definitions that point at infringing sources, that's their conduct, not ours. We don't ship with any pre-configured definitions targeting specific sites (§7).
