# Intro & credits detection

Automatically detect TV show intros and end credits so the player can render Skip Intro / Skip Credits buttons. Uses audio fingerprinting (Chromaprint) to find the shared intro across episodes of a season, and black-frame analysis to refine credits timing.

## What problem it solves

Almost every media server now ships an intro skipper. The user expectation exists; shipping without one feels like a regression. Unlike most competing approaches, this runs entirely locally — no external service, no per-show database lookups, no licensing concerns.

**TV only for v1.** Movies don't have shared intros across episodes (no pairs to fingerprint against). End-credit detection on movies via black-frame analysis alone is a coherent future extension but out of scope.

## How it works

Per season, for each pair of episodes:

1. Extract ~10 minutes of audio from the start of each file (intro mode) or the last ~7 minutes (credits mode).
2. Compute a Chromaprint fingerprint for each audio segment.
3. Compare fingerprints pairwise to find the longest shared audio region.
4. Refine the detected boundaries using silence detection, keyframes, and chapter markers.
5. Persist `intro_start_ms` / `intro_end_ms` / `credits_start_ms` / `credits_end_ms` on the Episode.

The core primitive is `rusty_chromaprint::match_fingerprints`, which takes two fingerprint arrays and returns segments with alignment scores. For our use case — "find one ~15–120s shared region near t=0" — this collapses the detection step into a few lines of code.

### Why Chromaprint

Chromaprint is an audio fingerprint library built for AcoustID. It produces one 32-bit fingerprint point per **0.1238s** of audio — a constant baked into the algorithm. Two files containing the same audio at the same timestamps produce bit-identical fingerprint points in that region. Used in the same way by Jellyfin's Intro Skipper plugin for three years with good results.

We use the `rusty-chromaprint` crate (pure Rust port, MIT licensed, actively maintained) rather than shelling out to the `fpcalc` CLI — keeps the single-binary story intact and exposes a match function that saves us re-implementing the comparison algorithm.

## 1. Audio extraction

FFmpeg pipes raw PCM directly into the fingerprinter — no intermediate file. Sample rate 44.1 kHz, stereo, signed 16-bit interleaved. The fingerprinter resamples internally if needed (rubato), but feeding it at 44.1 kHz is fastest.

```
ffmpeg -hide_banner -loglevel error \
  -ss {start} -i {file} -t {duration} \
  -ac 2 -ar 44100 \
  -vn -sn -dn \
  -f s16le -
```

| Mode | `-ss start` | `-t duration` |
|---|---|---|
| Intro | 0 | min(`intro_analysis_limit`, episode_runtime * 0.25) |
| Credits | max(0, episode_runtime − `credits_analysis_limit`) | `credits_analysis_limit` |

Defaults: `intro_analysis_limit = 600s` (10 min), `credits_analysis_limit = 450s` (7.5 min).

Stdout is read in chunks, cast to `&[i16]`, and fed into `Fingerprinter::consume(...)`. After the stream ends, `finish() → fingerprint()` returns `&[u32]` — one point per 0.1238s of audio.

## 2. Fingerprint comparison

For each pair of episodes in a season (both intro and credits):

```rust
let config = Configuration::preset_test2();   // algorithm 2, matches ffmpeg's chromaprint muxer
let segments = match_fingerprints(&fp_a, &fp_b, &config)?;
```

`match_fingerprints` returns `Vec<Segment>` where each `Segment` has `offset1`, `offset2`, `items_count`, and `score` (mean Hamming distance across matched points, 0–32 — **lower is better**). Convert item offsets to seconds using `config.item_duration_in_seconds()`.

### Pairing strategy

For each episode E in a season, find the episode with the strongest match:

1. Try the adjacent episode first (E±1) — intros rarely change inside a season, so the nearest neighbour is usually a strong match.
2. If no valid segment from the adjacent pair, try up to 3 more episodes in the season.
3. Cache the fingerprint per episode + mode — each file is fingerprinted at most once per season analysis.

A "valid segment" is one that satisfies:

| Filter | Intro | Credits |
|---|---|---|
| Minimum duration | 15s | 15s |
| Maximum duration | 120s | Episode runtime − credits search start − 1s |
| Start position in episode | within first 30s of the file | anywhere in the credits search window |
| Score | `≤ match_score_threshold` (default 10.0) | same |

The last filter comes from `match_fingerprints` internally — already gated.

### Credits: reverse the arrays

Credits live near the end of the file, and content *after* them (teasers, different-length stingers, silence) varies per episode. Applying the intro algorithm unchanged won't work because the "shared segment" doesn't start at t=0.

Solution (same trick as intro-skipper): reverse both fingerprint arrays before calling `match_fingerprints`, then un-reverse the resulting timestamps:

```rust
let adjusted_start = episode_duration - segment_end;
let adjusted_end   = episode_duration - segment_start;
```

After reversal, the problem is identical to intro detection: find the shared audio that starts near t=0 in both reversed streams.

## 3. Precision refinement

Raw Chromaprint boundaries are accurate to ~0.12s but often misaligned with natural breaks — the "skip" button jumps into the middle of a dialog line. Three optional refinement passes snap the boundaries to meaningful frames.

Applied to intro_end and credits_start in order:

### Silence snap

Run FFmpeg's `silencedetect` filter in a window around the detected boundary. Snap to the nearest silence of ≥ 0.33s. Window: ±5s by default.

```
ffmpeg -ss {start} -i {file} -t {duration} \
  -vn -sn -dn \
  -af silencedetect=noise=-50dB:duration=0.1 \
  -f null -
```

Parse `silence_start:` / `silence_end:` from stderr.

### Chapter snap

If the file has embedded chapters (common in mkv rips), snap to the nearest chapter boundary within ±5s. Chapters are already extracted by ffprobe during import (`-show_chapters`), so this is a DB read — no extra ffmpeg pass.

### Keyframe snap

Snap the boundary to the nearest keyframe so the Skip button seeks instantly (no B-frame reconstruction lag).

```
ffmpeg -skip_frame nokey -ss {start} -i {file} -t {duration} \
  -an -dn -sn -vf showinfo -f null -
```

Parse `pts_time:` from stderr. Keyframe snapping happens last — after silence and chapter adjustments — to preserve seek-accuracy.

### Credits: black-frame fallback

Chromaprint accuracy on credits is lower than intros (next-ep teasers, varying credit lengths, mid-credits scenes). For episodes where `match_fingerprints` returns no valid segment, fall back to black-frame detection.

Binary-search the last ~7 minutes for the first run of black frames using FFmpeg's `blackframe` filter:

```
ffmpeg -ss {scan_start} -i {file} -t 2 \
  -an -dn -sn \
  -vf blackframe=amount=50:threshold=10 \
  -f null -
```

A black frame is reported when ≥50% of pixels fall below threshold. The first detected run of ≥3 black frames marks the credits start. Works well for shows that fade to black before credits; struggles when credits are overlaid on footage.

## 4. When it runs

### Trigger

Post-import hook, after entity creation in the Import subsystem (step 8, see `04-import.md`). For the imported episode:

1. Load all other imported episodes in the same season.
2. If fewer than 2 episodes in the season, enqueue the season for later analysis (no-op until a second episode arrives) and stop.
3. Otherwise enqueue an `analyse_season` task.

### Orchestration

Analysis is CPU-heavy (FFmpeg decode dominates, Chromaprint is cheap). Run as a background task under the Scheduler with a small worker pool — default 2 concurrent season analyses, bounded by `max_concurrent_intro_analyses` in Config. Process priority: below normal, same as trickplay.

### Scheduled catch-up

A daily scheduled task re-scans for episodes that are missing intro or credits timestamps and re-runs analysis for their seasons. Catches:

- Episodes imported before the feature was enabled.
- Seasons that had only 1 episode when first analysed.
- Failed analyses that need retry (FFmpeg transient errors).

### Concurrency with trickplay

Both intro analysis and trickplay are post-import background tasks. They share a global "media processing" semaphore (count = 2) so the two subsystems together never exceed 2 concurrent FFmpeg runs. Playback transcoding is a separate semaphore — playback always takes priority.

## 5. Caching

Raw fingerprint arrays cache to disk:

```
{data_path}/fingerprints/
  {episode_id}-intro.bin       ← raw u32 LE bytes
  {episode_id}-credits.bin
```

Cache key is episode_id + mode. Rebuilt only if the media file mtime changes (detected during the next scheduled scan).

Storage cost: ~10 minutes of audio → ~4800 points × 4 bytes = ~20 KB per episode per mode. A full library of 10k episodes is ~400 MB. Negligible.

The cache lets the user re-run analysis with tuned parameters without re-decoding audio (dominant cost).

## 6. Schema

Extend Episode with nullable timing columns:

| Column | Type | Notes |
|--------|------|-------|
| intro_start_ms | INTEGER | Nullable — null = not analysed or not detected |
| intro_end_ms | INTEGER | Nullable |
| credits_start_ms | INTEGER | Nullable |
| credits_end_ms | INTEGER | Nullable |
| intro_analysis_at | TEXT | ISO 8601, null = never analysed |

Stored in milliseconds (Chromaprint gives sub-second precision; ms is the natural unit for web player timecodes).

No separate `media_segment` table — timing is always attached to an episode, never shared across multiple. Flat columns match the existing schema style.

A single column `intro_analysis_at` covers both modes. If analysis ran but found nothing, the columns stay null; the `_at` timestamp distinguishes "not detected" from "never tried".

Extend Show with a per-show skip override:

| Column | Type | Notes |
|--------|------|-------|
| skip_intros | BOOLEAN NOT NULL DEFAULT TRUE | When false, the player never shows or auto-skips the intro for this show |

Covers the "I like the theme song on Succession, skip it everywhere else" case without a timing editor.

## 7. API

### Read

Timing fields are added to the existing episode response payloads. No new endpoints. The player reads them from whatever endpoint feeds `Player.tsx` today.

```json
{
  "id": 1234,
  "title": "Ozymandias",
  ...
  "intro_start_ms": 0,
  "intro_end_ms": 72500,
  "credits_start_ms": 2634000,
  "credits_end_ms": null
}
```

### Manual re-analyse

```
POST /api/v1/seasons/{id}/analyse-intro
```

Enqueues a re-analysis task for the season. Returns 202 Accepted. Used by the scheduled catch-up task and by a future admin tool if defaults mis-detect. No per-episode re-analysis endpoint — analysis is always at the season level (single episodes have nothing to pair against).

**No manual timing override.** Detection is authoritative. Kino targets the "it works" audience, not tinkerers — we'd rather fix detection quality than expose a timing editor. If auto-detection is wrong for a show, the user disables intro-skipping for that show entirely (see per-show toggle in section 8).

## 8. UX

### Settings

Lives under Settings → Playback → Intro & Credits. All global unless noted.

| Setting | Values | Default | Notes |
|---|---|---|---|
| Detect intros | on / off | on | Master toggle for the subsystem |
| Detect credits | on / off | on | Master toggle for the subsystem |
| Auto-skip intros | off / on / **smart** | smart | "smart" = show button on the first episode of a season you watch, auto-skip the rest |
| Auto-skip credits | off / on | off | Credits usually end naturally into the Up Next card; auto-skip is more intrusive |
| Min intro length to show button | 10–30s | 15s | Below this, don't bother — matches detection floor |
| Skip button visible for | whole segment / 10s / 5s | whole segment | Per-user taste; "whole segment" is the least-frustrating default |

**Per-show:** the show detail page has a single toggle, "Show intro for this show" (defaults on). Flipping off stores `Show.skip_intros = false` and the player never shows or auto-skips the intro for any episode of that show.

### Skip button

Rendered by the existing Vidstack player (`Player.tsx`) from the four ms fields returned on episode load.

**Position:** bottom-right corner of the player, above the seek bar. Stays clear of center-screen subtitles. Visible whether controls are shown or hidden.

**Appearance:** small rounded button with chevron icon. Labels: "Skip Intro →", "Skip Credits →". Theme-matched to the existing UI.

**When it appears:**
- Intro button: when `currentTime` enters `[intro_start_ms, intro_end_ms]`, provided `intro_end_ms − intro_start_ms ≥ min_intro_length` AND the show's `skip_intros` is true.
- Credits button: when `currentTime >= credits_start_ms` (stays visible until end of file or user seeks out).

**When it disappears:**
- User clicks it → seek fires, button unmounts.
- User seeks out of the range → unmounts.
- The configurable "visible for" timeout elapses (default: whole segment, no timeout).

**Click behaviour:**
- Intro: seek to `intro_end_ms`.
- Credits: seek to `credits_end_ms` (or end-of-file if that's null).

**Keyboard:** `S` triggers whichever skip button is currently visible. Tab-focusable so keyboard-only users can reach it through the player's focus order.

**Fade animation:** button fades in over 200ms when it appears, fades out over 150ms when it disappears. Respect `prefers-reduced-motion` → disable the fade, instant appear/disappear.

**Accessibility:** `aria-label="Skip intro"` / `"Skip credits"`. When the button mounts, announce it to screen readers via `aria-live="polite"` on a visually-hidden region: "Skip intro available". Do not re-announce if the button re-renders for a new episode — only on initial appearance per episode.

### Auto-skip

When auto-skip fires (either always-on or "smart" mode has triggered):

1. Seek happens silently.
2. A small toast appears bottom-left for **3 seconds**: `Intro skipped · Undo`.
3. Clicking `Undo` seeks back to the pre-skip position and suppresses auto-skip for the rest of this episode.
4. If the user undoes twice in one playback session, auto-skip turns itself off for the remainder of the session (they clearly want to watch something in the intros — stop fighting them). Next session reverts to the configured setting.

**Why the toast matters:** silent auto-skip is indistinguishable from a bug when it's wrong. The toast is the difference between a feature that feels magical and one that feels broken.

**Smart auto-skip state:** needs to know whether you've already watched an intro this season. Backed by the existing `episode.play_count > 0` plus a session flag tracking "intro played through at least once on an episode in this season". Show the button for the first such episode; auto-skip every subsequent episode in the same season.

### Edge cases

| Situation | Behaviour |
|---|---|
| Episode not yet analysed (new import, season had <2 eps) | No button. Silent absence — no loading spinner |
| Analysis ran, detected nothing | No button. Same as above — user doesn't care whether we tried |
| Cold-open episode (intro starts at ~90s) | Button appears at `intro_start_ms` as normal. Auto-skip fires normally — no special case |
| Episode loads mid-intro (resume playback) | Button appears immediately. Auto-skip fires immediately if enabled |
| Show's `skip_intros = false` | No intro button ever, no auto-skip. Credits handling unaffected |
| Both intro and credits buttons want to render | Impossible in practice — intro ends within the first ~25% of runtime, credits start within the last ~15%. Assert-fail in debug if both are visible |

## Entities touched

- **Reads:** Episode (existing fields + new intro/credits columns), Media (file path for FFmpeg), Stream (chapters extracted during import, used for chapter-snap refinement)
- **Updates:** Episode (intro_start_ms, intro_end_ms, credits_start_ms, credits_end_ms, intro_analysis_at)
- **Writes to filesystem:** fingerprint cache files in `{data_path}/fingerprints/`

## Dependencies

- FFmpeg (already required for transcode + probe + trickplay)
- `rusty-chromaprint` crate
- `rustfft` + `rubato` (transitive, via rusty-chromaprint)
- Scheduler subsystem (runs catch-up task, bounds concurrency)
- Import subsystem (post-import trigger)

No new system binaries, no external services.

## Error states

- **FFmpeg fails to decode audio** → log warning, mark episode analysis attempted (to prevent retry loop), continue with other episodes in the season
- **Fingerprint shorter than expected** (truncated file, silent track) → skip this episode in the pairing pool
- **Season has only 1 episode after import** → defer analysis; scheduled catch-up retries when a second episode arrives
- **All pair comparisons fail** (no intro shared across episodes) → leave columns null; the episode has no intro (or intro detection failed — can't distinguish)

## Known limitations

- **Single-episode seasons get no intros.** Limited series, pilot-only seasons, ongoing seasons with one aired episode. Resolved by catch-up once more episodes land.
- **First-episode cold opens** have longer/different intros than the rest of the season. Accuracy on ep 1 is consistently lower than ep 2+; we accept this.
- **Anime with rotating OPs** (same series, different intro every few episodes) will detect only the dominant intro; minority-OP episodes may get no detection. Matches Jellyfin plugin behaviour.
- **Shows with stinger music / recurring motifs** mid-episode can rarely fool the matcher — `match_fingerprints` returns the strongest single alignment, so the intro's shift dominates in practice, but it's not impossible to mis-detect.
- **End credits over live footage** (no fade to black) lose the black-frame fallback and rely on fingerprint alone — accuracy drops ~20%.
- **Season boundaries matter.** Rebrands between seasons (different theme song, new intro sequence) mean analysis is always per-season; cross-season matching is not attempted.
