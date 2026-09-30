# Adaptive multi-source streaming

> **Status:** Design sketch (2026-04-28). Not started. Independent
> of stream-only mode (#34) but disproportionately valuable
> there, since every play in stream-only mode goes through
> watch-now and is therefore exposed to source-rate volatility.

The user clicks Play. They never see "Buffering…". If the active
torrent slows below what playback needs, kino silently grabs an
alternate release of the same film, pre-buffers it ahead of the
playhead, and hot-swaps the ffmpeg input across at a segment
boundary. The HLS player sees a brief `#EXT-X-DISCONTINUITY`
(~1 segment) and continues. No spinner, no fatal error.

Three composing layers:

1. **Bitrate-aware grab race** at start — commit to the candidate
   that proves it can sustain the source bitrate, not just the
   one that *scored* highest.
2. **In-flight rate-watch** — DASH-ABR-style buffer-occupancy
   monitor; trigger swap *before* the buffer drains, not after.
3. **Mid-stream source swap** — extend the existing transcode
   profile-chain "respawn at segment offset" to also restart
   against a different input URL.

## 1. Why this matters

Today's failure mode: user clicks play on a niche film. Top-scored
release has 4 seeders at 80 KB/s; the source needs 5 Mbps. The
player buffers for 30 s, plays for 90 s, buffers again, plays
again. Eventually the user gives up and `acquisition::policy`'s
dead-timeout kicks in (30 min) and watch-now PhaseTwo grabs the
next-best release — but by then the user has already left.

The pieces are all there in scattered form. What's missing is a
coordinator that treats "torrent rate < required rate" as a
first-class signal and acts on it within seconds, not minutes.

## 2. Prior art (verdict)

After research: **nobody ships this combination** —
rate-deficit-driven, mid-playback, across non-byte-identical
torrent releases of the same film. Closest precedents:

| Project | What it does | Why it falls short |
|---|---|---|
| **AutoStream** (Stremio addon) | Multi-source ranking with learned host-penalty scores | Click-time only — no in-flight swap |
| **Stremio core** | Single-source with manual user re-pick | Open feature request #1124 just for *manual* mid-play swap |
| **WebTorrent / Peerflix** | Single torrent at a time | No multi-swarm, no failover |
| **Real-Debrid + Stremio** | Pre-cached HTTP, user picks | List-time selection only |
| **Plex / Jellyfin** | Lower-bitrate ladder rung on slow CPU/network | Same source, different rung — not multi-source |
| **Live-stream broadcast failover** (Ant Media, Akamai) | Mid-stream substitute-stream swap | Identical encoder outputs, not different content releases |
| **DASH / HLS ABR** (BBA, BOLA, Pensieve) | Adaptive bitrate ladders | Single source, multiple pre-encoded rungs |
| **Metalink / aria2** | Multi-source concurrent download | Byte-identical mirrors of one file, not playback |

Naïve mid-stream swap across **non-byte-identical, non-same-cut
releases** is genuinely hard (PAL speedup, theatrical-vs-extended
cuts) and was the hairy part of the original draft. This spec
sidesteps it: candidates are tiered (byte-identical vs. same-cut
vs. cross-cut), and only the first two tiers participate in
mid-stream swaps. Cross-cut swaps are not attempted; they fall
back to today's `watch_now` PhaseTwo retry. See §3.1.

What's reusable from prior art:

- **DASH ABR's buffer-based trigger** (BBA, Huang 2014; BOLA,
  Spiteri 2016) — track `buffer_seconds_ahead`, act when it
  crosses a low watermark, hysteresis to avoid oscillation. We
  reuse the algorithm shape, not the code.
- **AutoStream's host-penalty scoring** — learn from past
  failures, decay on success. Useful as a tie-break in
  `acquisition::policy` for *next-time* candidate selection.
- **Live-stream substitute-stream pattern** — pre-buffer the
  candidate to the playhead's wall-clock position before the
  swap, never from t=0. Translates directly.
- **HLS `#EXT-X-DISCONTINUITY`** — already wired through kino's
  HW-fallback chain. The escape hatch for changing input
  mid-playlist; players handle it with ~1 segment of decoder
  reset.

## 3. The hard parts

### 3.1 Runtime mismatch — solved by candidate-set filter

Two releases of the same 122-min film may differ by ±10 s in
total runtime (PAL speedup, theatrical-vs-extended cuts, intro
padding). At wall-clock t=20:00 in release A, the equivalent
frame in release B might be at 20:03 — or, in the worst case
(director's cut), at a completely different scene.

**Approach: don't try to swap across different cuts at all.**
Two-tier candidate filter, applied at `CandidateSet`
construction time:

1. **Tier 1 — byte-identical.** Same release-group + source +
   codec + resolution, with file size within ±1 %. Verified by
   hashing the first 4 MB of each. PTS is aligned by
   construction; swap is a clean cut with no
   `#EXT-X-DISCONTINUITY` needed.
2. **Tier 2 — same-cut.** TMDB `runtime` matches the active
   release within ±2 s **and** same source tier
   (BluRay-1080p ↔ BluRay-1080p, WEB-DL-1080p ↔ WEB-DL-1080p,
   etc.). Naive `-ss <player_position>` swap; accept up to a
   few seconds of intro-padding drift, surfaced as a one-time
   `#EXT-X-DISCONTINUITY` — the same tax we already pay for
   HW-rung fallback today.
3. **Tier 3 — cross-cut. Excluded.** Different runtimes or
   source tiers never enter the candidate set, so the racer
   never picks them as swap targets. If the active source dies
   and no Tier 1/2 candidate exists, fall back to today's
   `watch_now` PhaseTwo sequential retry (which visibly
   restarts playback — fine, this is the rare case).

The runtime-match check is essentially free: TMDB `runtime`
metadata is already stored on every `movie` and `episode` row.
Source-tier match reads the existing `release.source` /
`release.video_codec` / `release.resolution` parser fields.
The byte-identical hash check downloads ~4 MB extra per
candidate, only at race-start time — bounded cost.

**No chromaprint, no fingerprint matching, no cross-correlation.**
The complexity ceiling is "compare two strings and a number".

### 3.2 Stream layout differences

Release A has TrueHD Atmos + AC-3 5.1 + English subs at indices
0/1/2; release B has DTS-HD MA + AAC stereo + Spanish subs at
0/1/2. The HLS variant the player is consuming has one audio
codec baked in.

**Approach**: kino's existing `decision.rs` already iterates
audio streams to pick a compatible codec ("multi-audio
compatibility" — subsystem 05). Re-run the picker against the
new source's stream layout; the output codec is unchanged
(client-compatible AAC or AC-3 passthrough), so the HLS variant
manifest stays valid. If the picker can't find an equivalent
audio language match, downgrade quietly to the source's default
audio and surface a one-time toast ("audio language changed for
remainder of stream").

### 3.3 Discontinuity tax

`#EXT-X-DISCONTINUITY` resets the audio decoder on most
players. Empirically: ~250–500 ms of output-side stall while
the player re-aligns. The user sees a perceptible glitch. This
is the cost of the feature, and it's already paid by the HW
fallback path today.

**Approach**: accept the glitch as the cost. Surface it in the
notification stream so the user knows *why* the glitch happened
("switched to a faster source"). Don't try to hide it.

### 3.4 No same-cut/byte-identical candidates exist

For obscure films or fresh-release content, only one release may
exist, or all candidates are different cuts. In that case the
candidate set has zero tier-1/tier-2 swap targets and adaptive
streaming **degrades to a no-op**: the racer commits to the
first-grabbed source and steady-state playback is identical to
today's behaviour (with `watch_now` PhaseTwo as the failure
fallback). The feature is opportunistic, not load-bearing.

### 3.5 Wasted bandwidth

Racing N candidates means downloading N × prefix bytes that we
throw away once the winner is chosen. For a 30 s race window at
5 Mbps × 3 candidates = ~5 MB of waste per session. Acceptable
in absolute terms; need to cap the race fan-out (default 3) and
disqualify candidates aggressively (~5 s after the leader is
clearly ahead).

## 4. Scope

### In scope

- Bitrate-aware grab race at watch-now start (top-N candidates,
  parallel grab, race-to-quorum).
- In-flight rate watch using DASH-ABR-style buffer-occupancy
  signal, triggering hot-standby grab before the buffer drains.
- Mid-stream source swap via the existing transcode-respawn
  machinery extended with a "swap input URL" rung.
- Audio-fingerprint-based swap-point alignment (reusing
  `intro_skipper`'s chromaprint plumbing).
- Host/release penalty scoring as a tie-break in
  `acquisition::policy` (learn from this user's past sessions).
- New `RateDeficit` AppEvent + UI surfacing ("switched to a
  faster source for the remainder").
- Settings knobs for race fan-out, rate-margin, max swaps per
  session.
- Works in both library mode and stream-only mode (#34); same
  code path, different lifecycle around it.

### Explicitly out of scope

- **Pre-emptive multi-source download for already-watched
  content** — only races when the rate signal demands it.
- **Frame-accurate cross-fade** — we accept the
  `#EXT-X-DISCONTINUITY` glitch.
- **Cross-quality swap** (1080p A → 4K B mid-stream). The
  candidate set is filtered to releases of comparable
  resolution before the race so swaps don't change the visible
  picture quality drastically. Future enhancement.
- **Live re-encode at lower bitrate when no fast source
  exists.** The decision engine already handles client-driven
  bitrate ladders; we don't conflate that with source-driven
  fallback.
- **Persistence of the candidate set across kino restarts.** The
  ranked candidate list is in-memory per session; on restart
  we re-search. Acceptable: search results live in the
  `release` table; what we don't persist is the *active racer
  set*.

## 5. Architecture

```
                ┌────────────────────────────────────────────────────┐
                │ acquisition::adaptive::CandidateSet                │
                │   top-N ranked Releases for (kind, entity_id)      │
                │   built by extending acquisition::policy           │
                └──────────────┬─────────────────────────────────────┘
                               │
                ┌──────────────▼─────────────────────────────────────┐
                │ download::race::Race                               │
                │   spawn parallel grabs (N=3 default)               │
                │   collect per-torrent rate samples (1s cadence)    │
                │   declare winner when sustained_rate ≥ required    │
                │     × margin for 8s, OR after 30s deadline         │
                │   drop losers via Session::delete()                │
                └──────────────┬─────────────────────────────────────┘
                               │
                ┌──────────────▼─────────────────────────────────────┐
                │ playback::transcode (existing, extended)           │
                │   ffmpeg input = librqbit /stream/{n} URL          │
                │   produces HLS to scratch dir                      │
                └──────────────┬─────────────────────────────────────┘
                               │
              ┌────────────────┴───────────────────────────────────┐
              │                                                    │
              │  ┌────────────────────────────────────────────┐   │
              │  │ playback::buffer_monitor (new)             │   │
              │  │   sample buffer_seconds_ahead every 2s     │   │
              │  │   emit RateDeficit when below low_watermark│   │
              │  └────────────┬───────────────────────────────┘   │
              │               │                                    │
              │  ┌────────────▼───────────────────────────────┐   │
              │  │ download::race::start_standby (new)        │   │
              │  │   pick next candidate from CandidateSet    │   │
              │  │   grab + prebuffer to current playhead     │   │
              │  │     (chromaprint-aligned offset)           │   │
              │  └────────────┬───────────────────────────────┘   │
              │               │                                    │
              │  ┌────────────▼───────────────────────────────┐   │
              │  │ playback::transcode::swap_input (new)      │   │
              │  │   on standby ready: respawn at next        │   │
              │  │     segment boundary against new URL       │   │
              │  │   (extends existing respawn_next_rung)     │   │
              │  └────────────────────────────────────────────┘   │
              └────────────────────────────────────────────────────┘
```

### 5.1 Required-rate calculation

`required_bps = source_bps × safety_margin` where
`source_bps = max(stream_probe_cache[file].streams.bit_rate, format.bit_rate)`
and `safety_margin = 1.2` (default). The `ProbeResult` is already
populated by `playback::stream_probe` on partial files (5 MB
threshold). For the very-first race window before any candidate
has 5 MB, fall back to a conservative
`required_bps_estimate` derived from release-name parsing
(`source: BluRay-1080p` → 8 Mbps default) — already present in
the release parser.

### 5.2 Buffer-occupancy signal

DASH-ABR's BBA algorithm in 50 lines: track
`buffer_seconds_ahead = (last_segment_produced - last_segment_played) × segment_duration`.
Emit `RateDeficit` when it falls below
`low_watermark_secs` (default 8 s — slightly above the default
6 s segment length so we react before stalling). Hysteresis:
require 3 consecutive samples below threshold before triggering
a swap, to filter out transient dips. Once a swap completes,
suppress new swaps for `swap_cooldown_secs` (default 30 s) to
prevent flapping.

### 5.3 Why this hooks into the existing respawn machinery

`playback::transcode::respawn_next_rung` (transcode.rs:1425-1483)
already does the hard part: kill current ffmpeg gracefully,
preserve `session_id` + `temp_dir`, version init segment as
`init_v{N+1}.mp4` and segments as `segment_v{N+1}_NNN.m4s`,
emit `#EXT-X-DISCONTINUITY` in the HLS playlist, restart ffmpeg
at `start_time = current_segment × segment_duration`. The only
new variable is **the input URL**. We add a `RespawnReason` enum
with variants `HwFailure` (existing) and `SourceSwap` (new); the
swap path bypasses the profile-chain advance logic and instead
takes a `new_input_url` argument.

The 90% reuse here is the load-bearing simplification of this
proposal.

## 6. What kino already has

Mapped from the codebase audit. The good news is most building
blocks exist:

| Building block | Where | Reuse |
|---|---|---|
| Multi-torrent native session | `librqbit::Session` (already in use) | `add_torrent()` / `delete()` / `pause()` work; per-torrent stats via `ManagedTorrent::stats()` |
| Per-torrent rate sampling | `download::monitor.rs` 3 s tick | Tighten to 1 s during active race; emit per-torrent rate to a registry |
| HLS respawn at segment offset | `playback::transcode::respawn_next_rung:1425` | Extend with a swap-input variant |
| Bitrate ground truth | `playback::stream_probe::ProbeResult.format.bit_rate` and `streams[].bit_rate` | Compute `required_bps` from this |
| Multi-audio decision logic | `playback::decision.rs` (multi-audio compatibility iteration) | Re-run on swap to pick equivalent codec on new source |
| Runtime + source-tier metadata | `movie.runtime` / `episode.runtime`; `release.source`, `release.resolution`, `release.video_codec`, `release.size` | Drives the tier-1/tier-2 candidate filter — already on every row |
| Sequential alternate-release retry | `watch_now::handlers::attempt_fulfill_and_kick:885` | Becomes a fallback when racing fails; keep as belt-and-braces |
| Release scoring | `acquisition::policy:401` (rank × 1000 + log10(seeders) × 10 + proper/repack bonuses) | Add host-penalty term; compute top-N instead of top-1 |
| Search persistence | `release` table | Already top-N; just stop discarding rows after picking the winner |

### What's net-new

1. **`download::race`** module (~400 LOC) — orchestrator: spawn N
   candidates, sample per-torrent rates, declare a winner, drop
   losers. State machine: `Racing → Won(release_id) → SteadyState`.
2. **`playback::buffer_monitor`** (~150 LOC) — sample buffer
   occupancy, emit `RateDeficit` AppEvent, debounce.
3. **`acquisition::candidate_set`** (~250 LOC) — top-N candidate
   computation with **tier-1 (byte-identical) / tier-2 (same-cut)
   swap-eligibility tagging**, host-penalty scoring, in-memory
   ranked list per active session.
4. **`playback::transcode::swap_input`** (~150 LOC, mostly extension
   of `respawn_next_rung`) — new respawn reason, new input URL,
   naive `-ss <player_position>` (no fingerprint alignment).
5. **`download::byte_identical_check`** (~80 LOC) — fetch first
   4 MB of each candidate, hash, group identical-hash candidates
   into the tier-1 pool.
6. **DB schema**: `host_penalty (host_origin TEXT PK,
   failure_count INT, last_success TEXT)` for the AutoStream-style
   learned penalty. Optional `swap_event` table for diagnostics.
7. **AppEvent** variants: `RateDeficit { download_id, actual_bps,
   required_bps, buffer_seconds }`, `SourceSwapStarted`,
   `SourceSwapCompleted { from_release_id, to_release_id,
   tier: "byte_identical" | "same_cut" }`.

Total new code: ~1000 LOC + ~350 LOC of tests. Substantial but
focused; no librqbit fork, no chromaprint integration, no
upstream patches.

## 7. Per-subsystem changes

| Subsystem | File(s) | Change | Size |
|---|---|---|---|
| Acquisition | `acquisition/candidate_set.rs` (new) | Top-N ranked list with tier-1/tier-2 swap-eligibility tagging; host-penalty scoring | medium |
| Acquisition | `acquisition/policy.rs` | New scoring term: `+ host_penalty(host)` | trivial |
| Download | `download/race.rs` (new) | Race orchestrator | medium |
| Download | `download/byte_identical_check.rs` (new) | First-4 MB hash to confirm tier-1 grouping | small |
| Download | `download/monitor.rs` | Rate sampling tightened to 1 s during active race; per-torrent rate registry | small |
| Download | `download/torrent_client.rs` | Spawn / drop additional torrents from the racer | small |
| Playback | `playback/buffer_monitor.rs` (new) | Buffer-occupancy signal, BBA trigger | small |
| Playback | `playback/transcode.rs` | New `RespawnReason::SourceSwap`; `swap_input()` wrapper around `respawn_next_rung`; naive `-ss <player_position>` for the new input | small |
| Playback | `playback/decision.rs` | Re-run multi-audio picker against new source on swap | trivial |
| Watch-now | `watch_now/handlers.rs` | PhaseOne hands off to `Race` instead of grabbing a single release | small |
| Events | `events/mod.rs` | New variants + `event_type_matches_serde_tag` test arms | small |
| Settings | `settings/config.rs` + migration | New columns (§9) | trivial |
| Settings | DB | `host_penalty` table | trivial |
| Frontend | Player | "Switched to a faster source" toast on `SourceSwapCompleted` | trivial |
| Frontend | Settings | Adaptive-streaming knobs section | small |

## 8. Lifecycle: a slow torrent and a graceful swap

Concrete walkthrough so the moving parts are anchored.

1. User clicks Play on a 122-min BluRay-1080p film.
   `CandidateSet::top(3)` returns:
   - **A** — `Movie.2024.1080p.BluRay.x264-GROUP1`, 8.5 Mbps,
     42 seeders, runtime 122:01
   - **B** — *byte-identical re-upload of A* on a different
     tracker (same group + same first-4 MB hash), 18 seeders,
     runtime 122:01 → **tier-1 swap-eligible with A**
   - **C** — `Movie.2024.1080p.BluRay.x264-GROUP2`, 5.0 Mbps,
     120 seeders, runtime 122:01 → **tier-2 swap-eligible with
     A and B**
2. `download::race::Race` adds A, B, C to librqbit
   simultaneously, all unpaused, all writing to scratch via
   `EphemeralStorage` (#34). `only_files` set to each picked
   file.
3. After ~12 s, A is downloading at 1.4 Mbps (bad), B at
   9.8 Mbps (good), C at 6.2 Mbps (marginal). `Race` declares B
   the winner. A is dropped; **C is kept in the standby pool**
   (one slot, paused) since it's the highest-ranked tier-2
   candidate that might be needed later.
4. ffmpeg starts against B's `/stream/{file_idx}` URL.
   `playback::stream_probe` runs against B's partial file at
   5 MB and refines `required_bps = 8.2 × 1.2 = 9.84 Mbps`.
5. Player begins playback. `playback::buffer_monitor` samples
   `buffer_seconds_ahead` every 2 s.
6. At minute 14 of playback, B's seeders drop. Buffer falls from
   18 s ahead to 9 s ahead over 25 s. Three samples in a row
   below the 8 s low-watermark trigger `RateDeficit`.
7. `download::race::start_standby(B)` unpauses C (already in
   the standby pool, candidate set knows it's tier-2 with B).
   No fingerprinting: C will be entered at the player's wall-
   clock position `14:32` via naive `-ss`.
8. C reaches "buffer ahead" parity in 6 s. `swap_input` fires:
   ffmpeg gracefully exits, restarts against C's `/stream` URL
   at `-ss 00:14:32`, emits `#EXT-X-DISCONTINUITY`, writes
   `init_v2.mp4`, segments numbered `segment_v2_*.m4s`.
9. C's intro is 1.4 s shorter than B's, so the new ffmpeg
   actually lands ~1.4 s earlier in the film than B did.
   Player's hls.js sees the discontinuity, resets the audio
   decoder, resumes; user perceives a brief glitch + a tiny
   scene replay. UI shows a "Switched to a faster source" toast.
10. B is dropped via `Session::delete()`. `host_penalty(B.host)`
    increments. C continues as the active source.
    `SourceSwapCompleted { tier: "same_cut" }` AppEvent fires.

If C *also* slows later, the loop repeats with the next
candidate; after `max_swaps_per_session` (default 3), give up
gracefully — the user sees a "Stream quality degraded" toast and
playback continues with whatever buffer remains. Watch-now
PhaseTwo's existing sequential retry kicks in as the
belt-and-braces fallback if the player fully stalls.

## 9. Settings & defaults

| Setting | Default | Rationale |
|---|---|---|
| `adaptive_streaming_enabled` | `1` | Worth-the-bandwidth-cost default |
| `race_fan_out` | `3` | Diminishing returns past 3; 5 MB waste per session at 5 Mbps × 30 s |
| `race_quorum_secs` | `8` | Sustained rate above threshold for 8 s before declaring winner |
| `race_deadline_secs` | `30` | Give up racing and pick the leader if no clear winner emerges |
| `rate_safety_margin` | `1.2` | 20 % overhead for transient dips |
| `buffer_low_watermark_secs` | `8` | Trigger swap-prep below this; ≥ segment_length |
| `buffer_low_consecutive_samples` | `3` | Hysteresis to avoid flapping on transient dips |
| `swap_cooldown_secs` | `30` | Don't swap again within this window |
| `max_swaps_per_session` | `3` | Bounded loop |
| `host_penalty_failure_increment` | `5` | Soft penalty: 5 swap-loss events ≈ rank drop of one quality tier |
| `host_penalty_decay_per_day` | `1` | Slow forgive-and-forget |

All knobs hidden under a "Streaming quality" advanced section in
Settings; defaults work for ~95 % of cases.

## 10. UX

- **Default**: invisible. Playback works; if a swap happens, a
  small "Switched source" toast (3 s, dismissible) at the top of
  the player.
- **Diagnostics view** in Settings → Health: per-session swap
  log, host-penalty leaderboard, race outcomes. Operator-only
  surface.
- **Failure escalation**: when `max_swaps_per_session` is hit,
  toast becomes a banner: "Stream quality may be degraded —
  no faster source found". User can hit a "try again" button
  which re-runs CandidateSet from scratch (re-search via
  indexers, build a new top-N).
- **No user input required during a swap**. The flow is fully
  automatic; user-facing controls are off-by-default debug
  tooling.

## 11. Testing

- **Unit**:
  - `download/race/winner_selection.rs` — given synthetic
    rate streams, the right candidate wins.
  - `playback/buffer_monitor/hysteresis.rs` — transient dips
    don't trigger; sustained dips do.
  - `acquisition/candidate_set/tier_filter.rs` — different
    runtime ⇒ excluded from tier 2; same group + same hash ⇒
    grouped into tier 1; different source tier ⇒ excluded.
  - `download/byte_identical_check.rs` — first-4 MB hash
    correctly groups two re-uploads of the same MKV; rejects
    re-encoded variant.
  - `acquisition/candidate_set/host_penalty.rs` — penalty
    increments on failure, decays on success, applied to
    score.
- **Integration** (against the librqbit fake):
  - `flow_tests/adaptive_race_clear_winner.rs` — three
    candidates, clear winner emerges in 12 s, others dropped.
  - `flow_tests/adaptive_race_no_winner.rs` — all three slow,
    deadline hits, leader picked, normal playback continues
    (degraded but functional).
  - `flow_tests/adaptive_swap_midstream.rs` — playback
    starts, rate-deficit triggered, swap completes, player
    sees one `#EXT-X-DISCONTINUITY` and resumes.
  - `flow_tests/adaptive_swap_alignment.rs` — chromaprint
    drift correctly handled (verify by comparing decoded
    audio frame at swap boundary to ground truth).
  - `flow_tests/adaptive_swap_max_swaps.rs` — third swap is
    suppressed, banner shown.
- **Property test**: rate-deficit detector never flaps within
  `swap_cooldown_secs`.
- **Manual smoke**: a deliberately throttled torrent (qdisc on
  the dev container) → swap to a faster mirror should land
  within 20 s of throttle activation.

## 12. Phasing

### Phase 1 — Foundations (no user-visible change)

- `acquisition::candidate_set` (top-N + tier tagging +
  scoring extensions)
- `download::byte_identical_check` (first-4 MB hash grouping)
- `download::race` (the orchestrator, but always racing N=1
  while disabled — same code path, no actual racing)
- `playback::buffer_monitor` + `RateDeficit` AppEvent (emitted
  but no consumer)
- DB migrations + config columns
- Tests for everything

Outcome: instrumented but inert. Diagnostics surface in
`/api/v1/health` shows rate vs required and the candidate
tiering.

### Phase 2 — Race-on-start (no mid-stream swap)

- Wire watch-now PhaseOne to use the racer with `N=3`
- No `swap_input` yet — if the chosen winner slows mid-stream,
  fallback to existing PhaseTwo sequential retry
- Settings UI: enable/disable + race fan-out

Outcome: faster initial-source quality, no flicker. Bigger
bandwidth bill at start but sub-second swap is not yet possible.

### Phase 3 — Mid-stream swap (tier 1 + tier 2)

- `playback::transcode::swap_input` (extends `respawn_next_rung`
  with a new input URL and naive `-ss <player_position>`)
- Buffer-monitor → standby grab → swap pipeline
- UI toast on swap, distinguishing tier-1 (cut-clean) from
  tier-2 (small-glitch) in the message

Outcome: the headline feature ships. User sees zero buffering
spinners on slow torrents in the common case where tier-1 or
tier-2 candidates exist; the rare cross-cut case falls back to
PhaseTwo retry as today.

### Phase 4 — Polish

- Host-penalty learning + decay
- Diagnostics view in Settings → Health
- `max_swaps_per_session` + degraded-banner UX
- Optional: `chromaprint`-based cross-cut alignment as a deferred
  enhancement, only if real-world telemetry shows tier-3
  cross-cut swaps would be valuable enough to justify the cost

## 13. Open questions

1. **Drift bound on tier-2 swaps in the wild.** ±2 s runtime
   tolerance is a guess. Verify against a sample of TMDB
   `runtime` vs actual file durations across our index of
   recent releases — the metadata is sometimes inaccurate. If
   the noise floor is bigger than the intro-padding signal,
   we may need to tighten to ±1 s or fall back to actual
   duration probed at search time.
2. **Different framerates within the same source tier**. Even
   with same-source-tier filter, a few releases mix 24 fps and
   23.976 fps. ffmpeg's `-vsync vfr` should handle this if the
   transcode output framerate is fixed, but verify; might need
   explicit `-r` on the swap respawn.
3. **`update_only_files()` runtime cost**. If the user is
   binge-watching a season-pack torrent, we change which file
   is active post-grab. Confirm this is cheap in librqbit.
4. **Race fan-out vs VPN port-forward saturation**. Three
   torrents on one VPN tunnel may saturate inbound peer slots.
   Verify librqbit's per-torrent peer caps are reasonable.
5. **Trakt scrobble continuity across swap**. Scrobble fires
   on threshold % of duration. Tier-2 swaps to a release with
   slightly different runtime could cross the 80 % threshold
   twice. Use a one-shot guard keyed on `(entity_id, session_id)`.
6. **Cast receiver behaviour through `#EXT-X-DISCONTINUITY`**.
   Empirical test against Chromecast Ultra and Google TV
   Streamer; some older receivers reset more aggressively than
   browser hls.js.
7. **Subtitles across swap**. External WebVTT sidecars are
   per-`(kind, entity_id)` so they survive. Burned-in subs
   from one release shouldn't be carried to a different
   release. Reasonable default: if release A burns subs and
   release B doesn't, suppress burn-in for the swap and
   surface a toast.
8. **Stream-only mode interaction**. With #34's
   `EphemeralStorage`, three concurrent racers each have their
   own rolling buffer (~800 MB each = ~2.4 GB peak during a
   30 s race). On a 1 GB tmpfs Pi this won't fit. Two options:
   (a) shrink keep-ahead to 64 MB during race window, expand
   on win; (b) require `stream_scratch_max_bytes ≥
   race_fan_out × keep_ahead_bytes` and fail config-validation
   otherwise. Lean toward (a).
9. **Tier-1 byte-identical false positives**. First-4 MB hash
   match is a strong but not absolute signal — two releases
   could share an identical mux header but diverge later
   (e.g. same encoder pre-sets, different cuts). Mitigation:
   if a tier-1 swap visibly drifts (player reports a hard
   stall after swap), demote that pair to tier-2 in the
   in-memory candidate set for the rest of the session.

## 14. References

- `docs/roadmap/34-stream-only-mode.md` — orthogonal but
  synergistic
- `docs/subsystems/03-download.md` — librqbit + monitor
- `docs/subsystems/05-playback.md` — transcode HW-fallback
  chain (the reuse target)
- `docs/architecture/state-machines.md` — `WatchNowPhase`
  background-retry sub-machine

External:

- [BBA paper (Huang et al. 2014)](https://web.stanford.edu/class/cs244/papers/sigcomm2014-bba.pdf)
- [BOLA (Spiteri 2016)](https://arxiv.org/pdf/1601.06748)
- [AutoStream addon (Stremio)](https://github.com/keypop3750/AutoStream)
- [Stremio open feature request: in-player source switch (#1124)](https://github.com/Stremio/stremio-web/issues/1124)
- [librqbit Session API on docs.rs](https://docs.rs/librqbit/latest/librqbit/struct.Session.html)
- [librqbit ManagedTorrent API on docs.rs](https://docs.rs/librqbit/latest/librqbit/struct.ManagedTorrent.html)
- [Ant Media stream failover (live-broadcast prior art)](https://docs.antmedia.io/guides/playing-live-stream/stream-failover-playback/)
- [Metalink RFC 5854 (multi-source download)](https://www.rfc-editor.org/rfc/rfc5854) — for the racing-math, not the use case
