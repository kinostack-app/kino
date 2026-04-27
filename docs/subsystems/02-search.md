# Search subsystem

Queries indexers (built-in Cardigann engine or any Torznab-compatible endpoint — Prowlarr/Jackett), parses release names, filters by language, dedupes across indexers, scores results against quality profiles, and triggers downloads.

## Triggers

### 1. Immediate (on request)

When a user requests a movie or follows a show, search fires immediately. No delay, no waiting for the next poll cycle. This is the main UX improvement over the *arr stack, which often makes you wait.

Flow: user requests content → Movie/Episode created as `wanted` → search fires → best release grabbed → download starts. Whole thing should take seconds.

### 2. Wanted search (periodic)

Scheduler triggers a targeted search for all content still in `wanted` state. Also checks for quality upgrades on existing content.

Key difference from the *arr stack: **each search fans out to every indexer concurrently** rather than querying them one at a time. Implementation lives in `fanout_search` (`services/search.rs`) — drives each indexer through a per-indexer async closure via `FuturesUnordered`, collects errors inline (one broken indexer can't block the others), and sorts the result set back into priority order so downstream dedup + scoring are deterministic. Rollup log line carries total wall-clock across indexers; per-indexer debug lines carry individual duration.

Multiple wanted items still process sequentially within a single sweep — that's acceptable today and not the hot path; adding outer-level parallelism is an iterate item.

### 3. Air date trigger

When Metadata detects a new episode with an air date of today (or past), search fires immediately for that episode. This is the "download episodes as they air" feature. No need to wait for a periodic cycle — the Metadata refresh creates the episode as `wanted` and search kicks in.

### 4. Manual search

User clicks "search" on a specific movie or episode. Same as immediate — no delay, results returned to UI for automatic grab or manual selection (interactive mode).

### Future: RSS polling

Not implemented initially. If targeted searches prove too aggressive on indexers, RSS polling (pulling the recent feed and matching against wanted content) can be added as a gentler alternative. One broad request per indexer covers the entire wanted list.

## Search strategy

Kino supports two indexer types: **Torznab** endpoints (Prowlarr, Jackett, or any Torznab-compatible API) and **Cardigann** definitions (built-in YAML engine that directly scrapes 500+ tracker sites without Prowlarr). See `14-indexer-engine.md` for the Cardigann engine spec.

All enabled indexers are queried in parallel for every search, regardless of type.

### Capability-aware queries

Each Torznab indexer advertises what it supports via `?t=caps`:
whether `tv-search` / `movie-search` are available, which ID params
each mode accepts, and which category IDs it serves. Kino parses
this into `TorznabCapabilities` (`torznab::caps`) and persists to
`indexer.supported_search_params` (JSON) + `indexer.supported_categories`
(JSON array) whenever the `indexer_health` sweep probes a reachable
indexer.

At search time, `services::search::narrow_query` reads the cached
capabilities back and strips ID params the indexer didn't declare
before building the URL. Example: LimeTorrents advertises only `q`
/ `season` / `ep` for tv-search — kino now sends exactly that,
instead of the previous blind `q + imdbid + tvdbid + tmdbid + season
+ ep` blob (which the server silently ignored).

Indexers that explicitly advertise a mode as unavailable
(`<tv-search available="no">`) are **skipped** from that mode's
fanout, logged at debug so the skip is visible in traces. Indexers
we've never probed get the full query (legacy behaviour —
conservative fallback).

### Query shape per mode

| Mode | Base params | ID params (if indexer declares support) |
|---|---|---|
| Movie | `q = "{title} {year}"` | `imdbid`, `tmdbid` |
| Episode | `q`, `season`, `ep`, `cat=5000` | `imdbid`, `tvdbid`, `tmdbid` |

The single combined query is sent; unsupported ID params get
stripped (not split into a second request). The "parallel ID vs
text tiers" design is not implemented — the combined-query
behaviour is what most Torznab servers expect anyway.

## Season pack handling

When searching for TV content, kino may find season packs alongside individual episodes. The decision logic respects the user's `monitored` flags per episode:

- Count how many monitored episodes in the season are `wanted`
- If a season pack is available and ≥ 2 wanted episodes remain in the season → boost the pack's score proportional to the episode coverage (~500 pts per covered episode, so a 10-ep pack beats a same-tier individual by ~5000 pts)
- If only 1-2 episodes are wanted → prefer individual episode releases
- When a season pack is grabbed, only the monitored episodes are imported. Unwanted files are ignored during import.

Season pack releases are scored the same way as individual episodes — quality tier, bonuses, seeders. The pack's score applies to all episodes it covers.

## Deduplication

The same .torrent often appears on multiple public indexers with
identical `info_hash` and different per-indexer guids. Before
scoring, cross-indexer dedup drops the duplicates:

- Fanout already returns priority-sorted (lowest `priority` first),
  so the first sighting of a hash is from the highest-priority
  indexer carrying it — kept.
- Subsequent occurrences of the same hash are discarded and logged
  at debug (`release`, `hash`, `indexer` fields) so operators can
  spot heavily-mirrored content.
- Releases without an `info_hash` pass through unchanged — can't
  dedup what we can't identify. The DB's unique index on
  `(movie_id, indexer_id, guid)` still prevents literal
  double-inserts.

Only exact-hash duplicates are considered "same release". Two
scene groups repackaging the same source into different `.mkv`
files produce different hashes and score independently.

Duplicate count rolls up into the per-search summary log line for
monitoring.

## Language filtering

Hard reject on language mismatch. `scorer::release_language_accepted`
compares the parsed release's languages against the quality
profile's `accepted_languages` list:

- Empty accept list → filter disabled, everything passes. Opt-out
  escape hatch.
- Release has no language tag → **accepted**. Scene naming
  convention is that untagged = English; rejecting untagged would
  kill the vast majority of grabs on an English profile.
- Release tagged `multi` → **accepted**. Multi-language packs
  carry the target language among others.
- Release tagged with at least one language in the accept list →
  **accepted**.
- Release tagged only with languages outside the accept list →
  **rejected**, logged at debug with release title, detected
  languages, and the accept list.

The accept list is populated from the setup wizard (step 3 of the
onboarding flow — default `["en"]`) and editable from Settings →
Quality per-profile.

"Preferred vs required" as dual modes isn't implemented — hard
reject is the only mode today. Users who want permissive
behaviour can empty the list.

Language is parsed from the release title by the release parser
(e.g. "FRENCH", "MULTi", "GERMAN.DL"). The parser normalises to
ISO 639-1-ish codes (`fr`, `de`, `multi`, etc.).

## Retry backoff

Content that's been `wanted` for a long time without results gets searched less frequently to avoid wasting indexer queries:

| Time in wanted state | Search interval |
|---------------------|-----------------|
| First 24 hours | Every cycle (most aggressive) |
| 1–7 days | Every hour |
| 7–30 days | Every 6 hours |
| 30+ days | Every 24 hours |

Based on `added_at` and `last_searched_at` on Movie/Episode — no fragile history dependency.

When content hits the 7-day tier, a notification is fired to alert the user that content hasn't been found. The user can always trigger a manual search at any time regardless of backoff.

## Release parsing

Every release title from an indexer is parsed to extract structured quality information. This runs on search results and also on filenames during import.

**Extracted fields:**
- Resolution: 480p, 720p, 1080p, 2160p
- Source: CAM, telesync, telecine, DVD, HDTV, WEBDL, WEBRip, Bluray
- Video codec: XViD, H.264, H.265/HEVC, AV1, MPEG2, VP9
- Audio codec: AAC, AC3/DD, EAC3/DD+, DTS, DTS-HD MA, TrueHD, Atmos, FLAC, Opus
- HDR format: SDR, HDR10, HDR10+, Dolby Vision, HLG
- Release group
- Flags: proper, repack, remux, internal, scene
- Languages (when indicated in title)
- For TV: show title, season number(s), episode number(s), season pack flag
- For movies: movie title, year

**Implementation note:** The *arr stack uses 50+ sequential regexes which is fragile and slow. Kino should use a structured single-pass parser — tokenize the title once, identify known patterns (resolution tokens, codec tokens, source tokens, group tags), extract everything in one pass.

## Quality scoring

Every parsed release is scored against the applicable QualityProfile to produce a `quality_score` integer.

### Score calculation

The score combines the quality tier rank from the profile with bonuses/penalties:

```
base_score = quality_tier.rank * 1000           (from QualityProfile items)

bonuses:
  + 100  if proper
  + 100  if repack
  + 50   if internal (indexer flag)
  + 10   per seeder tier: log10(seeders) * 10   (100 seeders = +20, 1000 = +30)

penalties:
  - 500  if quality tier not allowed in profile

final_score = base_score + bonuses + penalties
```

Higher score = better. The exact weights are tunable but the principle is: quality tier dominates, then flags matter, then seeders are a tiebreaker.

### Comparison with existing media

When checking for upgrades, compare the new release's score against the existing Media's quality:

- Reconstruct the existing file's score from its stored quality attributes
- The existing file's tier must be below the profile's `cutoff` (otherwise no upgrade needed)
- Upgrade triggers if:
  - New quality tier is **strictly higher** than existing tier, OR
  - New quality tier is the **same** but new score is higher by at least 200 points (prevents thrashing on minor differences — e.g., internal flag or extra seeders shouldn't cause a re-download at the same tier)
- Never downgrade (new tier < old tier → reject regardless of score)

## Grab decision

### Automatic (wanted search)

1. Filter: reject releases that fail basic checks:
   - Quality tier not allowed in profile → reject
   - Size outside acceptable range → reject (based on runtime × quality-appropriate bitrate)
   - On blocklist → reject
   - Already grabbed (same torrent hash) → reject
   - Already in download queue for this content → reject (unless upgrade)
2. Score all passing releases
3. Pick the highest scoring release
4. If it's an upgrade: only grab if score improvement is meaningful (new tier is strictly higher)
5. Grab → create Download, create DownloadContent links, set Release status to `grabbed`, log History event

### Future: hold time

If RSS polling is added later, a minimum hold time could delay grabbing RSS-discovered releases for N minutes, allowing a better release to appear before committing. Not needed without RSS — targeted searches already return all available results at once.

### Manual / interactive

Returns scored results to the UI. User picks which one to grab. No hold time, no automatic selection.

## Entities touched

- **Reads:** Movie, Episode (wanted content), Media (existing quality for upgrades), QualityProfile (scoring), Indexer (endpoints + capabilities), Blocklist, Release (existing releases), Download (active queue)
- **Writes:** Release (create, update status/score), Download + DownloadContent (on grab), History (grabbed events)

## Dependencies

- Indexer entities (Torznab endpoints)
- Prowlarr (external — the Torznab server)
- Release parser (shared utility)
- Quality scorer (shared utility)
- Download subsystem (to start a torrent)
- Notification subsystem (grabbed events)
- Scheduler (triggers wanted search)

## Error states

- **Indexer unreachable** → skip, increment failure count on Indexer entity, exponential backoff
- **All indexers failed** → log warning, retry next cycle
- **No results** → content stays `wanted`, searched again next cycle
- **Grab failed** (torrent URL dead) → blocklist the release, try next best release
- **Rate limited by indexer** → respect Retry-After header, back off
