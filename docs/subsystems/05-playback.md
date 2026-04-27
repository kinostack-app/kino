# Playback subsystem

Serves video to clients over HTTP. Handles direct play, remux, and transcoding with hardware-accelerated fallback; delivers subtitles via sidecar or burn-in; tracks watch progress per movie/episode.

## Responsibilities

- Serve media files with HTTP `Range` support (seeking)
- Decide per-request whether to direct-play, remux, or transcode
- Orchestrate FFmpeg with hardware-accelerated encoding + graceful software fallback
- Surface `TranscodeReason` observability so users + logs see *why* a transcode happened
- Deliver subtitles (text passthrough or burn-in for image-based)
- Track playback position per movie/episode + mark watched at threshold
- Manage session lifecycle: inactivity kill, graceful shutdown, sliding-window cleanup

## Decision engine

Every playback request runs through a per-stream compatibility check that produces one of three outcomes, accumulating a `TranscodeReason` bitmask along the way:

1. **Direct play** — serve the source file bytes with `Range` support. No FFmpeg.
2. **Remux (stream copy)** — `-c:v copy -c:a copy` into fMP4 HLS. Near-zero CPU. Triggered when only the container mismatches — e.g., HEVC in MKV delivered to Safari needs MP4 wrapping, audio codec is compatible.
3. **Transcode** — full video and/or audio re-encode via FFmpeg.

### Decision order

First mismatch determines the outcome and sets a reason flag:

`container → video codec → video profile → video level → pixel format → bit depth → HDR metadata → audio codec → audio channels → audio sample rate → subtitle codec`

Each failure contributes a flag to `TranscodeReason`: `ContainerNotSupported`, `VideoCodecNotSupported`, `VideoLevelNotSupported`, `VideoBitDepthNotSupported`, `VideoRangeTypeNotSupported`, `AudioCodecNotSupported`, `AudioChannelsNotSupported`, `SubtitleCodecNotSupported`, plus bitrate variants. The bitmask is surfaced on every transcoded URL as `?tr=1,4,16` and in `PlayPrepareReply.transcode_reasons` so the UI can render "transcoding because: audio codec not supported."

### Multi-audio compatibility

Most remuxes include multiple audio tracks — a primary high-quality track (TrueHD Atmos, DTS-HD MA) and a compatibility track (AC-3 5.1, EAC3). Before transcoding audio, the decision engine iterates the full `stream` row set and picks the first track whose codec is in the client's allowlist. A TrueHD + AC-3 MKV going to a Chromecast Ultra selects AC-3 for remux; no audio transcode.

### HDR and Dolby Vision

Source color metadata (`color_transfer`, `color_primaries`, `color_space`, `pix_fmt`, Dolby Vision RPU presence, DV profile/level) is extracted at import time and stored on the `stream` row. The decision engine reads these and branches:

| Source range | Client range | Decision |
|---|---|---|
| HDR10 | HDR10-capable | Direct play / remux |
| HLG | HLG-capable | Direct play / remux |
| HDR10+ | HDR10 only | Remux + `-bsf:v hevc_metadata=remove_hdr10plus=1` (strip dynamic metadata, keep HDR10 base) |
| DV Profile 5 (IPT-PQ-C2) | DV-capable | Direct play |
| DV Profile 5 | HDR10 or SDR | Tone-map to SDR (no HDR10 fallback exists for P5) |
| DV Profile 7 (dual-layer) | DV Profile 8.1-capable | Remux + bsf convert to 8.1 via RPU rewrite |
| DV Profile 7 | HDR10 only | Remux + `-bsf:v hevc_metadata=remove_dovi=1` (strip DV, keep HDR10 base) |
| DV Profile 8.1 | DV-capable | Direct play |
| DV Profile 8.1 | HDR10 or SDR | Strip DV → HDR10 passthrough; tone-map if SDR |
| Any HDR | SDR-only client | Tone-map to SDR via FFmpeg filter chain |

The "strip dynamic metadata" paths use bitstream filters — no re-encode — so they're near-zero CPU. Tone-map-to-SDR requires re-encode and uses the fastest available path: `libplacebo` (Vulkan) > `tonemap_vaapi` / `tonemap_cuda` (HW) > `tonemapx` (SW). Default algorithm is Hable (preserves dark + bright detail).

## Transcode pipeline

### Profile chain with in-stream fallback

Each session is initialised with an ordered profile chain for its stream type, highest-priority first:

```
[HardwareTranscode, Remux, SoftwareTranscode]
```

The session starts with the hardware rung. If FFmpeg exits non-zero with a known HWA-failure signature (`No NVENC capable devices found`, `Failed to init hwdevice`, `Device creation failed`, `vaapi_encode: driver does not support`, etc.), the session advances to the next profile and restarts FFmpeg at the current segment offset. The client sees a brief stall at the segment boundary, not a broken stream.

After three consecutive FFmpeg exits at the same rung — or when the chain is exhausted — the session is terminated and the client receives a fatal error card.

### Hardware acceleration

Available backends are probed once at startup by running a real trial encode per backend against a synthetic `lavfi testsrc` input. Probe output is parsed for vendor fingerprints (`Intel iHD driver`, `Mesa Gallium driver`) and success/failure classification, then cached as a typed capability matrix:

```
HwCapabilities {
    vaapi:        Available  { device: "/dev/dri/renderD128", driver: "iHD" },
    nvenc:        Unavailable { reason: "No NVENC capable devices found" },
    qsv:          Available  { device: "/dev/dri/renderD128" },
    videotoolbox: NotApplicable { reason: "not macOS" },
    amf:          NotApplicable { reason: "not Windows" },
}
```

The transcoder reads the capability matrix at session creation and filters the profile chain to only available backends. `config.hw_acceleration` selects the user's preferred backend; `auto` picks the highest-priority available by platform (VideoToolbox on macOS, NVENC > VAAPI > QSV on Linux, AMF > NVENC > QSV on Windows).

### FFmpeg command shape

```
ffmpeg -i {input} \
  {seek_args}                          # -ss for mid-stream seek
  {hwa_device_args}                    # -init_hw_device ..., -hwaccel ..., -hwaccel_output_format ...
  -map 0:v:0 -map 0:a:{audio_index} \  # from decision engine
  {subtitle_map}                       # if burn-in
  -c:v {encoder} {encoder_args} \      # libx264 / h264_vaapi / h264_nvenc / h264_qsv / h264_videotoolbox / hevc_*
  -preset {preset} -crf {crf} \
  -profile:v {profile} -level {level} \
  -pix_fmt {pix_fmt} \
  -g {gop} -keyint_min {gop} -sc_threshold 0 \
  -force_key_frames "expr:gte(t,n_forced*{seg})" \
  {tonemap_filter}                     # if HDR → SDR
  {subtitle_filter}                    # if burn-in
  -c:a {audio_codec} {audio_args} \    # copy / ac3 / eac3 / aac + downmix
  -f hls -hls_time {seg} \
  -hls_playlist_type event -hls_list_size 0 \
  -hls_segment_type fmp4 \
  -hls_fmp4_init_filename init.mp4 \
  -hls_segment_filename '{dir}/segment_%05d.m4s' \
  -hls_flags independent_segments+temp_file+program_date_time \
  -progress pipe:2 \
  {dir}/playlist.m3u8
```

### Keyframe / segment alignment

- Segment length: 6 seconds (Apple HLS recommendation, cache-friendly, reasonable seek granularity)
- GOP: `segment_length × framerate` frames (144 at 24fps, 180 at 30fps, 360 at 60fps)
- `-sc_threshold 0` disables scene-cut keyframes that would split GOPs mid-segment
- fMP4/CMAF segments (required for HEVC and AV1; `hls_segment_type fmp4`)
- `independent_segments` flag — each segment starts on an IDR; seek-without-prior-segment is safe
- `temp_file` flag — segments written as `.m4s.tmp` and renamed on close; web server 404s on `.tmp` prevents half-written reads

`-force_key_frames` is honoured by software x264/x265; hardware encoders that ignore it (NVENC, QSV, VAAPI, AMF) still hit segment boundaries because `-g` and `-keyint_min` are pinned to the same value. Belt-and-braces alignment.

### Audio handling

Passthrough is always preferred when the client accepts the codec. `Ac3CopyProfile` and `Eac3CopyProfile` emit `-c:a copy` to preserve 5.1 / Atmos-in-EAC3; AAC transcode is the final rung.

Downmix 5.1 / 7.1 → stereo uses explicit coefficients, not FFmpeg defaults (which ignore LFE and make dialogue thin). Selectable algorithms:

| Algorithm | Basis | Notes |
|---|---|---|
| `Dave750` (default) | Cinema spec | Balanced downmix |
| `Ac4` | Dolby AC-4 rear mixing | Preserves Dolby intent |
| `Rfc7845` | Opus spec | Neutral |
| `NightmodeDialogue` | Centre-channel boost | Quiet listening |

Applied via a `pan=` filter with per-layout coefficient strings. `loudnorm=I=-16:TP=-1.5:LRA=11` (EBU R128) is applied post-downmix to prevent clipping and match web-audio loudness targets.

### Master playlist

The master playlist is assembled via a typed writer — never by hand-concatenated strings. `BANDWIDTH` is the measured output bitrate (not hardcoded); `CODECS` is computed from actual `-profile:v` / `-level` values (not fabricated). `VIDEO-RANGE` is emitted (`SDR` / `PQ` / `HLG`) matching the output transfer function. `SUPPLEMENTAL-CODECS` is emitted for DV-compatible fallback (e.g., `dvh1.05.06/db1p` for DV profile 5 with HDR10+ base) so DV-capable clients direct-play while HDR10 clients see the base.

## Session lifecycle

### Session structure

Sessions are keyed by `{kind}-{entity_id}-{tab_nonce}` and stored in an in-process `HashMap`. Each session carries:

- `child` — the FFmpeg process handle
- `profile_chain` — remaining fallback rungs
- `last_chunk_requested` — client highwater
- `current_chunk_produced` — producer highwater
- `state` — `Active` | `Paused` | `Dead`
- `hard_deadline` — reset on every segment request
- `active_requests` — ref count from in-flight HTTP responses
- `transcode_reasons` — for observability

### Ref-counted activity

Each playlist / segment request increments the session's `active_requests` counter; each `Response::OnCompleted` decrements. When the counter hits zero, an inactivity timer arms. Timeout: `2 × segment_length + 30s` (~42s for 6s segments). Any new request resets.

### Graceful shutdown

On eviction (inactivity, explicit `DELETE`, process shutdown):

1. Send `"q\n"` to FFmpeg stdin
2. Wait up to 5 seconds for `child.wait()` to return
3. If still running, SIGKILL via `child.start_kill()`

The `"q"` path lets FFmpeg flush the MOOV atom / init segment / last partial segment cleanly. SIGKILL mid-write leaves clients with decodable prefixes and undecodable tails. On Windows, graceful stop writes `"q"` to stdin with a 5s deadline, then `TerminateProcess`.

### Producer throttle

When `current_chunk_produced > last_chunk_requested + 15`, the session is considered "buffered ahead" and the FFmpeg process is paused via `kill(pid, SIGSTOP)` on Unix or `NtSuspendProcess` on Windows. When the client catches up (requests a new segment), `SIGCONT` / `NtResumeProcess` restarts production.

A paused FFmpeg consumes ~0 CPU and holds its decoder state. This is cheaper than tuning FFmpeg's internal queue depth and doesn't require an in-band protocol over stdin.

### Sliding-window segment cleanup

A 20-second background timer deletes segment files with index below `last_chunk_requested - keep_window` (default `keep_window = 20` segments = 2 minutes of playback). Bounds disk usage regardless of stream duration.

On session eviction the entire session directory is removed. On process boot, leftover session dirs from prior runs are cleaned up.

### Disconnect detection

No WebSocket or HTTP-keepalive ping. Disconnect is inferred from segment-request cadence: if the client has stopped requesting segments for `2 × segment_length + 30s`, the session is considered gone and evicted. Works reliably behind CDNs and flaky mobile networks that drop TCP silently.

## Error recovery

On FFmpeg non-zero exit:

1. Parse last 1KB of stderr to classify error (driver fault, OOM, missing codec, malformed input)
2. If error signature matches a known HWA failure and another profile rung exists, advance profile chain and restart from current segment
3. Otherwise surface as a transient segment error; client retries per its own backoff policy
4. Log structured `{session_id, prior_profile, reason, stderr_tail}`

Three strikes per rung — after three consecutive exits at the same rung, advance profile chain. After chain exhaustion, terminate session.

## Subtitle delivery

| Subtitle type | Delivery |
|---|---|
| External text (SRT, ASS, VTT, SSA) | Converted to WebVTT at import time; served as sidecar over HTTP `<track>` |
| Embedded text track | Extracted lazily to WebVTT on first request; cached under `{data_path}/cache/subs/{media_id}/{stream_idx}.vtt` |
| Embedded image (PGS, VOBSUB, DVB-SUB) | Burned into video via `-filter_complex "[0:v][0:s:{idx}]overlay=eof_action=pass:repeatlast=0[out]"` — forces a transcode |

Forced / hearing-impaired / commentary flags (from MKV `forced` / `hearing_impaired` disposition, or HLS `CHARACTERISTICS`) are surfaced as typed fields on the `SubtitleTrack` so the UI can render badges. Forced subtitles in the user's selected audio language auto-enable on audio switch.

Burn-in uses HW-accelerated overlay variants (`overlay_cuda`, `overlay_vaapi`, `overlay_qsv`) when the active transcode profile is HW-based; software `overlay` otherwise. `eof_action=pass` prevents hangs when the subtitle stream EOFs before video.

### Subtitle endpoints

```
GET /api/v1/play/{kind}/{entity_id}/subtitles/{stream_index}
```

Returns `text/vtt` for text subtitles. Image-based subtitles are handled inline in the video transcode and have no separate endpoint.

## Trickplay

WebVTT thumbnail track + JPEG sprite sheet generated at import time. 160-wide thumbs at 10-second intervals, 10×10 tiles per sheet. Cached VTT is served with ETag once `trickplay_generated = 1`; sprites with `max-age=3600`.

Generation: `fps=1/10,scale=160:-1,tile=10x10` FFmpeg filter. Streaming sources (torrents still downloading) get partial trickplay via `-t duration -ss 0` bounds, overwritten in place as more content becomes available; completion triggers a full regenerate.

See subsystem 12 for details.

## Watch progress tracking

### Client reporting

```
POST /api/v1/play/{kind}/{entity_id}/progress
{ "position_secs": 1234.5, "paused": false, "final_tick": false, "incognito": false }
```

- `paused` — distinguishes play state from pause state; maps to Trakt `scrobble/start` vs `scrobble/pause`
- `final_tick` — tab close / unmount; maps to Trakt `scrobble/pause` and releases server-side session resources
- `incognito` — bypasses Trakt scrobble entirely (still updates local resume position)

Cadence: every 10 seconds during active playback. The server enforces no cadence lower bound (client-driven).

### Watched threshold

| Position | Action |
|---|---|
| < 5% of runtime | Reset position to 0 |
| 5–80% of runtime | Save position for resume |
| ≥ 80% of runtime | Mark watched: set `watched_at`, increment `play_count`, reset position |

80% aligns with Trakt's completion threshold. On next play, if `playback_position_ticks > 0`, surface a resume dialog.

## Client integration

HLS over fMP4 segments served at:

```
GET    /api/v1/play/{kind}/{entity_id}/prepare                 # PlayPrepareReply with all metadata
GET    /api/v1/play/{kind}/{entity_id}/master.m3u8             # triggers session
GET    /api/v1/play/{kind}/{entity_id}/variant.m3u8            # variant playlist
GET    /api/v1/play/{kind}/{entity_id}/segments/{n}            # individual segment
GET    /api/v1/play/{kind}/{entity_id}/subtitles/{stream}      # WebVTT
GET    /api/v1/play/{kind}/{entity_id}/trickplay.vtt           # thumbnail cues
GET    /api/v1/play/{kind}/{entity_id}/direct                  # byte-range direct play
DELETE /api/v1/play/{kind}/{entity_id}/transcode               # end session
POST   /api/v1/play/{kind}/{entity_id}/progress                # watch progress
```

`PlayPrepareReply` carries: state, resume position, direct-play eligibility, `transcode_reasons` bitmask, `streams` (typed `AudioTrack[]` + `SubtitleTrack[]`), intro/credits timestamps, trickplay URL. The frontend uses this one response to configure pickers, skip buttons, and error recovery — no separate stream-list fetch.

### Stream types

```
AudioTrack {
    stream_index: i64,
    codec: String,
    language: Option<String>,
    channel_layout: String,       // "stereo", "5.1", "7.1", ...
    channels: i64,
    default: bool,
    label: String,                // display-ready: "English · 5.1 · AC-3"
    roles: Vec<String>,           // "main" | "commentary" | "dub"
}

SubtitleTrack {
    stream_index: i64,
    codec: String,                // "webvtt" | "pgs" | ...
    language: Option<String>,
    forced: bool,
    hearing_impaired: bool,
    default: bool,
    label: String,
    roles: Vec<String>,           // "subtitle" | "caption" | "commentary"
    vtt_url: Option<String>,      // None for image subs (burned in)
}
```

### Error recovery

Client maps hls.js errors to a structured `{severity, category, code, data, handled}` shape. Retry policy:

- Segment-level: exponential backoff with ±50% jitter, 3 attempts before escalation
- Stream-level: one `hls.recoverMediaError()` attempt before surfacing to UI
- Fatal: UI shows error card with copy-to-clipboard diagnostic bundle

## Chromecast integration

Kino runs a Custom Web Receiver registered with the Google Cast Developer Console. Sender (browser) → receiver message includes:

- Signed playback URL (HMAC-keyed, per-session token, expires_at)
- Subtitle URL + metadata
- Current progress + audio/subtitle track selections
- Movie/episode metadata for receiver UI

**On Cast connect**: the local player pauses and calls `stopHls()`; sender issues `LOAD` with current position + selected tracks.

**On Cast disconnect**: sender pulls position + track state back from receiver and resumes local playback at the remote position. Tracks are matched by `(language, roles, forced)` tuple, not numeric id — ids differ between sessions.

The receiver uses the default CAF media player with a custom HTML/CSS branded UI. Codec support is determined by the receiver hardware (1st-gen Chromecast vs Ultra vs Google TV Streamer); kino's decision engine takes client-advertised capabilities into account when selecting the profile chain.

## Entities touched

- **Reads:** Media (file path, runtime), Stream (codec/format info, color metadata, channel layout, disposition flags), Config (hw_acceleration, ffmpeg_path, transcoding_enabled, audio_downmix_algorithm, tonemap_algorithm)
- **Writes:** Movie/Episode (playback_position_ticks, play_count, last_played_at, watched_at, status → `watched`)
- **Triggers:** History (watched event), Notification (watched event), Cleanup (via watched status)

## Dependencies

- FFmpeg (external binary; auto-downloaded on first run if not present in `PATH` or `config.ffmpeg_path`)
- Config table (transcode settings, HW acceleration, downmix/tonemap preferences)
- Filesystem (media files, session temp directories)
- Notification subsystem (watched events)

## Error states

- **File not found** → 404; media may have been cleaned up
- **FFmpeg not found** → transcode disabled, direct play only, health warning logged
- **FFmpeg HW backend fails at runtime** → profile chain advances, restart at current segment
- **All profile chain rungs exhausted** → session fatal, client error card
- **Disk full for session temp** → transcode fails with dedicated error code; cleanup sweep frees space
- **Client disconnects (inferred)** → inactivity timer → graceful shutdown → session dir purged
