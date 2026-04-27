# Lists

Follow curated content lists from external sources. Treats MDBList, TMDB lists, Trakt custom lists, and the Trakt watchlist as four flavours of the same primitive — a denormalised mirror of upstream items that the user can browse, with one-click add-to-library on any poster.

## Scope

A **list** is any external URL that resolves to a set of TMDB IDs. Kino polls it periodically and reflects the current contents on a dedicated page. Items don't auto-acquire — the user clicks any poster to run the normal add flow for that one item. This deliberately avoids "paste URL → 1000 TMDB fetches + 1000 download jobs".

**In scope:**
- Four source types (table below)
- Add-by-URL flow: user pastes a URL, we parse the source type + identifiers
- Scheduled polling with source-appropriate cadences
- Dedicated `/lists` page + per-list detail view
- Pin-to-home integration via the Customise Home drawer (subsystem 18). Pinned lists render as horizontal rows on Home; the drawer's "Available lists" section surfaces every un-pinned list with a toggle.

**Out of scope:**
- Auto-acquire on add. List items stay as metadata until the user clicks a poster; we only create Movie/Show rows on explicit add. This was in earlier drafts of the spec but removed once we landed on list-as-browse-surface rather than list-as-batch-acquire.
- Writing to lists from Kino (adding items to a Trakt custom list from here, etc.) — read-only for v1
- Soft-cap prompt on large lists — only useful with auto-acquire, dropped alongside it.
- Ordering semantics within a list (Kino doesn't preserve list-author's curated order beyond a "position" field used for default sort).

## Sources

| Source | Example URL | API endpoint | Auth |
|---|---|---|---|
| **MDBList** | `https://mdblist.com/lists/{user}/{slug}` | `https://mdblist.com/api/lists/{user}/{slug}/items?apikey={key}` | API key (user-provided, once per install) |
| **TMDB list** | `https://www.themoviedb.org/list/{id}` | `https://api.themoviedb.org/3/list/{id}` | Our TMDB key (already configured) |
| **Trakt custom list** | `https://trakt.tv/users/{user}/lists/{slug}` | `/users/{user}/lists/{slug}/items` | Our Trakt OAuth token (public lists don't need auth, but we attach anyway) |
| **Trakt watchlist** | `https://trakt.tv/users/{user}/watchlist` | `/sync/watchlist/movies` + `/sync/watchlist/shows` | Trakt OAuth token — user's own watchlist only |

### URL parsing

User pastes a URL into "Add list". We detect the source by host + path:

- `mdblist.com/lists/...` → MDBList
- `themoviedb.org/list/{id}` → TMDB
- `trakt.tv/users/{user}/lists/{slug}` → Trakt custom
- `trakt.tv/users/{user}/watchlist` → Trakt watchlist (only accepted if `user` matches the connected Trakt account)

Unparseable URLs → error toast "Unsupported list URL. Supported: MDBList, TMDB lists, Trakt lists, Trakt watchlists."

### Trakt watchlist as a special case

The watchlist behaves like any other list except:
- **Auto-created** when Trakt connects — can't be manually added.
- **Auto-removed** when Trakt disconnects — can't be manually removed.
- **Soft cap bypassed** — it's your own want-list, no need to confirm.
- **Polling folded into the Trakt last-activities flow** (see `16-trakt.md` §4) — no independent poll schedule.

Other Trakt-sourced lists (custom lists) are user-added and behave like MDBList / TMDB lists.

## Model

### Data flow

```
  Source URL
      │
      ▼
  Scheduled poll ──▶ Fetch items  ──▶ Diff vs list_item table
                                            │
                                            ▼
                                       New items ───▶ Insert list_item
                                                         │
                                                         ▼
                                            Auto-monitor? (config + soft cap)
                                                         │
                                                         ▼
                                             Existing monitor pipeline
                                              (Movie/Show wanted → Search → Grab)
```

### Add flow

- User pastes URL → `POST /api/v1/lists {url, confirm: false}` returns a `ListPreview` (title, count, item_type) without writing.
- User confirms → `POST` again with `confirm: true` → list + items inserted.
- No soft-cap prompt. We don't auto-acquire, so a 1000-item list just becomes a 1000-item browse page. Cheap.

### Bulk-growth notification

A poll that adds more than `list_bulk_growth_threshold` (default 20) items in a single sweep fires `AppEvent::ListBulkGrowth` so the user sees "List 'X' added 47 items" — lets them notice big curator changes without us spamming on every small delta. Fires from both the scheduled poll and manual refresh.

### Add-never-subtract

**Removing an item from a source list never removes it from your Kino library.**

If a curator prunes "A24 complete" and removes *Eighth Grade*, Kino leaves your copy of *Eighth Grade* alone. The `list_item` row goes (reflects current list state), but the underlying `Movie` / `Show` row stays. Files don't vanish because someone else edited a list.

The `ignored_by_user` column exists on `list_item` as a hook for future auto-acquire behaviour; it's preserved across re-insertions of the same `(list_id, tmdb_id, item_type)` tuple but has no UI surface today.

### Polling cadence

| Source | Interval | Rationale |
|---|---|---|
| MDBList | 6h | Curated lists, slow-changing |
| TMDB list | 6h | Same |
| Trakt custom list | 1h | User-owned, may update more often |
| Trakt watchlist | *via last-activities* | Folded into 5-min Trakt poll |

Manual "Refresh now" button on each list page forces an immediate poll.

## Schema

### `list`

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK | |
| source_type | TEXT NOT NULL | `mdblist` / `tmdb_list` / `trakt_list` / `trakt_watchlist` |
| source_url | TEXT NOT NULL | Original URL the user pasted (for display) |
| source_id | TEXT NOT NULL | Parsed identifier (slug for MDBList/Trakt, numeric ID for TMDB) |
| title | TEXT NOT NULL | From source metadata |
| description | TEXT | From source metadata |
| item_count | INTEGER NOT NULL DEFAULT 0 | Cached from last poll |
| item_type | TEXT NOT NULL | `movies` / `shows` / `mixed` — some sources are single-type, some mixed |
| last_polled_at | TEXT | ISO 8601, null before first poll |
| last_poll_status | TEXT | `ok` / `error: ...` |
| consecutive_poll_failures | INTEGER NOT NULL DEFAULT 0 | Unreachable notification fires on the transition to 3 |
| is_system | BOOLEAN NOT NULL DEFAULT FALSE | True for Trakt watchlist — can't be user-deleted |
| created_at | TEXT NOT NULL | |

**Pinning-to-home + ordering is not on this table.** A list is pinned iff the pseudo-row ID `list:<id>` appears in `user_preferences.home_section_order`; the position within that array is the visual order. On list delete, the backend scrubs stale `list:<id>` markers from `home_section_order` and `home_section_hidden` so the customise drawer doesn't see ghosts.

### `list_item` (new)

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK | |
| list_id | INTEGER NOT NULL FK | |
| tmdb_id | INTEGER NOT NULL | |
| item_type | TEXT NOT NULL | `movie` / `show` |
| title | TEXT NOT NULL | Cached for display without a join |
| poster_path | TEXT | Cached from TMDB |
| position | INTEGER | Curator's ordering if the source provides one |
| added_at | TEXT NOT NULL | When the source added this item to the list |
| ignored_by_user | BOOLEAN NOT NULL DEFAULT FALSE | User explicitly unmonitored — don't re-auto-monitor on next poll |

`UNIQUE(list_id, tmdb_id, item_type)`.

### Extensions to `Config`

| Column | Type | Default |
|---|---|---|
| mdblist_api_key | TEXT | null (required only if user follows an MDBList list) |
| list_bulk_growth_threshold | INTEGER | 20 |

## API

### List management

```
GET    /api/v1/lists                        List all followed lists
POST   /api/v1/lists                        Two-phase: {url, confirm:false} → preview; {url, confirm:true} → create
GET    /api/v1/lists/{id}                   List detail
DELETE /api/v1/lists/{id}                   Unfollow (rejects if is_system=true)
POST   /api/v1/lists/{id}/refresh           Force immediate poll
```

### List items

```
GET    /api/v1/lists/{id}/items             All items with joined library state
POST   /api/v1/lists/{id}/items/{item_id}/ignore   Set ignored_by_user (kept for future auto-acquire; no current UI)
```

Items return with a `state` string derived from the underlying Movie/Show: `not_in_library` / `in_library` / `monitoring` / `acquired` / `watched` / `ignored`. Gets rendered on the list-detail poster cards as a small badge so the user sees at a glance which items they already have.

## UX

### Onboarding wizard step

Lists are primarily a post-setup discovery feature — users add lists by URL from `/lists` once Kino is running. The wizard has **one optional sub-step tied to this subsystem**: an "MDBList API key" field grouped inside the broader "Integrations" wizard step (alongside the Trakt connection from `16-trakt.md`).

Structure:

- Text input: "MDBList API key (optional)" with a link "Don't have one? Sign up at mdblist.com →"
- **[ Skip ]** button — wizard advances with no key stored; MDBList lists remain unavailable until the user adds a key later from Settings
- **[ Save ]** button — stores the key, validates it with a test call to the MDBList API, shows a success toast

The field is optional because:
- TMDB lists work without any extra key (Kino already has a TMDB key)
- Trakt lists work via the Trakt auth from the same wizard step
- Only MDBList requires a user-provided key

Placement: alongside the Trakt connection as a single "Integrations" wizard step. One screen, two skippable integrations.

### Sidebar

New entry "Lists" in the primary sidebar, below Library, above Calendar. Badge shows count of followed lists (only when >0).

### `/lists` page

Landing page: a grid of list cards, one per followed list. Each card shows:
- List title + description
- Source-origin marker — the Trakt circle-mark for Trakt sources, a small text pill for MDBList / TMDB
- Item count
- Pin-to-home toggle (Pin / PinOff icon) — writes directly to `home_section_order`
- Refresh button, open-source link, unfollow button

Above the grid: "Add list" button → modal with URL input.

The Trakt watchlist card is visually distinct (subtle accent border, lock icon) and always appears first — can't be unfollowed while Trakt is connected.

### List detail (`/lists/{id}`)

Header: back-link to `/lists`, list title, description, item count, source-URL link, last-polled timestamp, Refresh button.

Body: grid of standard `TmdbMovieCard` / `TmdbShowCard` components — same cards Home and Discover use, so every item gets hover Play / Add / Remove affordances driven by `useContentState`. Posters are back-filled from TMDB at insert time for Trakt-sourced lists (Trakt's API doesn't return TMDB paths).

Click any item → normal MovieDetail / ShowDetail page.

### Add list modal

Single text input "Paste list URL" + "Preview" button. On submit:
- Backend parses the URL, fetches the list metadata (title, description, count)
- Preview displayed inline in the modal with a single "Add" button
- MDBList with no key configured → inline error with a link to Settings → Integrations

### Home integration

Pinned lists render as horizontal `MediaRow`s on Home via the same `section_order` registry as built-in rows. The Customise Home drawer (from subsystem 18) has two sections:

- **Sections** — everything in `section_order`, sortable, toggleable. List rows here sport their source-origin mark (Trakt circle-mark / MDBList / TMDB pill). Toggling a list row off removes `list:<id>` from `section_order` — it falls back to "Available lists" below.
- **Available lists** — every followed list not currently in `section_order`, rendered with a dashed-border row + toggle. Flipping the toggle adds `list:<id>` to the end of `section_order`.

The same pin state can also be toggled from the Pin button on each `/lists` card.

### Notifications

Three `AppEvent` variants fire through the existing webhook + WebSocket + history pipeline:

- **`ListBulkGrowth`** — fired by `lists_poll` (and manual refresh) when `apply_poll`'s `outcome.added > config.list_bulk_growth_threshold` (default 20).
- **`ListUnreachable`** — fired exactly once on the transition to `consecutive_poll_failures = 3`. Silent on further failures so a stuck source doesn't spam.
- **`ListAutoAdded`** — fired when `ensure_trakt_watchlist` actually inserts the system-list row (not on every reconnect).

No separate throttling plumbing: the "once" guarantees are baked into the emit predicates above.

## Entities touched

- **Reads:** Config (API keys, polling cadence, soft cap default), trakt_auth (for Trakt-sourced lists)
- **Creates:** `list`, `list_item`
- **Updates:** `list` (last_polled_at, item_count, pinned_to_home, home_order), `list_item` (ignored_by_user)
- **Creates (indirect)**: Movie / Show rows for unknown-to-us items (metadata resolved on first appearance via TMDB)
- **Triggers (indirect)**: Search subsystem (via `Movie.monitored = true`)

## Dependencies

- Scheduler subsystem — runs poll tasks per list
- Metadata subsystem — resolves TMDB IDs to Movie/Show rows when items first appear
- Search / Download / Import — the normal acquire pipeline is triggered by monitoring a new item; lists don't duplicate any of that logic
- Notification subsystem — surfaces significant changes
- Trakt subsystem (`16-trakt.md`) — provides the auth + last-activities plumbing for Trakt-sourced lists

No new system binaries or external services beyond what's already supported.

## Error states

- **Unparseable URL** → error toast at add time, no list created.
- **Source unreachable** (DNS failure, 503, etc.) → mark `last_poll_status = "error: ..."`, retry on next scheduled poll. After 3 consecutive failures, fire notification.
- **Source auth failure** (MDBList API key invalid, Trakt token expired) → surface via notification with link to fix; don't retry blindly.
- **List deleted on source** (Trakt user deletes the list) → mark as "source deleted" in UI; preserve `list_item` rows for history; user can unfollow when convenient.
- **Item has no TMDB ID** (rare — sources include obscure items) → skip the item, log warning, don't block the rest of the list.
- **Rate limited by source** → respect `Retry-After`, back off; TMDB has generous limits, Trakt is shared with the main Trakt bucket, MDBList has documented 60 req/min.

## Known limitations

- **Can't write to lists** — adding a movie to a Trakt custom list from Kino isn't supported. User manages lists on the source.
- **No auto-acquire** — list items sit as metadata until the user clicks a poster. See §Scope.
- **MDBList requires a user-provided API key** — unlike TMDB (we ship a key) and Trakt (OAuth), MDBList has no free programmatic access without a user-side key. Surfaced on the add flow.
- **Trakt watchlist constraints** — can't be renamed or deleted while Trakt is connected. Disconnecting Trakt removes the list.
