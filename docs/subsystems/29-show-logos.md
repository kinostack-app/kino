# Show and movie logos

> **Current status:** Shipped. `logo_path` / `logo_palette` columns
> exist on `show` + `movie`, `services/logos.rs` handles TMDB fetch
> + SVG sanitisation + palette detection, and the player renders
> the sweep-fill loading animation via `VideoShell`'s
> `LoadingOverlay`. Detail-page and hero reuse follow the same
> component.

Transparent wordmark/clearlogo artwork for each show and movie, surfaced in the player as a pulsing sweep-fill animation during initial buffering and seek, and reusable for detail pages and hero banners. The logo acts as the "loading identity" of what you're about to watch.

## Scope

**In scope:**
- Per-show and per-movie logo fetched from TMDB `/images` on metadata refresh
- Prefer SVG over PNG when a clean SVG is available
- Server-side SVG sanitization (`usvg`) — text→paths, strip filters, resolve `<use>`, flatten `<foreignObject>`
- Palette detection on ingest — classify each logo as `mono` (single fill) or `multi` (2+ fills / gradients) and persist the flag
- Mono logos get `currentColor` normalisation so CSS owns the tint; multi-colour logos preserved as-is
- PNG fallback when no SVG exists, sanitization fails, or resulting SVG is degenerate
- Disk cache alongside existing image cache at `{data_path}/images/{content_type}/{tmdb_id}/logo.{ext}`
- New `logo_path` + `logo_palette` columns on `show` and `movie` tables
- Player overlay: sweep-fill animation on initial load (source set → first `canplay`) and on seek, with a 300ms show-delay debounce
- `prefers-reduced-motion` support — render static logo with no animation
- Graceful fallback chain: SVG → PNG → styled show title → existing spinner
- Outline rendering mode for mono logos (`stroke: currentColor; paint-order: stroke`) for readability over busy backgrounds
- Endpoint: `GET /api/v1/images/{content_type}/{id}/logo` serving SVG text or PNG bytes with correct `Content-Type`

**Out of scope:**
- Per-season logos — TMDB's `/images` endpoint is show-level only; there is no season logo API. Anthology-style re-brands (AHS, True Detective) lose the nuance; accepted.
- Per-episode logos — episodes inherit the parent show's logo
- Fanart.tv as a secondary source — TMDB coverage is good enough for v1 and we already hold a TMDB key; re-evaluate after release if gaps are noisy
- Third-party metadata-image proxies — unofficial CDNs we'd depend on without a contract
- Real buffer-percentage-driven fill — the sweep animation is synthetic (2s loop), not tied to actual buffer state; keeps it flicker-free and simple
- User-uploaded logo override — power-user feature, revisit later
- Logo selection UI (picking between candidates) — automatic scoring only
- Localised logos for non-English primary language — filter pinned to `en,null`; revisit when we add UI locale switching
- Raster sanitization / re-encoding — PNGs stored as-downloaded

## Architecture

### Source stack

| Entity | Source | Format preference |
|---|---|---|
| Movie | TMDB `/movie/{id}/images` | SVG > PNG |
| Show | TMDB `/tv/{id}/images` | SVG > PNG |
| Season | *(inherits show)* | — |
| Episode | *(inherits show)* | — |

Call is folded into the existing metadata refresh sweep (`02-metadata.md`): same cadence, same retry/backoff, same `last_refreshed_at` gate. No new scheduler task.

Query parameter: `include_image_language=en,null` — English-tagged logos plus language-agnostic wordmarks. Multi-language expansion is a later concern.

### Selection scoring

Client-side filter + sort on the `logos[]` array returned by TMDB:

1. **Drop** entries with `vote_count < 3` (too unvetted)
2. **Drop** entries with `aspect_ratio > 4.0` (usually banners, not wordmarks)
3. **Prefer SVG** — all SVG candidates evaluated first; only fall through to PNG if no SVG survives sanitization
4. **Sort by** `vote_average` desc, then `width` desc
5. **Try each** in order: download, sanitize, validate. First one that survives wins.

### SVG sanitization

Run the downloaded SVG through `usvg` (pure-Rust, same crate family as `resvg`) to produce a normalised tree:

- Text elements converted to paths (font dependencies eliminated)
- `<use>` resolved inline
- `<foreignObject>` dropped
- Filters it can rasterise are inlined; filters it can't are removed
- `viewBox` normalised

After usvg, one additional pass over the serialised tree:

- **Palette scan**: collect all unique fill values from paths/shapes and any `<linearGradient>` / `<radialGradient>` stops. If the set has exactly one opaque colour, classify as `mono`; if 2+ or any gradient is present, classify as `multi`.
- **Mono logos only**: strip every `fill` attribute and every `fill: X` style declaration, replace with `fill="currentColor"`. CSS now owns the colour.
- **Multi logos**: leave fills untouched.

### Validation

A sanitized SVG must pass to be kept:

- Parses and re-serialises without error
- Has at least one path with non-empty `d` attribute
- Bounding box width and height both > 0
- Total file size after sanitization < 500KB (sanity cap, not enforcement — 99% will be <50KB)

If validation fails, move to the next candidate. If every SVG fails, fall through to PNG selection using the same scoring on `file_path.endsWith('.png')` entries.

### Palette detection examples

| Show | TMDB SVG | Palette | Rationale |
|---|---|---|---|
| Breaking Bad | `ojzKpMUAcA91P6wF0TfCyAvvYLw.svg` | `mono` | Single white fill — normalise to `currentColor` |
| Game of Thrones | `zlegZ8WCkr2xaovtv99QsjCUBrB.svg` | `mono` | Single white fill — normalise |
| The Simpsons | (multi-colour wordmark) | `multi` | Yellow + red — preserve |
| HBO shows with gradient treatment | — | `multi` | Gradient treated as multi; preserve |

### Storage

Columns on `show` and `movie`:

```sql
ALTER TABLE show ADD COLUMN logo_path TEXT;        -- relative path under data/images/
ALTER TABLE show ADD COLUMN logo_palette TEXT;     -- 'mono' | 'multi' | NULL
ALTER TABLE movie ADD COLUMN logo_path TEXT;
ALTER TABLE movie ADD COLUMN logo_palette TEXT;
```

File on disk:

```
{data_path}/images/{show|movie}/{tmdb_id}/logo.{svg|png}
```

The existing `images.rs` cache layer assumes `.jpg` for poster/backdrop — extend it to preserve the source extension for logos (store the extension, don't hardcode it in the resize path).

No blurhash for logos (small, transparent, pointless).

## Player integration

### Loading state

Current `VideoShell.tsx` exposes a single `isLoading` boolean. For logo overlay we need slightly more granular gating without refactoring the whole state machine:

- Add `hasPlayedOnce` (boolean) — set `true` on first `playing` event, never cleared.
- **Show logo overlay when**: `isLoading === true` AND (`hasPlayedOnce === false` OR user just seeked).
- **Suppress** mid-stream `waiting`/`stalled` shorter than a 300ms debounce window — most network hiccups resolve quickly and flashing the logo on every blip feels broken.
- **Hide on first frame**: cross-fade logo out over 250ms when `isLoading` flips to `false` and the video starts rendering.

A seek is tracked as "just seeked" for the next `canplay` event; after that the flag clears. Post-seek buffering counts as "initial-ish" for logo purposes.

### Animation

Synthetic, not buffer-driven:

- **Mono SVG, inlined**: two stacked copies of the same SVG. Back copy at dim tint (e.g. `rgba(255,255,255,0.25)`), front copy at full tint (`#fff`) with `clip-path: inset(0 calc(100% - var(--sweep)) 0 0)`. Animate `--sweep` from 0% to 100% over 1.8s, hold briefly at 100%, reset to 0%, repeat.
- **Multi SVG, inlined**: same sweep via `clip-path`, but on dim/bright opacity variants (`opacity: 0.35` back, `opacity: 1` front) rather than re-tinting — colours must be preserved.
- **PNG fallback**: same two-copy approach with `img` elements; dim copy uses `filter: brightness(0.4)` (or `opacity: 0.35`), front copy is at full brightness with animated `clip-path`.

All timings tuned against a dark backdrop layer — the show's cached backdrop image, blurred + darkened.

### Outline mode

For mono logos only. Toggled via a CSS class when the logo is rendered over a busy or light background (episode stills, certain theme previews):

```css
.logo--outlined path {
  fill: none;
  stroke: currentColor;
  stroke-width: 2;
  paint-order: stroke;
}
```

Multi logos skip outline mode; a `drop-shadow` halo is the acceptable stand-in there but won't be enabled in v1.

### `prefers-reduced-motion`

```css
@media (prefers-reduced-motion: reduce) {
  .logo-sweep-front { clip-path: none; }
  .logo-sweep { animation: none; }
}
```

Static logo at full tint, no sweep, no pulse. No fallback to the old spinner — the static logo is itself informative.

## Fallback chain

Order when the player needs to render the loading identity:

1. SVG logo (sanitized, inline)
2. PNG logo (cached locally)
3. Styled show/movie title text (uses the existing typography stack; same sweep animation applied to the text)
4. Existing spinner (only if the entity itself is unknown / local file with no TMDB match)

Each step is evaluated at mount time based on what's populated on the entity — no network calls at render time. The cached `logo_path` + `logo_palette` columns are sufficient.

## API

### Serve logo

```
GET /api/v1/images/{content_type}/{id}/logo
```

- `content_type`: `show` | `movie`
- Response: `image/svg+xml` (UTF-8 text) or `image/png` (bytes)
- Headers: `Cache-Control: public, max-age=31536000, immutable` (same as existing image cache)
- 404 if no logo stored for this entity

The frontend fetches the SVG as text (so it can be inlined into the DOM for the sweep animation), or as an image URL in places where inlining isn't needed.

### No extra frontend API surface

Logo presence is already implicit in the entity response — expose `logo_path` (relative URL) and `logo_palette` on the show/movie DTOs. The frontend uses `logo_path` to build the logo URL and `logo_palette` to decide between mono/multi rendering paths.

## Error states

- **TMDB `/images` 404** (obscure entity) → no logo candidates; record `logo_path=NULL`, fallback chain engages at render time.
- **All SVG candidates fail sanitization** → try PNG candidates; if none, `logo_path=NULL`.
- **Download HTTP error** → retry once, then skip this candidate; don't fail the whole metadata refresh.
- **Sanitization panics on malformed input** → catch at the ingest boundary, log with TMDB file_path, skip candidate, continue.
- **Palette detection ambiguous** (e.g. one black fill + one near-black) → classify as `multi`; cheapest correct answer.
- **Entity has no TMDB match** (local-only content) → logo feature simply doesn't apply; fallback step 3 or 4 handles it at render time.
- **Logo fetch fails mid-metadata-refresh** → subsystem logs the error and continues with the rest of the entity's metadata; retry on next refresh cycle. Logo is non-critical.
- **Inlined SVG contains a script element** (shouldn't survive usvg but defence in depth) → frontend uses `DOMPurify` on the SVG text before injecting.
- **Multi-colour logo against a dark-on-dark backdrop** → user-configurable backdrop darken/blur on the player; not a logo-subsystem concern.

## Known limitations

- **Anthology shows rebrand per season** (AHS, True Detective) — we surface one show-level logo; each season's branding is lost. TMDB doesn't offer season logos; accepted.
- **TMDB logos are user-submitted** — quality varies, wrong-show logos occasionally get uploaded. No automated detection; users will have to live with occasional duds until a user-override feature lands.
- **Pathologically complex SVGs** (auto-traced with 10k+ path segments) can be slow to render when inlined. Extremely rare in practice; if hit, emit a warning and fall through to PNG for that show.
- **Localised wordmarks are not surfaced.** Japanese anime may have a katakana SVG that would be preferred by some users — today we pin `en,null`. Revisit when kino gets a UI locale picker.
- **Outline mode produces visually thicker strokes on logos with thin strokes already baked into path geometry** (some wordmark designs are outlined by fill geometry, not stroke). Outline mode gets visually noisy on those. User can toggle per-theme.
- **The animation is synthetic, not progress-driven.** A 30-second transcode spin-up still shows the same 2s sweep loop — no real feedback on how close we are. Real buffer % would require a render loop tied to the HLS `loading` / segment state; high complexity for low UX gain.
- **The `hasPlayedOnce` state resets per mount** — navigating to a different episode re-triggers "initial" logo display. Intentional; every new entity gets its own identity reveal.
