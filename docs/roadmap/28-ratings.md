# Ratings and reviews

> **Status (2026-04-27): user-rating slice shipped via subsystem 16
> (Trakt); aggregator outstanding.** `RateWidget.tsx` posts to
> `POST /api/v1/integrations/trakt/rate/{kind}/{id}`
> (`integrations/trakt/handlers.rs`) and persists into the existing
> `user_rating INTEGER 1-10` columns on movie / show / episode.
> What this doc tracks — the multi-source aggregator (MDBList +
> OMDb + IMDb / RT / Metacritic / Letterboxd badges) and the
> dedicated `rating` / `review` tables — is **not yet implemented**.
> Detail pages still show TMDB's inline rating as the only external
> source. MDBList powers *lists* today via subsystem 17; the
> ratings half is separate.

Aggregated rating data for movies, shows, seasons, and episodes — IMDb, Rotten Tomatoes, Metacritic, Letterboxd, Trakt, MAL, RogerEbert — surfaced on detail pages as a compact badge strip. MDBList is the primary aggregator (single call returns all sources in one normalised shape), TMDB's inline rating is the always-available fallback, OMDb fills the per-episode IMDb gap for users who want it, and Trakt's per-user ratings slot in when Trakt is connected.

## Scope

**In scope:**
- Per-entity external ratings for movies, shows, seasons, and episodes
- MDBList as the primary aggregator for movies and shows (batch and single-item)
- TMDB inline rating as zero-cost fallback (already collected by the metadata subsystem)
- OMDb per-episode IMDb ratings as an optional integration for users who provide an OMDb key
- Trakt per-episode + per-item ratings when Trakt is connected (see `16-trakt.md`)
- Stale-while-revalidate refresh on detail-page load + nightly batch refresh of recently-accessed items
- Detail-page UI: 4-badge strip below the title (IMDb, RT critics, Metacritic, Letterboxd), each tooltipped with vote counts and linked to the source page
- Episode row UI: single inline rating number (no badges)
- Season UI: TMDB average only, no aggregated season-level rating
- Dedicated `rating` table keyed by `(entity, source)` — no new columns on Movie/Show/Episode
- Reserved `review` table schema for text reviews (TMDB reviews + MDBList appended reviews)
- Settings surface: shared MDBList key with `17-lists.md`, optional OMDb key as a sibling field

**Out of scope:**
- Text reviews UI surface — schema reserved, rendering added later once data quality is evaluated
- Scraping Rotten Tomatoes / Metacritic / Letterboxd / IMDb directly — fragile, ToS-grey; use MDBList instead
- A kino-hosted rating proxy (centralised aggregator) — infrastructure burden, brittle for self-hosted users
- A single OMDb key baked into the binary and shared across every install — free-rider model, eventual revocation
- Per-season aggregated ratings (RT and Metacritic don't do them reliably; surface TMDB's per-season average only)
- Parental / Common Sense Media text reviews — paid API, not worth the integration
- Ratings-driven acquisition decisions (e.g. "only grab releases of films rated > 7.0") — explicit separation of concerns; acquisition is Quality Profile's job
- Local community ratings (in-kino user rating that gets syndicated) — out of scope; Trakt already owns the user's rating graph

## Architecture

### Why a new aggregator

Common patterns we explicitly avoid:

- **Centralised proxy** (`api.example/v1` aggregator that every install
  calls home to). Works, but every install depends on infrastructure
  somebody else operates. Self-hosted tools shouldn't centralise like
  this.
- **Hardcoded scraping** of Rotten Tomatoes / Metacritic via stolen
  public Algolia keys. Fragile, ToS-grey, silent breakage on key
  rotation.
- **Single shared OMDb key baked into every install**. Free-rider
  model; inevitable revocation.

None of those survive kino's self-hosted-and-private values. The
pattern that does: let users bring their own keys (MDBList + optional
OMDb), use aggregators with documented batch APIs, fall back to free
sources (TMDB) when the user hasn't set anything up. Matches how
`16-trakt.md` and `17-lists.md` already work.

### Source stack per entity

| Entity | Always available | Optional (MDBList key) | Optional (OMDb key) | Optional (Trakt connected) |
|---|---|---|---|---|
| Movie | TMDB | IMDb, RT critics, RT audience, Metacritic, Letterboxd, MAL, Trakt, RogerEbert | (MDBList covers IMDb) | User's own rating |
| Show | TMDB | IMDb, RT critics, RT audience, Metacritic, Letterboxd, MAL, Trakt, RogerEbert | (MDBList covers IMDb) | User's own rating |
| Season | TMDB average | — (MDBList has no season endpoint) | — | — |
| Episode | TMDB | — (MDBList has no episode endpoint) | IMDb | Per-episode rating + user rating |

### Why MDBList specifically

MDBList's single-item endpoint (`GET /{provider}/{type}/{id}`) returns nine sources in one response, already normalised to `score` (0-100) alongside native `value`. The batch endpoint (`POST /{provider}/{type}` with `ids: [...]`) accepts up to 200 items per call — pivotal for nightly library refreshes. Response also includes `certification`, `age_rating`, `commonsense` flag, `watch_providers`, `trailer`, `poster` — bonus data we can either use or ignore. Pricing: 1000/day free, 100,000/day on Supporter tier (donation-ware).

Our existing usage via `17-lists.md` already collects the MDBList API key — we widen its scope to cover ratings, not add a new key.

### Why OMDb for episodes

MDBList's API has no `/episode` endpoint. For IMDb per-episode ratings (the most-recognised episode number), OMDb is the only clean option: `?i=tt...&type=episode` returns the per-episode IMDb rating and vote count, keyed on the episode's IMDb ID (which TMDB gives us via `external_ids`). Free tier: 1000/day — plenty when we use lazy-on-view + 30-day TTL. Users opt in by pasting a key; missing key simply means the IMDb badge doesn't appear on episode rows.

## 1. Schema

Separate `rating` table joined by entity FK. Rationale: 8 external sources × 4 entity types would add 32 nullable columns to Movie/Show/Series/Episode; a separate table is cleaner, scales, and lets us add sources without migrations.

```sql
CREATE TABLE rating (
  id         INTEGER PRIMARY KEY,
  -- Exactly one of these is set:
  movie_id   INTEGER REFERENCES movie(id)   ON DELETE CASCADE,
  show_id    INTEGER REFERENCES show(id)    ON DELETE CASCADE,
  series_id  INTEGER REFERENCES series(id)  ON DELETE CASCADE,  -- season
  episode_id INTEGER REFERENCES episode(id) ON DELETE CASCADE,

  source     TEXT NOT NULL,   -- 'imdb' | 'rt_critics' | 'rt_audience'
                              -- | 'metacritic' | 'metacritic_user'
                              -- | 'letterboxd' | 'myanimelist'
                              -- | 'rogerebert' | 'trakt'
  kind       TEXT NOT NULL,   -- 'user' | 'critic'
  value      REAL,            -- native provider scale
  score      INTEGER,         -- normalised 0-100
  votes      INTEGER,         -- nullable
  url        TEXT,            -- deep link to source page
  fetched_at TEXT NOT NULL,
  aggregator TEXT NOT NULL    -- 'mdblist' | 'omdb' | 'trakt'
);

CREATE UNIQUE INDEX idx_rating_movie   ON rating(movie_id,   source) WHERE movie_id   IS NOT NULL;
CREATE UNIQUE INDEX idx_rating_show    ON rating(show_id,    source) WHERE show_id    IS NOT NULL;
CREATE UNIQUE INDEX idx_rating_series  ON rating(series_id,  source) WHERE series_id  IS NOT NULL;
CREATE UNIQUE INDEX idx_rating_episode ON rating(episode_id, source) WHERE episode_id IS NOT NULL;
```

TMDB rating stays inline on Movie/Show/Episode as today — it arrives with metadata, never has a missing key, zero reason to split it out. Per-source uniqueness (upsert on `(entity, source)`) lets refresh jobs do straightforward `INSERT ... ON CONFLICT DO UPDATE`.

### Reviews (schema only)

```sql
CREATE TABLE review (
  id               INTEGER PRIMARY KEY,
  movie_id         INTEGER REFERENCES movie(id) ON DELETE CASCADE,
  show_id          INTEGER REFERENCES show(id)  ON DELETE CASCADE,
  source           TEXT NOT NULL,            -- 'tmdb' | 'mdblist'
  source_review_id TEXT,                     -- for dedup across refreshes
  author           TEXT,
  rating           INTEGER,                  -- author's own 1-10 if provided
  content          TEXT NOT NULL,
  url              TEXT,
  created_at       TEXT,                     -- from the source
  fetched_at       TEXT NOT NULL
);
```

Landed with the rating migration; population and UI come later — the subsystem reserves the shape so a future rendering change doesn't need a migration.

## 2. MDBList client

New module: `backend/crates/kino/src/ratings/mdblist.rs`. Thin `reqwest` wrapper.

```rust
pub struct MdbListClient { http: reqwest::Client, key: String }

impl MdbListClient {
    pub async fn item(&self, provider: IdProvider, kind: EntityKind, id: &str)
        -> Result<MdbItem>;

    pub async fn batch(&self, provider: IdProvider, kind: EntityKind,
                       ids: &[String], append: &[&str])
        -> Result<Vec<MdbItem>>;      // POST, up to 200 per call
}

pub enum IdProvider { Imdb, Tmdb, Trakt, Tvdb, Mal, MdbList }
pub enum EntityKind { Movie, Show, Any }
```

`MdbItem` flattens the nine-source `ratings[]` array into a typed struct — the normalised `score` is the field we persist; native `value` is shown in tooltips.

Rate limiting: honour `X-RateLimit-Remaining` / `X-RateLimit-Reset` headers. On 429, suspend in-flight batch jobs until reset; detail-page lookups continue serving cached data.

## 3. OMDb client (optional)

`backend/crates/kino/src/ratings/omdb.rs`. Only constructed when `Config.omdb_api_key` is non-empty.

```rust
pub async fn episode(&self, imdb_id: &str) -> Result<Option<OmdbEpisode>>;
```

Per-episode fetches use the episode's IMDb ID from TMDB's `external_ids`. Strategy: **lazy on first detail-page render, 30-day TTL, don't backfill**. A user browsing 50 episodes = 50 OMDb calls, well under the 1000/day free tier. Refreshing a full show eagerly would blow through it; deliberately don't.

On 401, mark the key invalid in the health dashboard (`20-health-dashboard.md`). Further calls suspended until the user fixes the key.

## 4. Trakt ratings folding

No new client — Trakt ratings ride on the Trakt subsystem (`16-trakt.md`).

- Item-level: piggyback on the bulk sync cycle Trakt already runs. When refreshing a movie/show, include `/movies/{id}/ratings` and `/shows/{id}/ratings` in the request batch; upsert into `rating` as `source='trakt'`.
- Per-episode: `/shows/{id}/seasons/{s}/episodes/{e}/ratings` lazy-fetched on episode-row render, same 30-day TTL as OMDb.
- User's own rating: stored separately on `user_rating` field — already covered in `16-trakt.md`, not owned here.

## 5. Refresh strategy

### On detail-page load (stale-while-revalidate)

1. Frontend calls `GET /api/v1/movie/{id}` (etc.) — existing endpoint, now includes `ratings: [...]` array from the `rating` table.
2. Backend inspects max `fetched_at` for the entity.
3. If fresh (< TTL), return cached.
4. If stale, return cached AND queue a background refresh — user sees data immediately, next render picks up new values via WebSocket cache-invalidation.
5. If nothing cached (first view), block on a single MDBList lookup (~300ms typical), then return.

### Nightly batch

Scheduler task `ratings_refresh` (see `07-scheduler.md`):

```
Every 24h:
  1. Find up to 200 movies with stale or missing ratings, ordered by
     (last_accessed_at DESC, rating_fetched_at ASC).
  2. One POST /tmdb/movie batch call to MDBList (ids concatenated).
  3. Upsert results into rating table.
  4. Same for shows.
```

One batch request covers a typical 200-item library. Supporter-tier users with 2000+ items naturally fall into 10 batch calls, still trivial.

### TTLs

| Source | Fresh window | Notes |
|---|---|---|
| TMDB inline | metadata refresh cycle | Already handled by `01-metadata.md` |
| MDBList movie/show (released > 30d) | 7 days | Critic scores don't move meaningfully |
| MDBList movie/show (unreleased / in-theatres) | 24 hours | Scores churn during release window |
| OMDb episode | 30 days | Stable; user-paid quota is the constraint |
| Trakt item | 7 days | Piggyback on Trakt sync cycle |
| Trakt episode | 30 days | Lazy |

### Never block acquisition

Rating data is supplementary. Metadata refresh, library import, and scheduled acquisition paths never await rating fetches. If a rating lookup times out or errors, the entity's detail page still renders with whatever cached data exists; fresh data arrives on the next cycle.

## 6. Detail-page UI

### Movie and show header — badge strip

Beneath the title and year, a horizontal row of up to four badges. Each is icon + value + subdued vote count, wrapped in a tooltip showing source name and link-out.

```
  IMDb 8.1 (673K)   🍅 97%   MC 87   Letterboxd 4.0
```

Rules:
- Only render badges for which data exists. Skip silently if missing.
- Order: IMDb, RT critics, Metacritic, Letterboxd. Audience RT collapses into a tooltip section on the RT badge rather than its own slot.
- Badges are `<a>` tags to the source URL from the `url` column, opening in a new tab.
- Icons: use each source's recognisable mark (tomato/splat for RT, MC box with its tier colour, Letterboxd bullseye, IMDb star). Tooltipped with full provider name for accessibility.

### Episode row

Single inline rating:

```
  S01E05 · The Wire · ⭐ 9.2 (12.3K)     [stream info]
```

Priority order for which number shows: IMDb (OMDb) → Trakt → TMDB. First available wins. No badge strip on episode rows — too cluttered.

### Season header

Single line: `Season rating: 8.4` using the average of the season's episodes' TMDB ratings. No per-season external aggregation.

### Attribution

Small "Ratings powered by MDBList · TMDB" footer line on detail pages, per `24-attributions.md`.

## 7. API surface

Existing entity endpoints extend to include ratings:

```
GET /api/v1/movie/{id}       → existing shape + "ratings": [{ source, score, value, votes, url, kind }]
GET /api/v1/show/{id}        → same
GET /api/v1/episode/{id}     → same
GET /api/v1/movie/{id}/reviews  → reserved, returns [] until reviews ship
```

New admin-ish endpoints:

```
POST /api/v1/ratings/refresh          → trigger nightly batch immediately (for testing)
POST /api/v1/ratings/refresh/{entity}/{id}  → force-refresh a single entity
```

## 8. Config and settings surface

Shared with the integrations step of the setup wizard (`13-startup.md` → Settings → Integrations):

| Field | Purpose | Visibility |
|---|---|---|
| `MDBList API key` | Lists + ratings | Existing field — reword label to reflect dual use |
| `OMDb API key` (optional) | Per-episode IMDb ratings | New, sibling field with a link to `omdbapi.com/apikey.aspx` |

Per-source toggles in `Settings → Appearance`: user can hide specific badges they don't care about (e.g. "hide Letterboxd"). Persisted in `user_preferences`. Default: all on.

## Entities touched

- **Reads:** Movie, Show, Series, Episode (by ID), `external_ids` from TMDB for IMDb ID lookups, Config (API keys)
- **Writes:** `rating` (upsert per source per entity), `review` (schema reserved, populated later)
- **External HTTP:** `api.mdblist.com` (batch + single-item + optional reviews append), `www.omdbapi.com` (episode lookups), `api.trakt.tv` (ratings piggybacking on Trakt subsystem's sync)
- **Events:** emits `rating_updated` on WebSocket when a batch refresh completes, so detail pages cache-invalidate without polling

## Dependencies

| Crate | Purpose |
|---|---|
| `reqwest` | HTTP (already a dep) |
| `serde`, `serde_json` | Response parsing |
| `sqlx` | DB (already a dep) |

No new system dependencies. MDBList and OMDb clients are pure Rust wrappers; Trakt integration reuses the Trakt subsystem's existing client.

## Error states

- **No MDBList key configured** → subsystem falls back to TMDB inline ratings only. Badge strip renders with just TMDB's number (or hidden entirely if the user has hidden TMDB in preferences). No error surfaced.
- **MDBList 401 / key invalid** → mark key invalid in Config health dashboard; suspend batch job until user fixes. Detail pages continue with cached data.
- **MDBList 429 rate limited** → honour `Retry-After`, suspend batch job. Detail-page lookups continue from cache.
- **MDBList 5xx** → log, don't retry in tight loop, fall through to cached data.
- **OMDb 401** → mark key invalid, suspend. Episode IMDb badges stop appearing.
- **OMDb 404 on episode** → cache the negative lookup for 30 days to avoid re-querying.
- **No IMDb ID for an entity** (obscure TV movie, direct-to-streaming, etc.) → IMDb badge simply doesn't render. No error UI.
- **Trakt disconnected** → Trakt ratings disappear from the strip; no error.
- **Rating data stale beyond TTL but refresh failing** → render cached values with no visible indicator. Health dashboard reflects the failing fetch chain.
- **Clock skew making `fetched_at` comparisons unreliable** → edge case; next successful fetch heals it.
- **Batch response partial success** (some IDs returned, others missing) → upsert the successes, log the misses, don't retry the missing in the same cycle.

## Known limitations

- **MDBList coverage is good but not perfect** for obscure titles. Falls back to TMDB-only display, which is already what happens today — no regression.
- **No per-episode ratings without OMDb key.** IMDb-per-episode remains the only widely-recognised number; TMDB covers most popular shows but not all.
- **Seasons don't get their own aggregated ratings.** MDBList doesn't expose season ratings; RT/MC only sometimes have season-level scores, inconsistently. Surfacing TMDB's per-season average is the honest compromise.
- **Rate limits are user-borne.** Free tiers cover typical libraries; large libraries (2000+) plus OMDb per-episode backfill will push users to Supporter tiers. Documented in settings help text.
- **Rotten Tomatoes data includes "fresh" rating only** — we don't display the critic consensus text, Top Critics split, or individual critic quotes. MDBList doesn't return them and we won't scrape.
- **No incremental review fetching yet.** Schema is ready; rendering the Reviews tab on detail pages is held until data-quality audit is done.
- **Ratings can appear to contradict each other** (IMDb 8.1 vs RT critics 42%) — UX accepts this. Showing all sources is the point; users calibrate their own trust.
- **OMDb's per-episode coverage depends on IMDb having the episode.** Netflix/Disney+ originals usually do; some niche streaming-only shows don't.
- **MDBList's `url` field for some sources is relative** (e.g. `tomatoes` → `/m/jaws`) — prepend provider base URL in the client before persisting.
