# Trakt integration

Two-way sync with Trakt.tv — live scrobbling during playback, bulk sync of history/collection/ratings/watchlist, personalised recommendations. Replaces the "manually tick things watched" workflow with "it just knows".

## Scope

**In scope:**
- Device-code OAuth auth, token refresh, graceful re-auth on expiry
- Live scrobble on playback events (start / pause / stop)
- Bulk sync (both directions): collection, history, ratings, watchlist, resume points
- Incremental polling via `/sync/last_activities`
- Recommendations surfaced in Discover / Home
- Surface the Trakt watchlist as a system list via the Lists subsystem (see `17-lists.md`)
- Global incognito mode for private sessions

**Out of scope:**
- Multi-user — Kino is single-user, Trakt connection is per-instance
- Checkin endpoint — strictly weaker than scrobble, no point
- Comments, social features, certifications, people, teams — not media-server concerns
- Trakt VIP features — none of the endpoints we use require VIP

## Identity strategy

Kino's metadata is TMDB-keyed. Trakt accepts `{"ids": {"tmdb": N}}` as the item identifier on every sync/scrobble endpoint — **no ID resolution step on writes**.

On reads, Trakt's response objects always include the full ID block (`trakt`, `slug`, `imdb`, `tmdb`, `tvdb`). We pluck `tmdb` and ignore the rest. When an inbound item has no TMDB ID (rare — usually very obscure shows), log a warning and skip.

No Trakt ID cache required.

## 1. Authentication

### Device code flow

Single-user, self-hosted — device code is the right UX. No OAuth redirect URI fiddling.

1. User clicks "Connect Trakt" in Settings → Integrations → Trakt.
2. Kino calls `POST /oauth/device/code` with our `client_id`.
3. Response includes `device_code`, `user_code`, `verification_url` (`trakt.tv/activate`), `expires_in`, `interval`.
4. UI modal shows the `user_code` in a large monospace block, a "Copy code" button, and a link that opens `verification_url` in a new tab.
5. Kino polls `POST /oauth/device/token` at `interval` seconds (default 5s, bump by +1s on 429).
6. Response codes:
   - `200` — approved. Store `access_token`, `refresh_token`, `expires_in`. Proceed to dry-run (see section 6).
   - `400` — pending, keep polling.
   - `404` — invalid device code, abort.
   - `409` — user already approved this code (unexpected); abort.
   - `410` — device code expired. Start over.
   - `418` — user explicitly denied. Abort cleanly with "You denied the connection request."
   - `429` — polling too fast. Increase interval.

### Token lifecycle

Trakt access tokens last 3 months. Refresh tokens last indefinitely but change on each refresh.

**Refresh:** when current token has <7 days left, call `POST /oauth/token` with `grant_type=refresh_token`. Done proactively on any API call that sees a token expiring soon, not just when requests fail. New tokens replace the old; if refresh fails (invalid grant), clear the `trakt_auth` row and surface the global "Trakt disconnected — click to reconnect" banner.

**Disconnect:** call `POST /oauth/revoke` to invalidate server-side, then clear the local `trakt_auth` row. If the revoke call fails (network error, already expired), still clear locally — don't leave a user stuck "connected" in the UI.

### App registration

**Pre-ship task for the maintainer:** create a Trakt app at `trakt.tv/applications` to obtain `client_id` + `client_secret`. Ship them embedded in the binary — standard for public OAuth clients. Redirect URI: `urn:ietf:wg:oauth:2.0:oob` (the Trakt convention for device-flow apps).

### Required headers

Every Trakt call includes:

```
trakt-api-version: 2
trakt-api-key:     {client_id}
User-Agent:        Kino/{version}
Authorization:     Bearer {access_token}   (on authenticated endpoints)
Content-Type:      application/json        (on POST/PUT)
```

## 2. Live scrobbling

Trakt's scrobble API is a three-state state machine driven by playback events — **not a heartbeat**. Trakt derives duration from start/stop timestamps server-side.

### State machine

```
 play      ──▶ POST /scrobble/start
 pause     ──▶ POST /scrobble/pause
 resume    ──▶ POST /scrobble/start   (yes, /start again)
 seek      ──▶ POST /scrobble/start   (re-sync after discontinuity)
 stop/end  ──▶ POST /scrobble/stop
```

Payload: `{ "movie": {...}, "progress": 0.0..100.0 }` or `{ "episode": {...}, "progress": ... }` with the episode identified via TMDB ID. Progress is percentage of runtime.

The important call is **`/scrobble/stop`** — Trakt decides:
- progress ≥ `watched_percent` (user's Trakt-side setting, default 80) → mark as watched, add to history
- progress < `watched_percent` → save as paused resume point, accessible via `/sync/playback`

Kino's local "watched" threshold reads `watched_percent` from the user's Trakt account on connect and mirrors it, so the two sides always agree.

### Trigger source

Existing playback WebSocket (`state/websocket.ts` consumers already receive play/pause/seek/stop events for the progress bar). Trakt becomes one more subscriber — no new event infrastructure.

### Seek detection

Raw playback-progress events fire continuously (~every second). Sending `/scrobble/start` on each one violates Trakt's rate limits and overwrites server-side state pointlessly. Only re-scrobble when something discontinuous happens:

- Paused-state changes (play ↔ pause)
- Playback advanced ≥10s more than wall-clock (user seek-forward)
- Playback position moved backward by ≥10s (user seek-back)

Implementation: for each active playback session, track `last_scrobble_tick` and `last_scrobble_wall_time`. On each progress event, compare `(current_tick - last_scrobble_tick)` against `(now - last_scrobble_wall_time)` in seconds. If they mismatch by >10s in either direction, fire `/scrobble/start` with current progress and update both baselines.

### Offline scrobble queue

Network failures during playback shouldn't lose scrobble events. Queue to a SQLite table:

| Column | Type |
|---|---|
| id | INTEGER PK |
| kind | TEXT (`start` / `pause` / `stop`) |
| item_type | TEXT (`movie` / `episode`) |
| tmdb_id | INTEGER |
| progress | REAL |
| wall_time | TEXT (ISO 8601) |
| attempts | INTEGER |

On network recovery, drain the queue in insertion order. Dedup during drain:
- Consecutive `start` events for the same item → keep the last only
- `start + pause + start` → keep the last `start`
- Any `stop` is authoritative — send it

Max retention 24h. Entries older than that are dropped with a warning log (user won't remember watching it).

### Incognito mode

Global TopNav toggle suppresses all **playback-related** Trakt calls for the current browser session:
- No scrobble start/pause/stop
- No history push from local watch-completion
- No resume-point push

**Not suppressed:** collection sync (still reflects what files you own), explicit rating clicks (the user meant those). Incognito is about stealth *watching*, not stealth preferences.

Stored in `sessionStorage` (per-tab, resets on tab close). UI shows a persistent filled icon + "Private" pill in TopNav while active; the player shows "👁 Private" in place of the usual Trakt-status dot.

Post-watch stealth ("oops, I forgot to go incognito") is not supported in v1 — user can delete the history entry on trakt.tv directly.

## 3. Bulk sync

### Categories and directions

| Category | Endpoint | Direction | Trigger |
|---|---|---|---|
| Collection | `/sync/collection[/remove]` | Kino → Trakt | On import / delete / daily reconciliation |
| History | `/sync/history[/remove]` | Kino ↔ Trakt | On watch-completion (push), on `last_activities` delta (pull) |
| Resume points | `/sync/playback` | Kino ↔ Trakt | On pause (via scrobble), on `last_activities` delta (pull) |
| Ratings | `/sync/ratings[/remove]` | Kino ↔ Trakt | On user rating click (push), on `last_activities` delta (pull) |
| Watchlist | `/sync/watchlist` | Trakt → Kino (pull) | On `last_activities` delta |
| Recommendations | `/recommendations/{movies,shows}` | Trakt → Kino (pull) | Cached 24h, refresh daily |

### Collection push

When we push to `/sync/collection`, include quality metadata Trakt understands:

```json
{
  "movies": [{
    "ids": {"tmdb": 603},
    "collected_at": "2026-03-14T18:22:15Z",
    "media_type": "bluray",
    "resolution": "uhd_4k",
    "hdr": "dolby_vision",
    "audio": "dts_hd_ma",
    "audio_channels": "5.1",
    "3d": false
  }]
}
```

All of this is available from the existing probe step (`04-import.md` section 4). Fields map directly from our `Media` + `Stream` rows. Trakt displays quality badges on its web/mobile apps — free polish.

Batching: up to 1000 items per request. On initial sync, chunk accordingly. On single imports, one request is fine.

### History push

On watch-completion (via `/scrobble/stop` at ≥ watched threshold), Trakt auto-adds to history — **we don't need a separate push**. The scrobble stop is also the history write.

Exception: if Kino had existing local watch state before Trakt connect (first-time import), we push that via `/sync/history` as one bulk operation during the dry-run confirm flow (see section 6).

If the user toggles something unwatched in Kino: push to `/sync/history/remove`. If the user deletes a file: push removal to `/sync/collection/remove`, but **do not** remove from history — they watched it even if the file is gone.

### Watchlist pull

The Trakt watchlist is surfaced via the Lists subsystem (see `17-lists.md`) as a special list with `source_type = trakt_watchlist`. This subsystem provides the auth + last-activities plumbing; the Lists subsystem owns the polling, diffing, soft-cap, and auto-monitor logic.

Key behaviours specific to the Trakt-sourced lists flavour:
- Watchlist is auto-created on Trakt connect, auto-removed on disconnect (`is_system = true`).
- Watchlist polling is folded into the 5-min `last_activities` cycle — no independent poll task.
- Custom Trakt lists (user-added by URL) poll hourly; see `17-lists.md` §"Polling cadence".

### Ratings

Bidirectional. When the user rates in Kino UI: write to `movie.user_rating` (or `show.user_rating` / `episode.user_rating`), then push to `/sync/ratings`. When `last_activities` shows ratings changed on Trakt: pull `/sync/ratings/{type}` and reconcile — Trakt wins on conflict (more recent rating is the truth).

Rating scale: 1–10 (Trakt native). Kino UI renders as 10 stars OR 5 stars at 0.5-increments — pick one at implementation time, matters for visual design not data.

### Resume points

On scrobble/pause Trakt already stores the resume point server-side. To pull *other devices'* resume points (the actual win): poll `/sync/playback/{movies,episodes}`, compare `paused_at` timestamps against local `episode.playback_position_ticks` (converted to ms) — if Trakt's is newer, update Kino.

Useful flow: user pauses episode 3 in Kino at 14min → opens Trakt-aware mobile player, finishes to 42min → Kino polls `last_activities`, sees playback updated, pulls new position → when they re-open Kino, resume is already at 42min.

### Recommendations

`GET /recommendations/movies` and `/recommendations/shows`. Returns Trakt's personalised picks based on the user's watched history + ratings.

Response cache: 24h. Stored in `trakt_sync_state` as JSON blobs. Refresh daily, or on explicit "refresh recommendations" click.

Surfaced as a row on Home and a section on Discover, only when connected. Each card links to the usual MovieDetail / ShowDetail page.

## 4. Incremental sync via `/sync/last_activities`

The efficiency win. Polling endpoint that returns a nested object of timestamps:

```json
{
  "all": "2026-04-17T15:33:12Z",
  "movies": {
    "watched_at": "2026-04-17T15:33:12Z",
    "collected_at": "2026-03-14T18:22:15Z",
    "rated_at": "2026-04-12T09:01:44Z",
    "watchlisted_at": "2026-04-16T11:05:01Z",
    "paused_at": "2026-04-17T15:33:12Z",
    ...
  },
  "episodes": { ... },
  "shows": { ... },
  ...
}
```

Compare each timestamp against the cached `trakt_sync_state` row. Only pull the categories whose timestamps have advanced. On a busy sync this typically means 1–2 categories changed instead of re-pulling all 6.

**Polling cadence:** every 5 minutes. Fast enough that phone-watched episodes show up in Kino within minutes; under rate limits by orders of magnitude.

A daily full sync (same endpoints without the incremental check) catches up on anything the incremental flow missed (edge cases around clock skew, race conditions, etc.).

## 5. Rate limiting

Trakt's documented limit: **1000 requests per 5-minute window** per user. Plus per-endpoint limits: `/scrobble/*` throttled to one state-change per item per 5 minutes.

**Token bucket:** 200 requests per minute (safely under 1000/5min). Single bucket for all Trakt traffic. On 429, read `Retry-After` header and pause the bucket accordingly. Don't do Jellyfin's fixed-1-second sleep — it wastes time when Trakt says "wait 30s" and violates limits when it says "wait 2s".

**Scrobble dedup:** before firing a scrobble request, check if we sent the same state for the same item within the last 2 minutes. If so, skip locally (Trakt would 409 anyway). Prevents wasted bucket capacity on rapid play/pause loops.

**Concurrency:** up to 4 in-flight Trakt requests at once (well below what the bucket enforces). Most of our traffic is bulk sync which benefits from parallelism; Jellyfin's serialised-everything strategy is overly conservative.

## 6. First-connect dry-run

The UX innovation vs Jellyfin. After device auth succeeds, **before any local state mutation**:

1. Fetch `/sync/history/movies?limit=1`, same for episodes — get total counts from `X-Pagination-Item-Count` header.
2. Fetch `/sync/ratings/movies`, `/sync/ratings/shows`, `/sync/ratings/episodes` counts.
3. Fetch `/sync/watchlist/movies`, `/sync/watchlist/shows` counts.

Present a modal:

```
Connecting to Trakt will import:

  • 452 watched movies
  • 1,287 watched episodes
  • 23 ratings
  • 12 items from your watchlist (added as monitored)

Your existing Kino watch state will be pushed to Trakt.

  [ Import and connect ]  [ Connect without importing ]  [ Cancel ]
```

- **Import and connect**: runs full bulk sync in both directions. Local Kino history + ratings push to Trakt; Trakt history + ratings + watchlist pull to Kino. Use bulk endpoints (up to 1000 items per request).
- **Connect without importing**: stores the auth, leaves local state alone. Future scrobbles + incremental pulls work normally. Existing data stays unmerged (user's choice).
- **Cancel**: discards the token, no auth stored.

This avoids the "I connected Trakt and it nuked/overwrote my state without warning" failure mode that every media-server Trakt integration has hit.

## 7. Schema

### `trakt_auth` (new, at most 1 row)

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK | Always 1 — enforce via `CHECK (id = 1)` |
| access_token | TEXT NOT NULL | |
| refresh_token | TEXT NOT NULL | |
| expires_at | TEXT NOT NULL | ISO 8601 |
| trakt_username | TEXT NOT NULL | Display in UI |
| trakt_slug | TEXT NOT NULL | For constructing Trakt profile URLs |
| watched_threshold | INTEGER NOT NULL DEFAULT 80 | Mirrored from user's Trakt setting on connect |
| connected_at | TEXT NOT NULL | |

### `trakt_sync_state` (new, at most 1 row)

Timestamps from the last observed `/sync/last_activities` response. One row per account.

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK | Always 1 |
| movies_watched_at | TEXT | Last known Trakt timestamp; null on fresh connect |
| movies_collected_at | TEXT | |
| movies_rated_at | TEXT | |
| movies_watchlisted_at | TEXT | |
| movies_paused_at | TEXT | |
| episodes_watched_at | TEXT | |
| episodes_rated_at | TEXT | |
| episodes_paused_at | TEXT | |
| shows_rated_at | TEXT | |
| shows_watchlisted_at | TEXT | |
| recommendations_cached_at | TEXT | |
| recommendations_movies | TEXT | JSON blob |
| recommendations_shows | TEXT | JSON blob |

### `trakt_scrobble_queue` (new)

Offline queue for scrobble events. Drained on network recovery.

| Column | Type | Notes |
|---|---|---|
| id | INTEGER PK | |
| kind | TEXT NOT NULL | `start` / `pause` / `stop` |
| item_type | TEXT NOT NULL | `movie` / `episode` |
| tmdb_id | INTEGER NOT NULL | |
| progress | REAL NOT NULL | |
| wall_time | TEXT NOT NULL | ISO 8601 |
| attempts | INTEGER NOT NULL DEFAULT 0 | |

### Extensions to existing tables

Add to `Movie`:

| Column | Type | Notes |
|---|---|---|
| user_rating | INTEGER | Nullable, CHECK 1..10 |

Add to `Show`:

| Column | Type | Notes |
|---|---|---|
| user_rating | INTEGER | Nullable, CHECK 1..10 |

Add to `Episode`:

| Column | Type | Notes |
|---|---|---|
| user_rating | INTEGER | Nullable, CHECK 1..10 |

Ratings exist independently of Trakt — they're a Kino feature now, Trakt happens to sync them.

### Extensions to `Config`

| Column | Type | Default |
|---|---|---|
| trakt_scrobble_enabled | BOOLEAN | true |
| trakt_collection_sync_enabled | BOOLEAN | true |
| trakt_history_sync_enabled | BOOLEAN | true |
| trakt_ratings_sync_enabled | BOOLEAN | true |
| trakt_watchlist_sync_enabled | BOOLEAN | true |
| trakt_resume_sync_enabled | BOOLEAN | true |
| trakt_recommendations_enabled | BOOLEAN | true |

All default on — the point of connecting is for things to sync. Toggle individuals off for fine control.

## 8. UX

### Global principle

Invisible when disconnected, woven in when connected. Single settings entry point + one dismissible discovery card is the total marketing surface.

### Settings → Integrations → Trakt

**Disconnected:** short pitch card, "Connect Trakt" button, "Don't have an account? Sign up →" link.

**Connecting:** device code modal with large monospace `XXXX-XXXX`, copy button, trakt.tv/activate link, polling spinner, cancel button.

**Before first sync:** dry-run preview modal (section 6).

**Connected:** username + avatar + "Connected since X", per-feature toggles (section 7 Config fields), "Last sync: 3m ago · 47/1000 requests this window", "Disconnect" button. Disconnect offers "Keep imported data" (default) or "Clear Trakt-imported ratings/history". Trakt-sourced lists (watchlist + any custom lists) are managed in the main Lists page, not here.

### Onboarding wizard step

Trakt connection is offered as an **optional step late in the first-run setup wizard**, after core configuration (TMDB key, indexers, VPN on Linux) is in place.

The step shows the same pitch card as the Settings → Integrations → Trakt disconnected state, with two buttons:

- **[ Connect Trakt ]** — launches the device code flow inline in the wizard, same UI as Settings. On successful auth, runs the dry-run preview (section 6) inline and applies the user's choice (import / skip-import). Proceeds to the next wizard step on completion.
- **[ Skip for now ]** — wizard advances. User can connect later from Settings at any time with no loss of function.

Placement: late in the wizard (after core acquire-pipeline setup) — Trakt is an enhancement, not a prerequisite for Kino to work.

The wizard never blocks on Trakt: network errors, auth denial, or device-code timeout all fall through to "Skip for now, you can do this later" rather than stalling setup.

### TopNav

**Incognito button** — filled icon + "Private" pill when active, outline icon when off. Persists per tab. No other TopNav changes.

**Token-expired banner** — thin global banner, "Trakt disconnected — click to reconnect". Appears when refresh fails; playback/library unaffected.

### Home

**Disconnected:** unchanged. One-shot dismissible "Connect Trakt" card on first visit, never again after dismiss.

**Connected:** new "Recommended for you" row (Trakt recommendations). "Trending on Trakt" row optional (public data, works without connection).

### Discover

**Connected:** posters get a small corner badge when the item is on Trakt watchlist or already watched-elsewhere.

### Library

**Connected:** filter chip "Watched on Trakt" to hide already-seen items. Item cards show Trakt-watched tick alongside local watched state.

### MovieDetail / ShowDetail

Rating control (1–10 stars) next to the title. Watchlist heart next to existing Monitor button. "Watched on Trakt: DATE" when Trakt has a watch event we didn't originate. Small Trakt-icon link to the Trakt page.

For **ShowDetail**, each episode row also gets a rating star + Trakt-watched tick.

### Player / TorrentPlayer

Tiny status icon in the player's status area:
- green dot — scrobbling active
- amber — rate-limited, catching up
- red `!` — scrobble failed (click to diagnose; usually token expired)
- `👁 Private` — incognito session

Pixel-sized. User never needs to look at it when things work.

### Calendar

Entries on Trakt watchlist get a subtle highlight.

### Notifications

Sync failures surface via the existing notification subsystem, throttled: only after 3 consecutive failures within a 30-minute window, not per-request.

## Entities touched

- **Reads:** Movie, Show, Episode (for sync + scrobble bodies), Media, Stream (for collection quality metadata), Config (feature toggles), `trakt_auth`, `trakt_sync_state`
- **Creates:** `trakt_auth`, `trakt_sync_state`, `trakt_scrobble_queue` (per event)
- **Updates:** Movie/Show/Episode (user_rating), Episode (playback_position_ticks, watched_at, play_count from Trakt-pull), Movie (watched_at, play_count)
- **Deletes:** `trakt_auth`, `trakt_sync_state` (on disconnect with cleanup), `trakt_scrobble_queue` (on drain)

## Dependencies

- Trakt API (`api.trakt.tv`)
- Existing playback WebSocket (`state/websocket.ts`) — subscribed by the scrobble listener
- Scheduler subsystem — runs incremental + daily full-sync tasks
- Notification subsystem — surfaces auth failures + significant sync events
- Import subsystem — triggers collection push on new imports
- Cleanup subsystem — triggers collection remove on deletions

No new system binaries.

## Error states

- **Device auth denied or cancelled** → abort cleanly, no auth stored, no UI breakage.
- **Device code expired** (user took >15min to activate) → modal shows "Code expired, try again", generates a fresh code.
- **Refresh fails (invalid grant)** → clear `trakt_auth`, show global "Trakt disconnected" banner, playback unaffected.
- **429 rate limited** → respect `Retry-After`, pause bucket, requests queue until window resets.
- **Network failure during scrobble** → queue to `trakt_scrobble_queue`, drain on recovery.
- **5xx from Trakt** → retry with exponential backoff (1s, 5s, 30s), drop after 3 attempts with warning log.
- **Inbound item has no TMDB ID** → log warning, skip. No user-facing error.
- **TMDB metadata drift** (show merged/deleted on TMDB side) → skip the item on next sync, log.
- **Clock skew** (server clock wildly off) → `expires_at` comparisons misfire; guard by treating any token >6 months old as expired regardless of stored timestamp.
- **Sync-failure notification** → only after 3 consecutive failures in 30min, not per-request.

## Known limitations

- **Multi-device simultaneous scrobble race** (user watches same episode on phone + Kino at same time) → Trakt stores both; profile momentarily shows whichever called `/stop` last. Pathological case for a single-user server, not solved.
- **Incognito forgetfulness** — "I meant to go incognito but didn't" has no in-Kino undo. Direct user to trakt.tv to delete the history entry.
- **Rating scale mismatch** — Trakt is 1–10. If we render as 5 stars, half-ratings (e.g., 3.5) round on export. Pick a scale at implementation and stick to it.
- **Trakt outages** → all sync pauses, scrobbles queue. When Trakt recovers, queue drains in order; worst-case loss is scrobble events older than 24h (queue retention limit).
- **"Watched elsewhere" without TMDB mapping** — if Trakt has a watch event for a show whose TMDB ID we can't resolve locally, it's silently skipped. Rare, non-fatal.
- **First-connect import cost** — pulling 1000+ history items + ratings takes ~10–30s on first connect. Dry-run modal's "Importing..." spinner must stay responsive; sync runs as a background task with progress updates.
