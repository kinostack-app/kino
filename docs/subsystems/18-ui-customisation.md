# UI customisation

Home layout customisation and persistent-view preferences. Tightly scoped — users can express **what they care about**, not **how it should render**. Rendering stays our job.

> **Current status:** Core customisation (hero toggle, row order /
> visibility, customise drawer, Up Next paused + next-episode +
> recently-added-as-padding, library view persistence) is shipped.
> **Not yet shipped:** the setup
> wizard doesn't populate `greeting_name` (so the greeting never
> appears on fresh installs until set from Settings), no "Saved"
> toast on the first few customise edits, and the "new episode since
> last watched" Up Next signal is deferred. The greeting *plumbing*
> (backend column, settings UI, Home render) all exists — only the
> wizard entry point is missing.

## Scope

**In scope:**
- Home row order + visibility (drag-reorder, toggle)
- Hero banner toggle
- Pin-to-home for followed lists (mechanism lives in `17-lists.md`; this doc owns the Home side)
- A new **Up Next** row replacing Continue Watching, with proactive next-episode detection and auto-cleaning
- Library view mode + sort + filter persistence (localStorage, no UI)
- Per-user greeting above the hero
- Empty-state Home for fresh installs

**Out of scope (deliberately):**
- Per-row sort configuration within rows
- Items-per-row, card size, poster aspect ratio
- Row titles (defaults are good; renaming is fiddling)
- Pinning individual movies/shows to Home (use a list instead)
- Per-device Home overrides — one layout, renders responsively
- Theme / colour scheme (separate subsystem if ever)
- Sidebar / TopNav reordering (TopNav is fixed, no sidebar exists)
- Badge counts on Up Next cards (thumbnail + forward-looking framing is the signal)
- Custom smart-filter rows ("2020s comedies rated 7+")

The rule: expose content intent, not rendering choices. Anything we've gotten right in defaults we don't need to expose as a knob.

## Row catalogue

Every Home row has a stable string ID. User preferences reference these IDs. Unknown IDs (e.g. a row we've removed in a future version) are silently dropped at render time — no migration UI.

| Row ID | Default position | Default visible | Source |
|---|---|---|---|
| `hero` | 1 | on (toggle) | Rotating backdrop of trending/featured content |
| `up_next` | 2 | on | §"Up Next" below — folds in paused, next-episode, new-episode, and recently-added-as-padding |
| `recommendations` | 3 | on (when Trakt connected) | Trakt `/recommendations/{movies,shows}`; auto-hides when disconnected |
| `upcoming_episodes` | 4 | **off** | Episodes airing this week for monitored shows (from Calendar data) |
| `trending_trakt` | 5 | on (when Trakt connected) | `/trending/{movies,shows}` — public data, auto-hides when disconnected |
| `trending_movies` | 6 | on | TMDB trending |
| `trending_shows` | 7 | on | TMDB trending |
| `popular_movies` | 8 | on | TMDB discover |
| `popular_shows` | 9 | on | TMDB discover |
| `list:{list_id}` | after built-ins | per `list.pinned_to_home` | One row per pinned list from `17-lists.md` |

Note: there is deliberately no separate `recently_added` row. Recently-added content surfaces *inside* Up Next as the padding signal (see §"Up Next" Content) — a separate row would duplicate items and dilute Up Next's signal. Users who want pure library browsing use the Library tab.

### v1 rows (what ships in this subsystem's first landing)

The catalogue above describes the **steady-state** Home. We ship in two passes so customisation lands before every dependency is ready:

| Row ID | v1 status | Why |
|---|---|---|
| `hero` | **in** | Already rendering today (HeroBanner) |
| `up_next` | **in** | Backend `/home/continue-watching` composes paused + next-episode + recently-added-as-padding |
| `trending_movies` | **in** | TMDB endpoint wired |
| `trending_shows` | **in** | TMDB endpoint wired |
| `popular_movies` | **in** | TMDB endpoint wired |
| `popular_shows` | **in** | TMDB endpoint wired |
| `recommendations` | deferred | Needs Trakt (`16-trakt.md`) |
| `trending_trakt` | deferred | Needs Trakt (`16-trakt.md`) |
| `upcoming_episodes` | deferred | Cheap to add (Calendar data exists); defaulted-off row, punt to v1.1 |
| `list:{id}` | deferred | Needs Lists (`17-lists.md`) |

Rows from the deferred set still exist in the server's catalogue — they simply don't appear in `GET /api/v1/home` output until their dependency ships. Adding one later = backend composer emits the row + frontend renderer type-guards it. No schema migration, no customise-drawer change, no user-prefs migration.

The customise drawer in v1 shows only rows the server emits. When a new row type becomes available, a user's saved `home_section_order` doesn't include it — it slots in at the bottom per the migration strategy below, and the user can drag it into place.

### Auto-hide behaviour

Rows hide themselves when they have nothing to show, regardless of user toggle:

- `up_next` — hides if empty
- `recently_added` — hides if zero recent additions (fresh install)
- `recommendations` + `trending_trakt` — hide when Trakt is disconnected
- `upcoming_episodes` — hides when no monitored shows have episodes airing in the next 7 days
- `list:{id}` — hides when the list has zero resolved items

Hiding an empty row is always silent — no "nothing here yet" placeholder. The user never sees a row consuming vertical space with no content.

## Up Next

Replaces the current Continue Watching row. Takes inspiration from Apple TV's "Up Next": a single forward-looking row that folds resumption, next-episode progression, and new-episode surfacing into one place. Self-curating — users never add to it, and items leave automatically when consumed.

### Content

Four signal classes, merged into one ordered list:

| Signal | What it is | When it appears |
|---|---|---|
| **Paused** | Partially-watched movie or episode | `playback_position_ticks > 0` and not within watched threshold |
| **Next episode** | The episode after a fully-watched one, when available in library | Previous episode watched ≥ threshold AND next episode file exists |
| **New episode** | Newly available episode of a show the user has watched at least one episode of | Episode imported in last 14 days AND `show.play_count > 0` AND this episode not yet watched |
| **Recently added (padding)** | Unwatched recently-added movies | Only if the above three yield <5 items |

Cap the row at 20 items total, virtualised horizontal scroll.

### Ordering

Single sort key per item: the most recent relevant timestamp.

| Signal | Timestamp used |
|---|---|
| Paused | `last_played_at` (when user paused) |
| Next episode | `last_played_at` of the preceding episode |
| New episode | `air_date_utc` of the episode |
| Recently added | `added_at` of the media file |

Sort descending by that timestamp. No weighting across classes — a recently-paused item and a recently-aired new episode compete on raw recency, which matches user intent ("whatever's most current to me right now").

### Card art

Forward-looking. Three shows all in-progress with identical show posters is unreadable.

| Signal | Card uses |
|---|---|
| Paused episode | Episode `still_path` from TMDB |
| Next episode | Next episode's `still_path` |
| New episode | Episode `still_path` |
| Paused movie | Movie backdrop (not poster) |
| Recently added movie | Movie poster |

Episode cards label the episode: `S01E04` or similar, small overlay. Show name is the card subtitle.

### Self-cleaning

- Finish a movie → leaves Up Next.
- Finish an episode → that card leaves; if next episode exists in library, it takes the slot.
- Finish a season with no next episode → show leaves Up Next until new episodes air.
- Unwatch an item (manually mark unwatched) → it re-enters at its natural timestamp.

Behaviour is emergent from the signal definitions — no explicit "remove from Up Next" action required.

### API

```
GET /api/v1/home/up-next
```

Returns a single ordered array of items with enough metadata to render cards:

```json
[
  {
    "id": 1234,
    "kind": "episode_paused",
    "type": "episode",
    "show_title": "Succession",
    "episode_label": "S04E08",
    "title": "America Decides",
    "still_path": "/...",
    "progress": 0.42,
    "sort_timestamp": "2026-04-17T19:42:01Z"
  },
  ...
]
```

Client doesn't need to merge from multiple endpoints — the server composes the row. Keeps the client dumb.

### CTA shape

Single primary action per card: tap = play. No `+`/`×`/ratings overlay on Home cards — those belong in Library. Long-press or hover → metadata modal (reuses existing MovieDetail/ShowDetail).

## Customise Home drawer

### Affordance

Small pencil icon in the Home page header (top-right, near the page title area). Visible at all times, not hidden behind a hover state. Opens a right-side drawer; Home content stays visible behind a subtle dim.

### Drawer contents

```
Customise Home                              [×]
────────────────────────────────────────────────
Show hero banner                          [●—○]

Sections (drag to reorder)
═══════════════════════════════════════════════
⋮⋮  Up Next                              [●—○]
⋮⋮  Recently Added                       [●—○]
⋮⋮  Recommended for you      Trakt       [●—○]
⋮⋮  Upcoming Episodes                    [○—●]   ← off by default
⋮⋮  Trending on Trakt        Trakt       [●—○]
⋮⋮  Trending Movies                      [●—○]
⋮⋮  Trending Shows                       [●—○]
⋮⋮  Popular Movies                       [●—○]
⋮⋮  Popular Shows                        [●—○]
─── Pinned lists ──────────────────────────────
⋮⋮  My Watchlist             Trakt       [●—○]  [×]
⋮⋮  Top 250 Horror           MDBList     [●—○]  [×]

                                [ Reset to defaults ]
```

Built-in rows and pinned lists interleave in the same draggable list — one coherent ordering. Pinned lists get a small `×` to unpin from Home (doesn't unfollow the list; lists are managed in `/lists`).

### Drag behaviour

- **Desktop**: grab anywhere on the row (cursor becomes grab), drag to reorder. Drag handle icon (`⋮⋮`) is visual only.
- **Mobile**: long-press on the row to initiate drag, then move. Matches native iOS/Android list-reorder pattern, avoids page-scroll conflicts inside the drawer.

### Keyboard accessibility (non-negotiable)

Every row is focusable. When focused:
- `↑` / `↓` to reorder one position
- `Space` to toggle visibility
- `Delete` to unpin a list (pinned-list rows only)
- `Tab` moves focus between rows

Screen readers announce reorder: "Moved Popular Movies to position 5 of 10."

### Save behaviour

Auto-saves on every change. No Save button. Small toast ("Saved") for the first three changes across the app's lifetime, then silent — don't nag.

Reset to defaults: confirmation dialog, wipes user preferences, reverts to defaults.

## Greeting

Above the hero banner (or above the top row if hero is disabled):

```
Good evening, Alex
```

Rules:

| Local time | Greeting |
|---|---|
| 05:00–11:59 | Good morning |
| 12:00–17:59 | Good afternoon |
| 18:00–04:59 | Good evening |

Name comes from the setup-wizard-provided user name (stored in Config). If no name is set, fall back to just `Good evening` with no name.

Time-of-day derives from the client's local time (not the server's — respects travelling users). Single small heading, `text-xl`, muted colour. Appears only on Home.

## Empty-state Home

Fresh install: no library content, no watch history, no Trakt connection.

- `hero` — shows Trending content from TMDB as the banner (still populated)
- `up_next` — auto-hides (empty)
- `recently_added` — auto-hides (empty)
- `recommendations` / `trending_trakt` — auto-hidden (Trakt not connected)
- Trending/Popular rows — fully populated from TMDB
- No special "Get started" component — Discover/Search are already prominent in TopNav

Home never looks empty; it just leans harder on public rows until personal signal exists. No ceremony, no setup checklist on Home. The existing setup wizard (`App.tsx`) handles first-run onboarding; Home stays focused on content.

## Library persistence

Remember the user's last-used view state automatically. No settings UI — just remembered silently.

| State | localStorage key | Scope |
|---|---|---|
| View mode (grid/list) | `kino-library-view-mode` | per-device |
| Sort order (title/added/year) | `kino-library-sort` | per-device |
| Status filter (all/available/unwatched/watched/wanted) | `kino-library-status` | per-device |
| Search query | *not persisted* | ephemeral per-tab |

Per-device because these preferences genuinely differ by context — phone vs desktop users want different views. No backend sync needed.

On page load, read localStorage; if values are absent or invalid, apply defaults (`grid`, `added`, `all`).

Discover page filters stay on their current URL-params pattern — keeps back-button behaviour sane, no change.

## Schema

### `user_preferences` (new)

Single row, enforced via `CHECK (id = 1)`.

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK | Always 1 |
| home_hero_enabled | BOOLEAN NOT NULL DEFAULT TRUE | |
| home_section_order | TEXT NOT NULL | JSON array of row IDs in order |
| home_section_hidden | TEXT NOT NULL DEFAULT '[]' | JSON array of hidden row IDs |
| greeting_name | TEXT | Populated from setup wizard; null → no name in greeting |
| updated_at | TEXT NOT NULL | ISO 8601 |

Defaults for `home_section_order` on fresh install: the row catalogue in default-position order, minus `upcoming_episodes` (that stays implicit-off until user opts in via the toggle).

### Migration strategy

Row-catalogue changes over time:
- **New row added** → append to the user's `home_section_order` at the bottom; visibility defaults to on unless catalogue marks it default-off.
- **Row removed** → silently drop on read, don't rewrite preferences (leaves the ID in storage; harmless).
- **Row renamed (ID change)** → treated as "removed + added". We avoid this — IDs are stable.

## API

### Preferences

```
GET    /api/v1/preferences/home
PATCH  /api/v1/preferences/home        { hero_enabled?, section_order?, hidden_sections? }
POST   /api/v1/preferences/home/reset  Clears to defaults
```

`PATCH` is idempotent; client sends full current state, server validates and saves.

### Home composition

```
GET /api/v1/home
```

Returns the composed, ordered Home payload: array of sections, each with its ID, title, and items (pre-fetched). Auto-hide logic applied server-side — the client just renders whatever comes back.

```json
{
  "greeting": { "period": "evening", "name": "Alex" },
  "sections": [
    { "id": "hero", "kind": "hero", "items": [...] },
    { "id": "up_next", "title": "Up Next", "kind": "row", "items": [...] },
    { "id": "recently_added", "title": "Recently Added", "kind": "row", "items": [...] },
    { "id": "list:7", "title": "Top 250 Horror", "kind": "row", "items": [...], "source_badge": "MDBList" },
    ...
  ]
}
```

Single request = one render. Sections load as one payload. TanStack Query cache keyed per-section ID so individual section invalidation (e.g. after watch event) is cheap.

## Entities touched

- **Reads:** `user_preferences`, `list` (pinned lists + metadata), `Episode` + `Movie` + `Media` (for Up Next composition), `Config` (greeting name fallback), `trakt_auth` (whether to include Trakt rows)
- **Creates:** `user_preferences` (first-time save)
- **Updates:** `user_preferences` (any customisation change)

## Dependencies

- Lists subsystem (`17-lists.md`) — `list.pinned_to_home` + `list.home_order` fields drive list-row ordering
- Trakt subsystem (`16-trakt.md`) — `recommendations` and `trending_trakt` rows require Trakt auth + cache
- Calendar data — `upcoming_episodes` row reads the existing Calendar endpoint data
- Metadata subsystem — TMDB trending/popular fallbacks
- Scheduler — Up Next composition runs on demand but benefits from cached signal aggregates

No new system binaries.

## Error states

- **Preferences row missing** (fresh install or data corruption) → fall back to defaults, recreate row on next save.
- **Unknown row ID in stored order** (row removed in a newer Kino version) → drop silently at render; preserve in stored JSON in case it returns (harmless).
- **Pinned list deleted** → row auto-hides (zero items); `list:{id}` entry stays in `home_section_order` until user unpins via Customise drawer or until the deletion is reconciled.
- **Up Next composition failure** → section returns empty, row auto-hides.
- **localStorage unavailable** (privacy mode, quota exceeded) → Library falls back to defaults on every load. No error surfaced — feature gracefully degrades.
- **Drag-reorder conflict** (two tabs open, both reordering) → last-write-wins; preferences load on tab focus to stay in sync.

## Known limitations

- **Up Next isn't infallibly smart.** "Watched one episode of a 12-season show 2 years ago, never touched it again" → new episodes still surface. Trade-off accepted; opt-out is to unmonitor the show.
- **Single layout across devices.** A user who wants hero off on mobile but on on desktop has one toggle, not two. We chose simplicity over granular control. Mobile's responsive rendering shrinks hero naturally.
- **No in-row sort control.** If "Recently Added" happens to order in a way the user dislikes, their recourse is to hide the row. Accepted — within-row sort is render detail, not customisation.
- **No row-level analytics display.** Can't see "you watch 80% of Up Next items, 5% of Popular Shows" to tell you which rows are earning their space. Out of scope — surface area we don't need.
- **Greeting is English-only in v1.** Localisation (i18n) is a separate concern; when we add it, the morning/afternoon/evening logic stays, strings translate.
- **Drag-reorder on touch requires long-press** which is a learned gesture. First-time mobile users may try to grab and scroll-cancel. Mitigated by a one-time hint toast on drawer open ("Long-press to drag").
