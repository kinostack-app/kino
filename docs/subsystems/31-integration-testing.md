# Backend integration testing

> **Status (2026-04-27): shipped.** Harness lives at
> `backend/crates/kino/src/test_support/{mod,mock_tmdb,mock_trakt,mock_torznab,fake_torrent}.rs`
> with fixtures under `src/test_support/fixtures/{tmdb,trakt,torznab}/`.
> `backend/crates/kino/src/flow_tests/` holds ~70 files containing
> ~187 `#[tokio::test]` functions covering setup wizard, grab→import,
> Trakt sync, scheduler triggers, VPN actions, cast token, etc.
> Run via `just test`. Out-of-scope items (frontend tests, Playwright,
> real-peer BitTorrent, real FFmpeg) remain explicitly out of scope.

A first-class HTTP integration test suite for the kino backend: real axum router, real SQLite, real schedulers, with external services (TMDB, Trakt, Torznab, librqbit, FFmpeg, wall-clock) mocked at well-defined seams. Every user-visible workflow — setup wizard, follow show, search/acquire, download, import, playback, Trakt sync, list import — exercised end-to-end in milliseconds, deterministically, on every `just test` run.

## Scope

**In scope:**
- Crate-level integration tests living in `backend/crates/kino/tests/` (public-API surface) — separate from the existing `src/tests.rs` in-module tests
- Shared harness crate `kino-test-support` (or `src/test_support/` module behind `#[cfg(any(test, feature = "test-support"))]`) owning all fixtures, fakes, and helpers
- `wiremock` for TMDB, Trakt, and Torznab/Prowlarr — injected by making their base URLs configurable via `AppState`
- Trait seams for the non-HTTP externals that can't be swapped at the network edge: `TorrentSession` (librqbit), `Transcoder` (FFmpeg), `Clock` (chrono)
- Deterministic scheduler control via `POST /api/v1/tasks/{name}/run` — tests force ticks instead of waiting
- Fixture library of captured TMDB JSON, Trakt JSON, and Torznab XML responses under `tests/fixtures/`
- ~15 golden-path flow tests **plus** a large catalogue of edge-case tests derived from the analytical pass in [Known problems by flow](#known-problems-by-flow) — current count ~150 candidate cases, expected to settle around ~80–100 after dedup and prioritisation
- CI wiring: `just test` runs the full suite; no external network; deterministic under `cargo nextest` with test-level parallelism
- Explicit gap documentation — what this harness does **not** cover, so we know when manual QA is still required

**Out of scope:**
- Frontend tests (unit, component, or E2E). Separate subsystem — see the companion doc when written.
- Playwright / browser-driven tests. Separate subsystem.
- Real-peer BitTorrent tests. FakeTorrentSession simulates state transitions; real swarm behaviour is validated by hand against live indexers.
- Real FFmpeg tests. FakeTranscoder emits synthetic progress events; actual codec correctness is a unit-test concern for codec-policy logic, not integration tests.
- Real TMDB / Trakt network calls in CI. The `/api/v1/metadata/test-tmdb` and `/api/v1/trakt/...` endpoints already exist for dev-machine smoke tests; those stay as-is.
- Load, stress, or performance testing. Different tool, different budget.
- Code coverage enforcement or mutation testing. Optional follow-up; not a prerequisite.
- Migrating existing tests in `src/tests.rs` to `tests/`. They stay where they are; new flow tests go in the new location.
- Property-based or fuzz testing of parser / scorer. Unit-test territory, already partially covered in `src/parser/tests.rs`.

## Architecture

### Harness shape

Build directly on the existing `test_app_with_db()` pattern in `src/tests.rs:10`. That function already:
- Creates an in-memory SQLite pool with all migrations applied
- Calls `init::ensure_defaults()` to seed the config row
- Constructs `AppState` with every optional dep set to `None`
- Hands back `(Router, api_key, SqlitePool)`

What changes: we extend `AppState::new()` to accept the new trait-object dependencies (clock, torrent session, transcoder), and the harness grows a builder so each test declares exactly which fakes to install.

```rust
// backend/crates/kino/src/test_support/mod.rs
pub struct TestAppBuilder {
    tmdb_server: Option<MockTmdbServer>,
    trakt_server: Option<MockTraktServer>,
    torznab_servers: Vec<MockTorznabServer>,
    torrents: FakeTorrentSession,
    transcoder: FakeTranscoder,
    clock: MockClock,
    now: DateTime<Utc>,
}

impl TestAppBuilder {
    pub fn new() -> Self { ... }
    pub fn with_tmdb(mut self) -> Self { ... }
    pub fn with_trakt_authed(mut self) -> Self { ... }
    pub fn with_indexer(mut self, name: &str) -> Self { ... }  // adds torznab mock + DB row
    pub fn at_time(mut self, when: DateTime<Utc>) -> Self { ... }
    pub async fn build(self) -> TestApp { ... }
}

pub struct TestApp {
    pub router: Router,
    pub api_key: String,
    pub db: SqlitePool,
    pub clock: MockClock,
    pub torrents: FakeTorrentSession,
    pub transcoder: FakeTranscoder,
    pub tmdb: Option<MockTmdbServer>,
    pub trakt: Option<MockTraktServer>,
}

impl TestApp {
    pub async fn get(&self, path: &str) -> Response { ... }
    pub async fn post(&self, path: &str, body: serde_json::Value) -> Response { ... }
    pub async fn run_task(&self, name: &str) -> Response { ... }  // POST /tasks/{name}/run
    pub async fn wait_for_download_state(&self, id: i64, state: &str) -> Download { ... }
    pub fn tick_clock(&self, delta: Duration) { ... }
}
```

Every test is: build → act → assert. No shared state across tests.

### Trait seams — the testability refactor

Four new traits. Each adds one `Arc<dyn Trait>` field to `AppState`; production wiring stays identical to today.

**1. `Clock`** — replaces direct `chrono::Utc::now()` calls.

```rust
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
pub struct SystemClock;
pub struct MockClock { inner: Arc<Mutex<DateTime<Utc>>> }
```

Scope of the refactor: every `Utc::now()` call site that affects observable behaviour. Priority order: `scheduler/mod.rs`, `services/download_monitor.rs`, `integrations/trakt/client.rs` (token expiry), `services/search.rs` (backoff), `services/metadata.rs` (refresh staleness). Observability/logging call sites can keep `Utc::now()` — drift in a trace timestamp doesn't break tests.

**2. `TorrentSession`** — wraps librqbit.

```rust
pub trait TorrentSession: Send + Sync {
    async fn add_magnet(&self, magnet: &str, output_dir: &Path) -> Result<TorrentHandle>;
    async fn status(&self, hash: &str) -> Result<TorrentStatus>;
    async fn pause(&self, hash: &str) -> Result<()>;
    async fn resume(&self, hash: &str) -> Result<()>;
    async fn remove(&self, hash: &str, delete_files: bool) -> Result<()>;
    async fn stream(&self, hash: &str, file_idx: usize) -> Result<Box<dyn AsyncRead>>;
}
```

Production impl: today's `LibrqbitClient`, unchanged internals. Test impl `FakeTorrentSession`:
- Stores torrents in a `DashMap<hash, FakeTorrent>`
- Test drives progress explicitly: `torrents.complete(hash)`, `torrents.set_progress(hash, 0.5)`, `torrents.fail(hash, "tracker timeout")`
- On completion, writes a stub file (fixture or zero-byte placeholder) to the configured download dir so the import step has something to find

The streaming method (`stream.rs` piece-prioritised reads) is called rarely in flow tests; the fake returns an in-memory cursor over a small fixture bytestream.

**3. `Transcoder`** — wraps `TranscodeManager`.

```rust
pub trait Transcoder: Send + Sync {
    async fn start_session(&self, media: &Media, params: TranscodeParams) -> Result<SessionId>;
    async fn session_status(&self, id: SessionId) -> Option<SessionStatus>;
    async fn stop_session(&self, id: SessionId) -> Result<()>;
    async fn master_playlist(&self, id: SessionId) -> Result<String>;
    async fn segment(&self, id: SessionId, index: u32) -> Result<Bytes>;
}
```

FakeTranscoder returns canned HLS playlists and 1KB fixture segments. Playback tests assert on playlist structure and header presence, not codec output.

**4. Configurable external base URLs** — no trait, just config.

Add three fields to `AppState`:
- `tmdb_base_url: String` (default `https://api.themoviedb.org/3`)
- `trakt_base_url: String` (default `https://api.trakt.tv`)
- A Torznab indexer's URL is already per-row in the `indexer` table — nothing to change; tests insert mock URLs into that row.

Construction reads from env (`KINO_TMDB_BASE_URL`, `KINO_TRAKT_BASE_URL`) with the production defaults as fallback. Tests point these at wiremock servers.

### External HTTP mocking

`wiremock-rs` for TMDB, Trakt, indexer Torznab. Each external gets a thin wrapper:

```rust
pub struct MockTmdbServer {
    server: wiremock::MockServer,
}

impl MockTmdbServer {
    pub async fn start() -> Self { ... }
    pub fn base_url(&self) -> String { self.server.uri() }
    pub async fn stub_movie(&self, id: i64, fixture: &str) { ... }   // loads tests/fixtures/tmdb/movie-{id}.json
    pub async fn stub_show(&self, id: i64, fixture: &str) { ... }
    pub async fn stub_search(&self, query: &str, results: serde_json::Value) { ... }
    pub async fn stub_404(&self, path_pattern: &str) { ... }
}
```

Fixture files are real captured responses, trimmed to relevant fields. Capture once via the real endpoint (dev-machine smoke flow), commit the JSON, use forever. When TMDB schemas evolve, re-capture.

Rate-limit semantics (TMDB semaphore, Trakt 1 req/sec POST gate) are **not** part of these tests — they're verified by unit tests around the client modules themselves.

### Fixtures layout

```
backend/crates/kino/tests/
├── flows/
│   ├── setup_wizard.rs
│   ├── follow_show.rs
│   ├── follow_movie.rs
│   ├── watch_now.rs
│   ├── trakt_sync.rs
│   ├── list_import.rs
│   └── ...
├── fixtures/
│   ├── tmdb/
│   │   ├── movie-603.json        # The Matrix
│   │   ├── show-1399.json        # Game of Thrones (+ seasons + episodes)
│   │   └── search-the_matrix.json
│   ├── trakt/
│   │   ├── watched-shows.json
│   │   └── watchlist-shows.json
│   ├── torznab/
│   │   ├── matrix-releases.xml
│   │   └── got-s01e01-releases.xml
│   └── media/
│       ├── tiny.mp4              # 1KB MP4 with valid ffprobe metadata
│       └── tiny.mkv
└── support/                       # re-exports from src/test_support/
    └── lib.rs
```

Fixtures are plain files loaded at runtime via `include_str!` or `std::fs::read`. No codegen, no build script.

### Deterministic scheduler

The scheduler (`scheduler/mod.rs`) already exposes `POST /api/v1/tasks/{name}/run` which dispatches a task immediately via the `trigger_tx` channel. Integration tests use this instead of waiting for the 3-second tick loop.

One gap to close: the trigger is fire-and-forget — the HTTP response returns before the task finishes. For tests, we need to know when the task is done. Options, in order of preference:

1. **Return a completion future from the trigger** — cleanest; change `trigger_tx` to send `(name, oneshot::Sender<Result<()>>)` and have `/tasks/:name/run` await the sender before responding. Production-visible change but unlocks synchronous end-to-end tests.
2. **Poll the task's `last_run_at` / `running` flag** via `GET /api/v1/tasks` until it flips. Works today without changes but is polling.
3. **Subscribe to `AppEvent`** — tests attach a listener to `event_tx` before triggering and wait for the matching event. Already works; good for event-driven assertions anyway.

Recommend (1) for task-completion waits and (3) for event-shape assertions. Both can coexist.

## Flow coverage

Prioritised by user impact. Tier 1 is the "must have integration coverage or we'll keep shipping regressions" list.

### Tier 1 — golden paths

| # | Flow | What the test verifies |
|---|---|---|
| 1 | **Setup wizard** | Empty DB → `GET /status` reports `setup_required=true` → POST config with paths + TMDB key → `setup_required=false`, indexer definitions loadable |
| 2 | **Follow show** (end-to-end) | POST `/shows` with tmdb_id → show/series/episodes persisted → force `wanted_search` → release grabbed → fake download completes → import fires → media linked to episode → library reflects it |
| 3 | **Follow movie** (end-to-end) | Same as above, movie variant |
| 4 | **Watch now** (movie) | POST `/watch-now { tmdb_id }` → movie created → release grabbed → download queued → stream endpoint returns master playlist |
| 5 | **Quality profile CRUD** | Create, edit tiers, set default, apply to existing show (reprocessing side effects) |
| 6 | **Indexer CRUD + test** | Add Torznab indexer → `test` endpoint passes against mock → appears in health panel |
| 7 | **Library views** | Library stats, calendar (upcoming episodes), continue-watching surface correct rows after the follow flows |

### Tier 2 — important but not blocking

| # | Flow | What the test verifies |
|---|---|---|
| 8 | **Trakt OAuth** | Device-code request → mock Trakt "pending" → mock "authorised" → tokens persisted |
| 9 | **Trakt incremental sync** | Library has shows; mock Trakt watched list → episodes marked watched; mock watchlist → follow suggestions surfaced |
| 10 | **List import** | Create mdblist/Trakt list source → `lists_poll` → items surfaced in `/lists/:id/items` → follow one → added to library |
| 11 | **Release upgrade** | Existing media at 720p; better 1080p release available → scorer picks it → download → import → old media cleaned |
| 12 | **Blocklist + retry** | Download fails → blocklist the release → re-search finds a different candidate |
| 13 | **Unfollow/delete** | DELETE show → episodes gone, media rows gone (or kept per config), blocklist rows pruned |

### Tier 3 — nice to have

14. Manual grab (user picks release from `/releases` instead of auto)
15. Manage dialog toggles (per-season monitor, re-download, mark watched)
16. Webhook dispatch fires on key events
17. Intro-skipper: media imported with subtitles → `intro_catchup` runs → skip ranges persisted

Tier 3 is deferred until Tier 1 + 2 are green. Don't front-load.

The flows above are **journey-level**. Each one fans out into multiple specific test cases — happy path, every external failure mode, every race window, every data edge case. Those cases are catalogued in the next section.

## Known problems by flow

This section is the output of an analytical pass over the backend code (auth, indexers, shows, movies, search, downloads, import, playback, Trakt, lists, scheduler, webhooks). It is the **seed list of edge-case tests**, not exhaustive — new cases will be discovered while writing the tests, and as production bugs reveal additional gaps. Treat it as a living document.

Format: each table row is one concrete test case. `Where` columns reference `file.rs:line`. Cases marked **(seam)** require one of the trait refactors from [Architecture / Trait seams](#trait-seams--the-testability-refactor) to be testable; the rest can be written against today's code.

### Cross-cutting patterns

Seven patterns recur across nearly every flow. Worth internalising before reading the per-flow tables, and worth a one-time set of harness primitives (e.g. a `race(future_a, future_b)` helper, a `with_db_lock` helper, a `mock_external_failures()` matrix runner):

1. **Check-then-act races (TOCTOU).** Code path queries existence, then writes, with no row-level lock between. Seen in: API key rotation, quality-profile set-default, follow-show idempotency, indexer enable/disable, release grab vs scheduler grab, blocklist add vs in-flight download. Test pattern: spawn two concurrent ops on the same row, assert exactly-once semantics.
2. **Silent error swallowing.** `.ok()`, `let _ = ...`, `unwrap_or_default()` patterns drop errors that should surface. Seen in: `auth.rs:127` (DB lock → auth bypass), `init.rs` (config insert failure), `services/search.rs` (indexer XML parse failure), `cleanup/mod.rs` (file delete failure → DB row deleted anyway), preferences JSON corruption. Test pattern: inject failure at the boundary, assert the error shows up in `last_error` / response / logs — not in a `.ok()`-shaped silence.
3. **Scheduler vs HTTP races.** A 3-second tick can fire while a user-initiated HTTP request is mid-flight on the same row. Seen in: download monitor vs cancel/retry, metadata refresh vs delete show, wanted-search vs grab-and-watch, list poll vs manual refresh, transcode cleanup vs active session. Test pattern: trigger the task via `/tasks/:name/run` while holding open the HTTP request, assert state stays consistent.
4. **External failure modes ignored.** Most external calls assume happy 2xx responses. Test matrix per external (TMDB, Trakt, Torznab, librqbit add, ffprobe): timeout, 429 rate limit, 5xx, 4xx, malformed body (HTML error page where XML/JSON expected), partial response (truncated mid-stream). Almost every TMDB and Trakt call has at least one untested branch.
5. **Cleanup / cascade gaps.** Delete operations don't cascade fully. Seen in: deleted indexer leaves orphan downloads, deleted show leaves orphan releases, deleted media leaves orphan playback rows, history rows reference deleted entities, hardlinked file deletion has ambiguous semantics, librqbit session retains file handles after `remove(delete_files=true)`. Test pattern: do the delete, query every adjacent table, assert no orphans.
6. **Data edge cases.** Null fields, empty collections, very-long strings, unicode (Cyrillic, CJK, emoji), case-insensitive filesystem collisions, paths with spaces, season 0 specials, episodes with null air dates, anthology re-numbering. Mostly affects shows/movies/import. Test pattern: parameterised inputs over the boundary types.
7. **State-machine holes.** Download has 7+ states; transcode has 4+; import has stages. Many off-happy-path transitions are undefined or silently broken: cancel after imported, retry after blocklist, resume on failed, redownload mid-pack. Test pattern: drive every legal transition explicitly, then assert every illegal transition is rejected (not silently accepted).

### Setup, config, auth, indexers, quality profiles

Surface area: ~25 endpoints. Mostly CRUD; concurrency exposure low except around config rotation and quality-profile defaults. Highest risk is **silent auth bypass on DB error** (`auth.rs:127`) — this is the single highest-priority case in the document.

| Where | Problem | Test |
|---|---|---|
| `auth.rs:127` | `get_api_key_from_db` swallows DB errors with `.ok().flatten()` → returns None → middleware decides "no key configured" → allows traffic | Hold a long-running write transaction on `config`; concurrently hit a protected endpoint; assert 401, not 200 |
| `auth.rs:85` | Cast token middleware: malformed media_id in path silently falls through to regular auth, returns 401 with no hint | Request `/api/v1/playback/abc/...?cast_token=...`; assert error message names the parse failure |
| `api/config.rs::rotate_api_key` | No coordination — old key stays valid in any in-flight request's auth state | Two concurrent requests: one rotates, one uses old key on a slow endpoint; document the window and either make rotation atomic or accept the gap |
| `api/config.rs::PUT /config` | `sync_scheduler_intervals` `try_send` silently drops if scheduler not yet running (fresh install) | Fresh DB → POST config with non-default `auto_search_interval` → first scheduler tick uses old default, not new value |
| `api/config.rs::set_default_quality_profile` | Two concurrent set-defaults can leave both profiles with `is_default=1` | Spawn 100 alternating concurrent set-defaults across two profiles; assert exactly one default at end |
| `api/quality_profiles.rs::DELETE` | Usage check + delete not transactional; movie can be assigned to profile in the gap | Concurrent DELETE profile + POST movie assigning that profile; assert one fails cleanly, no FK violation |
| `api/quality_profiles.rs::POST` | No semantic validation — empty `items`, cutoff referencing tier not in items, both accepted | POST profile with `items=[]` + cutoff="bluray_1080p"; assert 400, not 200 |
| `api/indexers.rs::POST` | URL scheme not validated — `file:///etc/passwd`, `gopher://`, etc. accepted | POST indexer with `file://` URL; assert 400. Also test http→https mismatch and IDN homograph |
| `api/indexers.rs::PUT enabled=true` | Trigger fires unconditionally, even when already enabled — easy DOS via repeated PUTs | PUT enabled=true 100 times; assert at most one trigger queued (or rate-limited) |
| `api/indexers.rs::DELETE` | No cascade on related downloads/releases/blocklist rows referencing this indexer | Create indexer → grab via it → DELETE indexer → assert downloads either re-link or are explicitly orphaned with a flag |
| `api/fs.rs::browse` | Path traversal: `..` resolves outside any intended root | GET browse with `?path=/media/library/../../etc`; assert 403 or scoped to library root |
| `api/fs.rs::test_path` | Writetest tempfile orphaned if process killed mid-test | Repeatedly call test_path then SIGKILL; assert no `.kino-writetest-*` accumulation, or sweep on startup |
| `api/metadata_test.rs::test-tmdb` | Doesn't bypass cache — passes against stale cached value when current credentials are wrong **(seam)** | Mock TMDB to 401; warm the cache with a 200 first; assert test endpoint reports failure |
| `api/health.rs::collect_storage` | Filesystem unmount mid-check returns inconsistent per-path errors | Test with one path good, another path's parent unmounted; assert top-level state reflects worst path |
| `init.rs::ensure_defaults` | Concurrent init can race; UNIQUE on config row, but quality_profile insert can be skipped | Call `ensure_defaults` twice in parallel; assert exactly one config row + exactly one default quality profile |
| `startup.rs::cleanup_orphans` | Orphan-cleanup DELETEs not in a transaction; partial failure leaves DB worse than before | Inject failure on the second DELETE; assert all-or-nothing semantics |

### Show / movie lifecycle, metadata refresh, image cache

Surface: ~20 endpoints + metadata sweep + image cache. Heaviest data-edge-case territory.

| Where | Problem | Test |
|---|---|---|
| `api/shows.rs::POST` | Two concurrent follows for same `tmdb_id` race the existence check | Spawn N concurrent POSTs same tmdb_id; assert exactly one show row, no orphan series |
| `api/shows.rs::POST` | TMDB returns show metadata but season-fetch 404s mid-loop → show row exists with partial seasons | Mock TMDB: show OK, season N → 404; assert show is created with seasons up to N-1 and a flag/log so user knows it's incomplete |
| `api/shows.rs::POST` | TMDB partial: `seasons=null` → show with zero episodes → wanted-search permanently skips it | Mock TMDB returning `seasons:null`; assert error visible in UI, not silent zero-episode show |
| `api/shows.rs::POST` | TMDB 429 mid-season-loop swallowed by `.ok()` → silently partial show | Mock TMDB to 429 on 3rd of 5 seasons; assert error surfaced + retry mechanism for missing seasons |
| `api/shows.rs::POST` | Unicode/very-long titles — round-trip integrity, UI overflow | Cyrillic, CJK, emoji titles + 2000-char overview; assert byte-equal round-trip |
| `api/shows.rs::POST` | `monitor_new_items="future"` applied to already-aired show: existing episodes get acquire=1 anyway | Show with first_air_date=2015 + monitor=future; assert old episodes are acquire=0 |
| `api/shows.rs::DELETE` | Cascade: orphan release rows when release was linked to both episode and movie | Create show with linked release → DELETE → assert no orphan releases |
| `api/shows.rs::DELETE` | Race: metadata refresh in flight on same show → refresh writes series rows after DELETE → orphans | Trigger metadata_refresh + DELETE same show in parallel; assert no orphan series/episodes |
| `api/shows.rs::DELETE` | Race: download in flight → librqbit may keep file handle open after `remove(true)` → disk not reclaimed **(seam)** | DELETE show with active download; assert torrent removed AND file handle closed AND disk space freed |
| `api/shows.rs::redownload` | Multi-episode pack: removes one `download_content` row, leaves pack download alive — remaining episodes can still import | Trigger redownload on episode 1 of S01E01-E03 pack; assert episodes 2 + 3 still import correctly |
| `api/shows.rs::PATCH monitor` | Cancel-downloads-for-unmonitored skips imports already in `importing` state → ghost import | Unmonitor season while import in flight; assert import either completes or is cleanly aborted |
| `services/metadata.rs::refresh_sweep` | TMDB drops a season between refreshes → orphan series + episodes never cleaned | Mock TMDB to omit season 0 on 2nd fetch; assert orphans pruned or flagged |
| `services/metadata.rs::refresh_sweep` | Show goes Returning → Ended; `monitor_new_items=future` keeps acquiring future episodes that won't air | Mock status="Ended" on refresh; assert no further future-episode acquire flips |
| `services/metadata.rs::refresh_sweep` | TMDB renumbers episodes between refreshes → existing rows now point to wrong episode metadata | Refresh with renumbered episodes; assert stable identity (use TMDB episode_id, not number) |
| `api/movies.rs::POST` | `release_date=null` from TMDB; `physical_release_date` parsing assumes US release exists; both end up null → search can't sort releases → movie permanently wanted | Mock movie with no US release_dates entry; assert search still runs with sane fallback |
| `api/images.rs::GET` | TMDB image 404 → fallback redirect serves user TMDB's 404 page instead of a placeholder | Mock TMDB image 404; assert kino serves a placeholder, not a TMDB-error redirect |
| `api/images.rs::GET` | Disk full mid-resize → partial cache file → next request retries forever | Mock disk full on cache write; assert partial files cleaned up, error surfaced |
| `api/shows.rs::watch_state` | Stale cache after delete: returns `followed:true` for just-deleted show | DELETE show → immediately query watch-state; assert `followed:false` |
| `api/movies.rs::list` (pagination) | Malformed cursor → 500, not 400 | Send `?cursor=xxxx` malformed base64; assert 400 with helpful message |

### Search → grab → download (the state-machine heart)

Surface: ~25 endpoints + 2 schedulers. **Highest-risk area** — most user-visible bugs in production will originate here. This list is intentionally long.

| Where | Problem | Test |
|---|---|---|
| `download` state machine | Enumerate all states (`queued`, `grabbing`, `downloading`, `seeding`, `importing`, `imported`, `failed`, `cancelled`) and assert every illegal transition is rejected | Write a parameterised test that drives state X → action Y for every (X,Y); assert legal succeed, illegal return 409 |
| `services/download_monitor.rs:186` | `start_download` claim races `cancel_download`: cancel sets failed → claim resurrects it as grabbing | Race start + cancel; assert cancel wins; no resurrection |
| `services/download_monitor.rs:201` | `add_torrent` succeeds in librqbit but DB UPDATE fails → next tick re-adds → librqbit returns AlreadyManaged → DB still wrong **(seam)** | Inject DB failure after add_torrent; assert reconciliation, not duplicate torrents |
| `services/download_monitor.rs:363` | `complete_download` fires when librqbit reports finished, but selected files may be partial → import partial content **(seam)** | Multi-file torrent with subset selection; assert only complete files imported |
| `services/search.rs::grab + scheduler` | Manual grab + wanted-search grab can both INSERT download for same release if no UNIQUE constraint | Race manual + scheduler grab; assert at most one download row per release |
| `services/search.rs:160` (scoring) | Score ties have no tiebreaker → non-deterministic release pick | Two releases with identical score; run grab N times; assert deterministic winner (by created_at, info_hash, or explicit tiebreaker) |
| `services/search.rs:146` (blocklist) | Blocklist by `torrent_info_hash`; null hash on alternate indexer bypasses block | Block release with hash=X; mock alternate indexer returning same torrent with hash=null; assert still blocked (by source_title fallback) |
| `services/search.rs::search_cardigann_indexer` | Cardigann releases with `info_hash=null` panic blocklist check or skipped silently | Mock cardigann returning info_hash=null; assert search completes, releases stored or rejected explicitly |
| `torznab/parse.rs` | Indexer returns HTML error page (Cloudflare challenge, 403 page) instead of XML → parser errors → result silently dropped | Mock Cloudflare-style HTML response; assert error logged + indexer health flagged, not silently skipped |
| `torznab/parse.rs` | Magnet in `guid` AND `enclosure` — which wins? | Mock both; assert documented precedence |
| `parser/mod.rs` | Multi-episode `S01E01-E03` parsed but import doesn't link all 3 episodes to the media | Import `Show.S01E01-E03.1080p`; assert all 3 `media_episode` rows |
| `parser/mod.rs` | Anime absolute numbering "Show.Absolute.01.1080p" → episodes=[] (parsed as year) | Parse anime title with absolute numbering; assert `episodes` array populated |
| `parser/mod.rs` | Cyrillic / CJK title with embedded episode pattern: tokenizer splits wrong | Parse `Шерлок.S01E01.1080p`; assert title and episodes split correctly |
| `parser/mod.rs` | Season pack `S01.1080p` parsed as season pack but import only links episode 1 | Import season pack; assert all season's episodes linked or fail explicitly |
| `api/watch_now.rs` | Best-release tiebreaker missing; `LIMIT 1` is order-dependent | watch-now twice with tied scores; assert deterministic |
| `api/watch_now.rs` | grab returns; `kick_download` queries `state='queued'` but scheduler already moved to `grabbing` → kick no-ops → user waits up to 15min for next scheduler tick | Race kick after a forced state flip; assert kick still triggers start, or document the gap |
| `api/watch_now.rs::watch_now_show_smart` | Next-up picks earliest unwatched aired episode; if `air_date_utc=null`, picks specials with no scene releases → search times out | Show with null-aired specials; assert smart picker skips or surfaces useful error |
| `api/downloads.rs::cancel` | librqbit `remove(delete_files=true)` silently fails (locked file, permission); DB marks failed → orphan files **(seam)** | Mock librqbit remove failure; assert files cleaned up OR DB state reflects failure |
| `api/downloads.rs::retry` | Retry uses stored magnet, not re-resolved from release → if magnet was malformed and release re-parsed, retry repeats failure | Modify release magnet between fail and retry; assert retry uses fresh magnet |
| `api/downloads.rs::blocklist_and_search` | Async re-search spawned after delete; concurrent re-follow can produce duplicate downloads | Blocklist + concurrent re-follow; assert no duplicate downloads |
| `services/download_monitor.rs` | Stall detection uses wall-clock; clock jump (NTP) can false-positive or false-negative **(Clock seam)** | With MockClock: simulate 0-byte progress for 60min, advance clock; assert exactly-one stall transition |
| `services/download_monitor.rs` | Disk full mid-download not surfaced — looks like stall, takes 30-60min to fail **(seam)** | Mock disk full; assert "out of space" surfaced fast, not after stall timeout |
| `download/torrent_client.rs::add_torrent` | URL is HTML details page (cardigann fallback misfires) → librqbit error generic "failed to add torrent" | Mock URL resolution returning HTML; assert error surfaces the URL and resolution failure |
| `download/torrent_client.rs::remove` | `delete_files=true` on hardlinked file — does it unlink only one inode or the backing file? Ambiguous | Hardlinked download → cancel; assert documented behaviour and disk reclaim |
| `services/download_monitor.rs` (session restart) | librqbit session corruption on restart → torrents missing in client but DB shows downloading **(seam)** | Corrupt the librqbit JSON; restart; assert reconciliation marks affected downloads as needing re-add |

### Import → media → library → cleanup

Surface: ~10 endpoints + 2 background services. Filesystem and ffprobe are the failure-mode-rich surface.

| Where | Problem | Test |
|---|---|---|
| `services/import_trigger.rs:160 find_media_file` | Multi-episode torrent → "largest file" heuristic picks one file; other episodes lose their video | Multi-episode torrent; assert each episode linked to its own file (not all to the largest) |
| `import_trigger.rs:191 hardlink fallback` | Hardlink fails (cross-device), falls back to copy, copy fails → media row inserted anyway | Inject copy failure; assert media row NOT inserted, download marked failed cleanly |
| `import_trigger.rs:209` | Unicode/space/special-char paths via `to_string_lossy` → garbled stored path; case-insensitive FS collision on macOS | Import `日本語.mkv` and `Show.MKV` vs `show.mkv`; assert correct round-trip and collision handling |
| `import_trigger.rs:191` | Symlink as source — `hard_link` fails; fallback `copy` copies the symlink target via dereferencing — semantics unclear | Source as symlink; assert documented behaviour (copy target, not link) |
| `import/archive.rs` | Encrypted RAR — `unrar x -y` hangs on password prompt despite `-y` | Encrypted RAR; assert timeout or skip, not hang |
| `import/archive.rs` | Archive bomb (10GB → 500GB) — no extraction size cap | Bomb archive; assert extraction bounded or refused |
| `import/archive.rs` | Archive contains no video file — extracted dir orphaned, find_media_file falls back to base dir → may pick stray file from prior download | Archive with only `.srt` + sample; assert clean failure, no contamination from base dir |
| `import_trigger.rs:217 ffprobe` | ffprobe binary missing → media inserted with null codecs, no error to user | Mock ffprobe missing; assert error surfaced, not silent degradation |
| `import_trigger.rs:217 ffprobe` | ffprobe hangs (large file, slow disk, no timeout) → import worker stalls | Mock ffprobe hang; assert timeout fires |
| `import_trigger.rs` | Media linked to multiple episodes silently allowed (`INSERT OR IGNORE`) — corruption goes undetected | Trigger import twice for same episode; assert one media_episode row, not silently-allowed dupes |
| `api/media.rs::DELETE` | File missing on disk → log+continue; DB row deleted; subsequent playback 404s on other lookups already fetched | DELETE media after manual file delete; assert clean response |
| `api/media.rs::DELETE` | Hardlinked media — DELETE removes one inode; if seeding torrent holds the other, original survives | Delete hardlinked media while torrent seeding; assert documented behaviour, no silent disk leak |
| `cleanup/mod.rs` | Cleanup race: cleanup deletes media row while import is inserting media_episode FK → constraint failure | Concurrent cleanup + import on overlapping media; assert atomic ordering, no FK violation |
| `cleanup/mod.rs` | File delete fails (read-only, NFS lock) but DB row deleted anyway | Read-only file; trigger cleanup; assert all-or-nothing |
| `cleanup/mod.rs` | "Empty subdirs" cleanup hardcoded to `Movies` / `TV` — custom paths never cleaned | Custom library path; trigger cleanup; assert empty dirs removed |
| `cleanup/mod.rs` | Disk-space check uses `df` (POSIX-only) — non-POSIX returns Unknown forever | Mock `df` failure; assert graceful fallback, warning surfaced |
| `api/library.rs::calendar` | DST/year-boundary: `air_date_utc` rendered without client-TZ adjustment → episode shown on wrong day | Episode airing 00:00 UTC Jan 1; client in PST; assert correct calendar bucket |
| `api/library.rs::calendar` | Movie release_date all null → calendar slot defaults to 1970-01-01 | Movie with all release_dates null; assert excluded from calendar, not garbage-dated |
| `api/library.rs::stats` | Time-dependent counts (`datetime('now')` per-query) → race across queries in same `widget()` call | Trigger widget at second boundary; assert counts are internally consistent |
| `api/history.rs` | History rows reference deleted entities; history retention unbounded | Delete movie → assert history rows handled (NULL FK or pruned); add retention policy |
| `api/history.rs` | Cursor pagination with deleted row mid-page can skip entries | Fetch page 1, delete the cursor row, fetch page 2; assert no skipped or duplicated entries |
| `home.rs::continue-watching` | Sort by Option<DateTime> — equal NULLs have non-stable order across requests | Several rows with NULL last_played_at; call N times; assert stable order |
| `home.rs::continue-watching` | Deleted media with playback_position rows present — appears in continue-watching → 404 on resume | Delete media → query continue-watching → assert filtered |

### Playback / transcode / cast / stream

Surface: ~25 endpoints + 3 background services. Most tests need the `Transcoder` seam.

| Where | Problem | Test |
|---|---|---|
| `api/playback.rs` HLS | Master playlist returned before init segment ready (500ms hardcoded sleep) → first segment 30s timeout **(seam)** | Mock IO with 1s+ latency; assert master not returned until init segment exists |
| `api/playback.rs` HLS | Segment index >> max → polls 30s then times out instead of immediate 404 **(seam)** | Request segment 9999; assert 404 fast |
| `api/playback.rs` HLS | Segment file deleted between exists-check and read (ffmpeg ring rotation) → 500 not 404 **(seam)** | Race delete + read; assert 404 |
| `api/playback.rs::transcode` | Idle-timeout cleanup races active session: cleanup decides idle, increment-then-remove, but a new client request just bumped activity → kills active playback **(seam)** | Mock activity bump just before sweep; assert active session preserved |
| `api/playback.rs::progress` | progress > 100% (client clock skew, seek past end) — clamped for Trakt but stored unclamped | POST progress with 1.2× runtime; assert clamping in DB |
| `api/playback.rs::progress` | progress for deleted media silently no-ops; user doesn't see "media gone" | DELETE media → POST progress; assert 404 with message |
| `api/playback.rs::progress` | Final-tick races 80% scrobble: scrobble suppressed but Watched event still fires → "still watching" stays on Trakt | POST 80% then immediate final-tick; assert `/scrobble/stop` is sent |
| `api/playback_cast.rs::cast-token` | Token doesn't bind to media_id at verification — token for media 100 can play media 101 | Issue token for 100, request `/playback/101?cast_token=...`; assert rejected |
| `api/playback_cast.rs::cast-token` | Token issued for media whose file no longer exists | Delete file → issue token; assert verification fails or fast 404 |
| `api/stream.rs::watch-now stream` | librqbit `stream(file_idx)` with out-of-range index → 500 not 400 **(seam)** | Request `file_idx=10` on 3-file torrent; assert 400 |
| `api/stream.rs::watch-now master.m3u8` | Sparse pieces → ffmpeg hangs reading non-sequential bytes — no timeout **(seam)** | Mock sparse torrent; assert timeout/error within reasonable bound |
| `api/stream.rs::segments` | Stream completes → import → temp dir cleaned → in-flight segment requests fail mid-playback | Force import while segment-fetch in flight; assert clean handoff to library player or graceful error |
| `api/playback_probe.rs::probe` | Config (ffmpeg path) read at probe start; concurrent config update → uses old path | Concurrent PUT config + probe; assert snapshot semantics |
| `api/playback_probe.rs::test-transcode` | Spawned during user transcode → encoder resource starvation | Active transcode + concurrent test-transcode; assert semaphore gating |
| `services/trickplay_gen.rs` | media with `runtime_ticks=0` → ffmpeg fails → marked done → `/trickplay.vtt` 404s forever | Force runtime=0; trigger sweep; assert clean fallback (no trickplay flag, not error-marked-done) |
| `services/trickplay_gen.rs` | Defers if `transcode_busy()`; `try_read` returns true on momentary lock contention → trickplay never runs on busy systems | High-frequency short transcodes; assert trickplay eventually runs |
| `services/trickplay_gen.rs` | ffmpeg crash mid-run → some sprites generated, marked done → VTT references missing sprites | Inject crash after sprite N; assert VTT trimmed to existing sprites OR full regenerate next sweep |
| `services/intro_skipper.rs` | Season with <2 episodes deferred forever (no "attempted" marker) | Single-episode show; assert not retried on every sweep |
| `services/intro_skipper.rs` | Semaphore shared with trickplay → intro can starve | Trickplay flooding; assert intro still gets a slot |
| `playback/transcode.rs::cleanup` | Orphan temp dir + new session with same id → sweep deletes active session's files | Pre-create orphan dir at predictable id; start session; trigger sweep; assert active session preserved |
| `api/playback.rs` direct play | File deleted between `info` and `direct` request → race | Delete file mid-flow; assert 404 not 500 |
| `api/playback.rs` subtitles | Stream index from probe doesn't match transcoded stream index → subtitle off-by-one | Multi-audio-stream file → transcode → subtitle by index; assert correct alignment |
| `api/preferences.rs::PATCH home` | Invalid `section_order` accepted → unknown sections persist → frontend silently breaks | PATCH with invented section name; assert 400 |
| `api/preferences.rs::GET home` | Corrupted JSON in DB → `unwrap_or_default()` silently resets layout | Corrupt the JSON; GET; assert error logged + clear fallback |

### Trakt / lists / scheduler / events / webhooks

Surface: ~30 endpoints + 5 schedulers + event bus + webhook dispatch. High concurrency + external-failure surface.

| Where | Problem | Test |
|---|---|---|
| `integrations/trakt/auth.rs` device-poll | Device code expires during polling | Wait beyond expiry; poll; assert 410 + UI restart prompt |
| `integrations/trakt/auth.rs` | Token fetch ok, identity fetch fails → "Connected" with empty username | Mock identity 500 after token success; assert connect succeeds with degraded data, or rolls back |
| `integrations/trakt/client.rs` | Refresh-token refresh races across two concurrent /sync calls | Spawn two /sync, force expiry; assert single refresh-token POST |
| `integrations/trakt/client.rs` | Trakt rotates refresh_token; old one invalid for next refresh | Mock refresh sequence with rotation; assert new token persisted |
| `integrations/trakt/sync.rs` | Watched list paginated 10k+ entries; sync interrupted; resumes from start → duplicates | Kill mid-sync; resume; assert idempotent (uses last_activities watermark) |
| `integrations/trakt/sync.rs` | 429 rate limit during sync → no backoff respect | Mock 429 with Retry-After; assert backoff honoured |
| `integrations/trakt/sync.rs` | 502 / malformed JSON during sync → silent failure | Mock truncated JSON; assert error in `last_error`, not silent task end |
| `integrations/trakt/scrobble.rs` | Queue grows unbounded if Trakt down 24h+ | Mock Trakt offline; flood scrobbles; assert max-age cap (~24h) prunes oldest |
| `integrations/trakt/scrobble.rs` | Scrobble for deleted media — `.unwrap()` panics in lookup | Delete media; on_progress old id; assert graceful skip |
| `integrations/trakt/scrobble.rs` | Duplicate stop-scrobble on client reconnect | Send stop twice; assert one Trakt event |
| `integrations/lists/sync.rs` | List source URL becomes unreachable after create | Create with reachable URL → mock DNS fail → assert list exists, items empty, error visible |
| `integrations/lists/sync.rs` | List returns 0 items (curator emptied) — silent total wipe | Mock empty list; assert wipe is logged and reversible (preserve item history?) |
| `integrations/lists/sync.rs` | Manual refresh + scheduler tick race on same list | Concurrent refresh + tick; assert single poll, no duplicate items |
| `integrations/lists/sync.rs` | List deleted while `apply_poll` in flight → orphan items via FK violation | Delete + poll race; assert FK protection or transactional cleanup |
| `scheduler/mod.rs` | Task hangs forever → `running:true` blocks all future runs | Mock task hang; assert timeout wrapper or cancellation |
| `scheduler/mod.rs` | Two ticks fire same task across restart if `last_run_at` not persisted | Kill mid-task; restart; assert no double-fire |
| `scheduler/mod.rs` | `/tasks/:name/run` for nonexistent task → silent 200 | POST `/tasks/typo/run`; assert 404 |
| `scheduler/mod.rs` | `/tasks/:name/run` while already running → silent dupe (no `running` check?) | POST run twice fast; assert second is skipped or queued |
| `scheduler/mod.rs` | Late-registered task never picked up | Register task post-startup; assert next tick sees it |
| `events/listeners.rs` | Event broadcast capacity 512 — slow subscriber lags → events dropped | Fire 600 events fast with slow listener; assert lag handling, no silent loss |
| `events/listeners.rs` | Event emitted before listeners attached (startup window) | Emit during startup; assert captured (buffer or replay) |
| `events/listeners.rs` | Listener panics → stream not restarted | Mock listener panic; assert listener loop survives, supervised |
| `api/ws.rs` | WS client disconnects mid-broadcast → backpressure | Slow WS reader, fast broadcaster; assert backpressure handling |
| `api/webhooks.rs::POST` | Self-signed cert / TLS error → no override flag | Self-signed target; assert clear error in test endpoint |
| `notification/webhook.rs` | SSRF via user-supplied URL → can reach localhost / metadata endpoints | Webhook URL = `http://127.0.0.1:5000/api/...`; assert allowlist or warning surfaced |
| `notification/webhook.rs` | 5xx → exponential backoff; verify cap | Mock 5xx forever; assert backoff capped, target eventually disabled |
| `notification/webhook.rs` | Disable target mid-dispatch → in-flight delivery still completes | Disable mid-dispatch; assert documented (likely fine, but make it intentional) |
| `api/logs.rs` | Ring buffer overflow loses old entries silently | Flood logs; assert cap is documented and accessible via export window |
| `api/logs.rs::ingest` | Malformed client log JSON → 500 not 400 | POST broken `fields_json`; assert 400 |
| `api/logs.rs::export` | Log line with embedded newline → invalid NDJSON | Long line with `\n`; assert export escapes or splits correctly |
| `api/logs.rs::export` | Export concurrent with retention sweep → cursor jumps over deleted rows | Race export + retention; assert no missed rows (snapshot read) |

## Test style

- **One flow per file.** `tests/flows/follow_show.rs` contains multiple `#[tokio::test]` cases, all about the follow-show journey. Not one giant file.
- **Name tests after user intent**, not endpoints. `follows_show_with_specific_seasons_only`, not `post_shows_with_seasons_filter_returns_200`.
- **Arrange-act-assert structure.** No nested helpers that hide what the test is doing. Shared setup lives in the builder; business logic stays in the test.
- **Own your state.** Each test gets a fresh `TestApp`. No `#[serial]`, no test ordering, no cross-test fixtures.
- **Assert on outcomes, not side effects.** "After this flow, `GET /library` shows the show" — not "the `import_trigger` function was called once."
- **Mock only at the seams we defined.** No in-test monkey-patching of private functions. If a test needs to reach inside, the seam is missing — add it to the harness instead.

## Effort estimate

Realistic, in focused dev-days. Elapsed calendar time ≈ 2× depending on interruption load.

| Chunk | Days | Blocking? |
|---|---|---|
| `test_support` scaffold, `TestAppBuilder`, harness primitives | 2 | Yes — blocks everything below |
| `Clock` trait + refactor priority call sites | 1–2 | Blocks Tier 1 time-sensitive tests |
| `TorrentSession` trait + `FakeTorrentSession` | 2–3 | Blocks Tier 1 flows 2, 3, 4, 11 |
| `Transcoder` trait + `FakeTranscoder` | 2 | Blocks Tier 1 flow 4 (playback) |
| wiremock harnesses (TMDB, Trakt, Torznab) + fixture capture | 2 | Blocks all flow tests |
| Scheduler trigger-completion change | 0.5 | Blocks task-driven tests; cheap |
| Tier 1 flow tests (7 flows, golden paths) | 3–4 | Primary deliverable |
| Tier 2 flow tests (6 flows, golden paths) | 3–4 | Follows Tier 1 |
| Edge-case tests from the inventory (~80–100 cases after dedup/prioritisation) | 10–14 | Drives most regression confidence; can be parallelised across people |
| CI wiring, flake audit, docs | 1 | |

**Total: 26–36 focused days** ≈ **5–7 calendar weeks** for the full programme.

The estimate roughly doubles from the original ~2.5–4 week figure, because the analytical pass surfaced ~150 candidate edge-case tests (catalogued in [Known problems by flow](#known-problems-by-flow)). Each takes roughly 30–90 minutes once the harness exists; many are parameterised variants of the same scaffolding (one external-failure matrix runner covers 30+ rows).

**Phasing options if 5–7 weeks is too long:**
- **Minimum useful**: harness + Clock + TorrentSession + Tier 1 flows + the ~10 highest-severity edge cases (auth bypass, set-default race, download state-machine holes, import file races). ~2 weeks.
- **Comfortable middle**: above + Tier 2 flows + ~40 edge cases focused on the download/import state machine and Trakt sync. ~4 weeks.
- **Full programme**: everything in this doc. ~5–7 weeks.

The seam refactors are still the long pole; everything else can be parallelised.

## Rollout order

1. Land `Clock` and the `test_support` scaffold first, behind the existing `src/tests.rs` using it for one or two tests to prove the shape
2. Land `TorrentSession` next — unlocks the highest-value flows (follow show / movie)
3. Land the wiremock harnesses and fixture library
4. Write Tier 1 flows, merging each as it goes green
5. Land `Transcoder`, write flow 4 (watch now)
6. Tier 2 flows, one per PR

Each trait refactor is a separate, shippable PR — production behaviour unchanged, just an extra `Arc<dyn>` in `AppState`. Reviewing them independently keeps the testing subsystem from landing as one giant change.

## Known limitations

- **No real torrent wire protocol coverage.** FakeTorrentSession simulates the state machine librqbit exposes to us, not actual peer/tracker behaviour. Regressions in how librqbit itself handles real swarms are caught only by manual QA.
- **No codec-accurate playback coverage.** FakeTranscoder verifies orchestration (session lifecycle, playlist structure, segment routing) but never runs FFmpeg. A bug in HW-accel flag selection or filter graph composition will not be caught by this suite — that stays unit-tested inside the transcode module.
- **Rate-limit edge cases untested.** The TMDB semaphore and Trakt POST gate are unit-test concerns; integration tests run with high-permit configs so they don't slow the suite.
- **Clock refactor is incremental.** Not every `Utc::now()` call site will be migrated in the first pass. Tests whose flows touch un-migrated code will still see wall-clock drift in their assertions — we accept this and prioritise migration as those flake.
- **Fixture drift.** Captured TMDB/Trakt JSON goes stale when upstream schemas evolve. No automated detection; re-capture script is a manual periodic chore. A dev-machine smoke test (existing `/api/v1/metadata/test-tmdb` endpoint) is the canary.
- **No multi-user or permissions coverage.** Kino is single-user; this subsystem assumes that and tests one API key.
- **librqbit persistent session interference.** Real `LibrqbitClient` writes session state to disk. Tests use FakeTorrentSession exclusively to sidestep this, so any bug in the real session-persistence path stays in manual-QA territory.
- **FFmpeg binary availability.** Not required — FakeTranscoder has no subprocess dependency. But the existing `probe` endpoints that actually shell out to `ffmpeg` in production are excluded from the integration suite; they're verified at dev time only.
- **No frontend coverage.** A bug that exists purely in React Query invalidation, form handling, or route state won't be caught. That's the Playwright subsystem's job, covered separately.
- **Scheduler concurrency edge cases.** Tests force tasks via the trigger channel, so the actual 3-second tick interplay (two tasks becoming due simultaneously, clock skew across ticks) is not exercised. Scheduler unit tests in `scheduler/mod.rs:481` cover claim atomicity.
- **The edge-case inventory is not exhaustive.** It came from a one-pass analytical read of the codebase; further cases will surface while writing the tests, while doing manual QA, and from production bugs. Treat the inventory as a living document — add to it as new cases are found, prune as cases ship as tests.
- **Some inventory cases are speculative.** A few entries (e.g. SQLite pagination cursor off-by-one, certain librqbit hash-collision scenarios) are derived from code-shape inference, not reproduced bugs. Triage as you write — some will turn out to be non-issues, document and move on.
