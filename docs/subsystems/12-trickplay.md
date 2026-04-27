# Trickplay thumbnails

Seek bar preview thumbnails — the small images that appear when hovering over the video player's progress bar. Generated after import (or incrementally during a torrent stream), served as a WebVTT thumbnail track with `#xywh` fragment URIs that point at sprite-sheet JPEGs.

## Why WebVTT and not HLS image playlists

Apple's HLS spec defines `#EXT-X-IMAGES-ONLY` + `#EXT-X-TILES` for in-band thumbnail tracks. It exists. It is not ubiquitous.

We chose the **WebVTT `xywh` fragment approach** instead:

- Every browser that supports `<track kind="metadata">` can read it — no playback-library dependency
- Universally supported across mainstream web video UI libraries (Media Chrome, Vidstack, Plyr, Shaka's legacy path)
- Decouples trickplay from HLS entirely — direct-play sources get the same hover tooltip
- Doesn't require the master playlist to carry an image-track declaration, which some strict validators reject
- Maps cleanly onto our own hand-rolled scrubber, which reads cues directly via `track.cues`

The HLS image-playlist path remains a potential future addition for Apple-native HLS clients. It's out of scope for day one.

## How it works

1. FFmpeg extracts one JPEG frame every 10 seconds from the video
2. Those frames composite via the `tile` filter into 10×10 sprite sheets (100 thumbnails per sheet) — done inside the same FFmpeg invocation, no separate image-library compositing step
3. A WebVTT file is written alongside the sheets, with one cue per thumbnail whose cue text is `sprite_NNN.jpg#xywh=x,y,w,h`
4. The player attaches the VTT as a `<track kind="metadata" label="thumbnails">` and the scrubber renders the cue's sprite sub-rectangle on hover

## Generation

### When

Two trigger paths:

- **Post-import** — a `trickplay_post_import_listener` consumes `Imported` / `Upgraded` events and fires `TaskTrigger::fire("trickplay_generation")` via the scheduler's trigger channel. Near-immediate latency for fresh media.
- **Scheduled sweep** — the 5-minute `trickplay_generation` scheduler task picks up any `trickplay_generated = 0` rows as backstop, including media imported before the listener existed.
- **Streaming** — for torrent streams that are playable before import, a companion `trickplay_stream` task generates partial sheets incrementally as the file fills in. Sealed sheets are written once; the currently-growing sheet is regenerated each tick until the file is sealed. On import, the streaming sheets promote in place into the library directory.

A process-wide mutex + `PER_SWEEP = 1` guards against concurrent FFmpeg against the same media. Trickplay is deferred when the transcode manager reports any active session — a user watching a video is prioritised over background sprite generation.

### FFmpeg command

```
ffmpeg -hide_banner -loglevel error -y \
  -threads 2 \
  -i {input_file} \
  -vf "{tonemap?,}fps=1/10,scale=320:-1,tile=10x10" \
  -vsync vfr -qscale:v 5 \
  {output_dir}/sprite_%03d.jpg
```

- `fps=1/10` — one frame every 10 seconds
- `scale=320:-1` — 320px wide (up from the earlier 160px default; renders cleanly in the resume-dialog tile at ~440px displayed width)
- `tile=10x10` — composite 100 thumbnails per sheet in one pass
- `qscale:v 5` — good-quality JPEG
- `-threads 2` — caps CPU at 2 cores; background work shouldn't saturate the host

A 2-hour movie produces ~720 thumbnails = ~8 sprite sheets.

### HDR tone-mapping

HDR sources (PQ / HLG / Dolby Vision) produce washed-out or purple thumbnails without colour-space conversion. `trickplay::generate` probes the source once and prepends a tone-map filter when the primary video stream's `color_transfer` is `smpte2084` (HDR10) or `arib-std-b67` (HLG).

Filter choice follows the transcoder's `use_libplacebo` pattern:

- **`TONEMAP_LIBPLACEBO`** — one-stage GPU-accelerated tone-map (`libplacebo=tonemapping=hable:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv:format=yuv420p`) when the active ffmpeg build carries libplacebo (jellyfin-ffmpeg does; vanilla ffmpeg needs `--enable-libplacebo`).
- **`TONEMAP_HABLE_SW`** — CPU `zscale` + `tonemap` + `zscale` + `format=yuv420p` chain as the fallback. Correct but ~10× slower on 4K HDR.

The **streaming trickplay** path (incremental sheets during a torrent download) deliberately skips the probe + tonemap. ffprobe is unreliable against partial files, and the post-import `generate` pass replaces the streaming sheets with correct-colour ones within a sprite sheet's write cycle anyway — washed-out thumbnails for the few minutes of live streaming is an acceptable trade.

### Sprite sheet storage

```
{data_path}/trickplay/{media_id}/
  trickplay.vtt
  sprite_001.jpg      ← first 100 thumbnails composited into a 10×10 grid
  sprite_002.jpg      ← next 100
  …
```

Stored under `data_path`, not in the media library — files are disposable and regenerated on demand if missing. Cleaned up when media is deleted (`playback::trickplay::clear_trickplay_dir` is called from both `api::media::delete_media` and `cleanup::delete_media_file`).

Streaming trickplay lives under `{data_path}/trickplay-stream/{download_id}/` and promotes to the library location on import.

### Storage cost

~5-15 KB per thumbnail at 320px. A 2-hour movie: ~4-10 MB total. Negligible relative to the video file.

### Failure handling

`TrickplayError::is_permanent()` classifies failures:

- **Permanent** — `TooShort(secs)` (file shorter than two intervals). Marks `trickplay_generated = 1` immediately; no retry would ever succeed.
- **Transient** — `Probe` / `Ffmpeg` / `Io`. Bumps a `trickplay_attempts INTEGER` counter on the media row, rolls `trickplay_generated` back to 0, and the next sweep tries again. After `MAX_ATTEMPTS = 3` we give up and mark done so a genuinely-broken file doesn't loop forever. Successful generation resets the counter so a later force-regenerate starts fresh.

## API

### Thumbnail VTT

```
GET /api/v1/play/{kind}/{entity_id}/trickplay.vtt
```

Returns a WebVTT thumbnail track. Library sources resolve against the library directory; streaming sources against the live stream-trickplay directory. During the stream → library transition the endpoint naturally starts returning the library VTT once sheets promote — WS-driven cache invalidation (`trickplay_stream_updated` event) refreshes the frontend.

Response shape:

```vtt
WEBVTT

00:00:00.000 --> 00:00:10.000
sprite_001.jpg#xywh=0,0,320,180

00:00:10.000 --> 00:00:20.000
sprite_001.jpg#xywh=320,0,320,180

…
```

The endpoint rewrites the relative sprite paths in-flight to full API URLs so the browser fetches `sprite_NNN.jpg` from the segments endpoint below, not from a relative path the `<track>` element has no context for. Cache headers vary by state — `no-store, must-revalidate` while streaming (sheets still growing), `public, max-age=86400` with an ETag once the generated flag flips to 1.

### Sprite sheet

```
GET /api/v1/play/{kind}/{entity_id}/trickplay/sprite_{index}.jpg
```

Serves the sheet JPEG. `Cache-Control: public, max-age=3600` during streaming; `max-age=86400` with ETag once the library VTT is ready.

## Player integration

On the video element:

```html
<video>
  <track kind="metadata" label="thumbnails" default
         src="/api/v1/play/movie/42/trickplay.vtt?api_key=…" />
</video>
```

On seek-bar hover, the scrubber component reads the matching cue via `track.cues`, parses the `#xywh=x,y,w,h` fragment from `cue.text`, and renders the sprite sub-rectangle as a hover tooltip. No coordinate math in the backend — the VTT embeds everything needed.

The browser fetches sprite sheets lazily — only when the user hovers into that time range — because it only renders sprites for cues whose `#xywh` URL it actually reads.

Three hover states surface beyond the happy path:

- **Cue exists** — real thumbnail + timestamp
- **No cues yet** — skeleton + spinner (streaming source, first sprite sheet hasn't landed)
- **Past the covered range** — timestamp + "Generating…" label (streaming source, later part of file not sheeted yet)

## Database

- `media.trickplay_generated INTEGER` — 0 = pending, 1 = done, 2 = claimed by library sweep, 3 = claimed by stream-task post-import. `= 1` is the authoritative "serveable" state; the intermediate claim values are private to the background tasks.
- `media.trickplay_attempts INTEGER` — bumped on each transient failure, reset on success. Gates permanent-retry-loop protection (see above).

## Entities touched

- **Reads:** Media (file path, id)
- **Writes:** Media (`trickplay_generated`, `trickplay_attempts`)
- **Writes to filesystem:** sprite-sheet JPEGs + `trickplay.vtt` in `{data_path}/trickplay/{media_id}/` or `{data_path}/trickplay-stream/{download_id}/`
