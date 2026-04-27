# Metadata subsystem

Owns all interaction with the TMDB API. Provides content discovery, metadata storage, image caching, and ongoing metadata refresh.

## Responsibilities

- Search TMDB by query string (movies and TV)
- Browse TMDB discover endpoints (trending, popular, by genre)
- Fetch full metadata for a movie or show (details, external IDs, certifications, images, trailer)
- Persist metadata to database when content is requested
- Download and cache images locally (posters, backdrops, episode stills)
- Refresh metadata periodically for existing content
- Detect new episodes/seasons on followed shows
- Serve cached images over HTTP

## TMDB client

All TMDB API calls go through a single client with built-in rate limiting.

**Rate limiter:** Sliding window, 40 requests per 10 seconds (conservative vs TMDB's ~50 limit). Bursts up to 40 are allowed; the 41st request in a window blocks until the oldest ages out. The window is shared across all callers of the client — user-initiated and background refresh both go through the same limiter. No priority queueing today; single-user kino rarely gets close to the cap in practice.

**Retry:** 429 (rate limited) and 5xx responses trigger up to 2 retries (3 total attempts), respecting `Retry-After` when the server sends it and otherwise falling back to exponential backoff (1 s → 2 s). `Retry-After` is capped at 30 s so a misbehaving proxy can't park the scheduler. 404 returns immediately — the content doesn't exist. Transport errors retry on the same schedule as 5xx.

**API key:** Stored in Config table. Required for kino to function — first-run setup prompts for this.

## API calls by operation

### Search

```
GET /3/search/multi?query={query}&page={page}
```

Returns mixed movies and TV shows. Results are **not persisted** — they're transient, returned to the UI for the user to browse. Pagination and repeated queries fall back on the frontend's TanStack Query cache; there's no server-side cache layer.

### Discover / browse

```
GET /3/trending/movie/week
GET /3/trending/tv/week
GET /3/movie/popular
GET /3/tv/popular
GET /3/discover/movie?with_genres={id}&sort_by=popularity.desc
GET /3/discover/tv?with_genres={id}&sort_by=popularity.desc
```

Also transient — no server-side cache, browser-side TanStack Query handles per-session reuse.

### Full movie metadata (on request)

When a user requests a movie, fetch everything in one call using `append_to_response`:

```
GET /3/movie/{id}?append_to_response=external_ids,release_dates,videos
```

This returns the full movie details, IMDB/TVDB IDs, regional certifications, and trailers in a single API call. Parsed and persisted to the Movie table.

### Full show metadata (on request or follow)

```
GET /3/tv/{id}?append_to_response=external_ids,content_ratings,videos
```

Persisted to Show table. Then for each season:

```
GET /3/tv/{id}/season/{season_number}?append_to_response=external_ids
```

Returns all episodes for that season. Persisted to Series and Episode tables.

For a new show, this means 1 + N API calls (1 for show + N seasons). A 5-season show = 6 calls.

### Metadata refresh

Re-fetch show/movie details and update the database. See Refresh Logic below.

## Image cache

Images are downloaded from TMDB at original resolution and served locally by kino's HTTP server. Resized variants are generated on demand and cached.

**Storage layout:**

```
{data_path}/images/
  originals/
    movies/{tmdb_id}/poster.jpg
    movies/{tmdb_id}/backdrop.jpg
    shows/{tmdb_id}/poster.jpg
    shows/{tmdb_id}/backdrop.jpg
    shows/{tmdb_id}/seasons/{season_number}/poster.jpg
    shows/{tmdb_id}/seasons/{season_number}/episodes/{episode_number}/still.jpg
  resized/
    {md5_hash_of_params}.jpg      ← cached resized variants
```

**Download strategy:**
- Always fetch `original` size from TMDB (~1-5MB posters, ~2-10MB backdrops)
- Original size ensures quality on all targets — phone, browser, and 4K TV via Chromecast
- Downloaded in background — doesn't block the request response
- Fetched immediately when content is requested/followed

**On-demand resizing:**
- Clients request images with size parameters: `GET /api/images/{type}/{id}/{image_type}?w=500&h=750`
- If requested size matches a cached variant, serve it directly
- If not, resize from original, cache the result, and serve
- Cache key is an MD5 of the resize parameters
- No upscaling — if requested size exceeds original, serve original

**Serving:**
- Returns the cached file with long-lived cache headers (365 days, immutable)
- If original not cached yet, redirect to TMDB URL as fallback
- Blurhash placeholder generated on first download and stored on the entity (for progressive loading in the UI)

**Cleanup:**
- When a movie/show is removed from the library, its image directory is deleted
- On metadata refresh, if TMDB provides a new image path, download the new one and replace
- Resized cache can be purged at any time — regenerated on demand

## Manual refresh

Users can trigger an immediate metadata refresh from the UI. API endpoints:

- `POST /api/movies/{id}/refresh` — re-fetch movie metadata + images from TMDB
- `POST /api/shows/{id}/refresh` — re-fetch show + all seasons/episodes + images

These call the same metadata fetch logic as the periodic refresh but run immediately, bypassing the scheduler. Response returns the updated entity.

## Refresh logic

Triggered by the Scheduler subsystem on a 30-minute tick, or manually via the API. The sweep picks rows out of `movie` / `show` whose `last_metadata_refresh` is older than a tier-dependent threshold, fetches them from TMDB, updates in place, and emits events for new episodes.

### Two-tier cadence

| Tier | Staleness threshold | Who's in it |
|---|---|---|
| Hot | **1 hour** | Shows where `status` is anything other than `Ended` / `Canceled` (including NULL). Movies where `release_date` is NULL, in the future, or within the last 60 days. |
| Cold | **72 hours** | Shows where `status` is `Ended` or `Canceled`. Movies whose release date is older than 60 days. |

Tier selection happens in SQL via a `CASE` expression — single scan per table per sweep, no app-side branching. The tier each row was resolved as is logged so refresh traffic is observable.

Rationale: the only operationally-meaningful TMDB changes are new episodes on airing shows and release-date shifts on upcoming or newly-released movies. Those warrant fast detection (within an hour). Everything else — cast corrections, rating drift, still-image updates on ended shows — is cosmetic and rechecking it every 12 hours is wasteful; 72 hours is fine.

Shows in limbo (finale aired, no new season announced) stay on TMDB as `Returning Series` and so stay in the hot tier. If TMDB flips a show to `Ended`, we notice within an hour and thereafter poll weekly; if the show is later revived, worst-case detection latency is 72 hours.

**Episode detection (on show refresh):**
1. Fetch show details — check `number_of_seasons`
2. If new season exists that we don't have → fetch season details, create Series + Episode entities
3. For existing seasons, fetch season details and compare episode count
4. New episodes → create Episode entities
5. New episodes on a monitored show → defaults come from the show's `monitor_new_items` policy (`future` → acquire + in_scope, `none` → tracked only); Scheduler picks them up for search when acquire is set
6. Air date changes → update existing Episode records

## Entities touched

- **Reads:** Config (API key), Movie, Show, Series, Episode (for refresh comparisons)
- **Writes:** Movie, Show, Series, Episode (metadata fields, external IDs, image paths, last_metadata_refresh)
- **Creates:** Show, Series, Episode (on follow/request), Movie (on request)

## Dependencies

- TMDB API (external)
- Config table (API key)
- Filesystem (image cache)
- Scheduler (triggers refresh and new episode detection)
- Notification subsystem (emits events: new episode detected)

## Error states

- **No API key configured** → subsystem disabled, first-run setup required
- **TMDB unreachable** → search/browse return errors to UI, refresh jobs silently retry next cycle
- **Rate limited** → queued requests wait, user sees slight delay
- **Content not found on TMDB (404)** → return empty result, don't create entities
- **Image download fails** → UI falls back to TMDB URL, retry on next refresh
