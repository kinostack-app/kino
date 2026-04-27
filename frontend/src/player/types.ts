/**
 * Player type surface.
 *
 * Single source of truth for anything that crosses the
 * player boundary — consumed by `PlayerRoot` (owner) and
 * by every hook + chrome component beneath. Hand-rolled
 * DTO mirrors are prohibited: track/subtitle shapes come
 * from the OpenAPI-generated types so the wire format
 * drives the UI.
 */

import type { AudioTrack, SubtitleTrack } from '@/api/generated/types.gen';

/**
 * Everything the shell needs to know about "what to play."
 * Resolved once per source change by the parent and fed in.
 * The shell doesn't care whether the source came from the
 * library (transcoded HLS with probed metadata) or a live
 * torrent (direct byte-range with incremental stream probe).
 */
export interface VideoSource {
  /** Direct file URL (byte-range) OR HLS master playlist URL. */
  url: string;
  /** `hls` → build an Hls.js session; `direct` → set
   *  `<video>.src` and let the browser handle it. */
  mode: 'direct' | 'hls';
  /** Title for the top bar. */
  title: string;
  /** When set, switching to cast will load this media id on
   *  the receiver. Cast only runs against library media
   *  today; torrent sources leave this undefined. */
  castMediaId?: number;
  /** Optional WebVTT URL for trickplay hover previews. The
   *  VTT's cues must use the `sprite.jpg#xywh=x,y,w,h`
   *  fragment format — Media Chrome's preview thumbnail
   *  parses that shape natively. */
  trickplayUrl?: string | null;
  /** Incrementing counter that forces the trickplay VTT to
   *  re-fetch. Torrent sources wire this to
   *  `trickplay_stream_updated` WS events so coverage grows
   *  in the UI as sprites land. */
  trickplayRefreshSignal?: number;
  /** Total runtime in seconds when known. Lets the hover
   *  tooltip distinguish past-covered from past-end-of-file. */
  totalDurationSec?: number;
  /** Optional subtitle tracks; empty array hides the picker. */
  subtitles?: SubtitleTrack[];
  /** Optional audio tracks (library sources that have probed
   *  stream metadata). Switching tracks forces HLS transcode
   *  restart so the server can select the requested stream. */
  audioTracks?: AudioTrack[];
  /** Currently selected audio stream index. */
  audioStreamIndex?: number;
  /** Called when the user picks a new audio track. Parent
   *  resolves a new `VideoSource.url` (typically with
   *  `?audio_stream=N`) and feeds it back on the next render. */
  onAudioStreamChange?: (streamIndex: number) => void;
  /** Called when the user picks an image-based subtitle
   *  (burn-in) or clears one. Fires with `null` on "Off" or a
   *  text-sub selection — text subs render client-side via
   *  `<track>`, but the parent must still clear any prior
   *  burn-in. Image subs require an ffmpeg restart with the
   *  overlay filter chain. */
  onBurnInSubtitleChange?: (streamIndex: number | null) => void;
  /** Called when direct play fails with `MediaError::SRC_NOT_SUPPORTED`
   *  and the source has an HLS endpoint to fall back to (library media
   *  with a `castMediaId`). Parent must rebuild the source URL with
   *  `/master.m3u8` and flip `mode` to `'hls'` together — flipping mode
   *  alone leaves hls.js trying to load the byte-range `/direct` URL,
   *  which it can't parse. */
  onForceHls?: () => void;
  /** Seconds to auto-seek to once metadata arrives. Used by
   *  the handoff from torrent→library and by the Resume
   *  prompt. */
  resumeAtSec?: number;
  /** Known runtime in seconds from a source other than
   *  `<video>.duration`. Range-requested partial MKVs report
   *  only the buffered portion — `expectedDurationSec` is the
   *  authoritative value in that case. */
  expectedDurationSec?: number;
  /** Intro/credits skip markers in milliseconds. All four are
   *  optional; the Skip button renders when the playhead
   *  enters an intro range, and when it passes credits_start.
   *  `skipEnabledForShow` gates intro rendering per-show. */
  introStartMs?: number | null;
  introEndMs?: number | null;
  creditsStartMs?: number | null;
  creditsEndMs?: number | null;
  skipEnabledForShow?: boolean;
  /** Identity for the per-session "intro already watched" map. Only
   *  populated for episodes — movies get `null`. */
  showId?: number | null;
  seasonNumber?: number | null;
  /** True when the user has watched (`play_count > 0`) at least one
   *  episode in this season. Lets smart-mode auto-skip fire from the
   *  very first episode of the session when the user has seen the
   *  show before. */
  seasonAnyWatched?: boolean;
  /** Raw `auto_skip_intros` config (`"off"` / `"on"` / `"smart"`). */
  autoSkipIntros?: string;
  /** HLS transcode `-ss` offset in seconds. The scrubber spans
   *  the full runtime, but the `<video>`'s `currentTime` is
   *  relative to the transcode start — conversion happens at
   *  the element boundary. Zero/undefined for direct sources
   *  and fresh HLS starts. */
  hlsSourceOffsetSec?: number;
  /** Called when the user seeks to a source-time that can't
   *  be satisfied by the currently-running transcode (past
   *  the playlist's produced end, or backwards of the current
   *  offset). Parent rebuilds `source.url` with a new
   *  `?at=<secs>` and bumps `hlsSourceOffsetSec`. When
   *  omitted, the shell falls back to native seek and accepts
   *  that the browser may snap back. */
  onSeekReload?: (sourceSec: number) => void;
}

/**
 * Imperative handle exposed via `handleRef` — lets the parent
 * pause/resume the underlying `<video>` without owning it
 * directly. Used by PlaybackInfoChip to pause while the user
 * reads playback specs; kept deliberately narrow to avoid the
 * "full player remote" footgun.
 */
export interface VideoShellHandle {
  pause: () => void;
  play: () => void;
  isPaused: () => boolean;
}

/**
 * Identity + progress card rendered in the center of the
 * screen until the first `playing` event. Fades to
 * transparent after. Absent → the player falls back to the
 * built-in loading indicator.
 */
export interface LoadingOverlay {
  title: string;
  logo?: {
    contentType: 'movies' | 'shows';
    entityId: number;
    palette: string | null | undefined;
  } | null;
  /** One-line activity description: "Finding a release", etc. */
  status: string;
  /** Ordered stage labels for the step indicator. */
  stages?: string[];
  /** Current stage (0-based index into `stages`). */
  currentStage?: number;
  /** Monotonic 0–100 progress; drives the logo sweep. */
  progress: number;
  error?: {
    message: string;
    retry?: () => void;
    action?: { label: string; onClick: () => void };
  };
}

/**
 * Mid-stream recovery overlay. Rendered only when playback
 * has actually stalled (video element `waiting` after first
 * play) AND the caller has set a stall reason — so the shell
 * knows WHY the data stopped and can offer a fix.
 */
export interface StallOverlay {
  title: string;
  logo?: LoadingOverlay['logo'];
  message: string;
  action: { label: string; onClick: () => void };
}

/** Shell props — preserved wholesale from the pre-migration
 *  surface so PlayerRoot's call site needs no changes. */
export interface VideoShellProps {
  source: VideoSource | null;
  handleRef?: React.RefObject<VideoShellHandle | null>;
  /** Overlay rendered above the video before controls
   *  (e.g. a download-progress badge). Fades with the
   *  main chrome. */
  topOverlay?: React.ReactNode;
  /** Extra layer drawn inside the seek bar — used by the
   *  torrent wrapper for its download-percent stripe,
   *  sitting beneath the native buffered range so the
   *  latter remains legible on top. */
  seekExtraLayer?: React.ReactNode;
  /** Back-button handler. Defaults to history.back() + `/`
   *  fallback when history is empty. */
  onBack?: () => void;
  /** Called on every `timeupdate` with the current playhead
   *  in seconds. */
  onPlaybackTime?: (seconds: number) => void;
  /** Fires once per source on the first `playing` event. */
  onFirstPlay?: () => void;
  /** Fires whenever the element transitions between playing
   *  and paused. Drives Trakt scrobble start vs. pause. */
  onPlayStateChange?: (paused: boolean) => void;
  loadingOverlay?: LoadingOverlay;
  stallOverlay?: StallOverlay;
}

export const SPEED_OPTIONS = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2] as const;
export type Speed = (typeof SPEED_OPTIONS)[number];

export function stepSpeed(current: number, delta: 1 | -1): Speed {
  const idx = SPEED_OPTIONS.indexOf(current as Speed);
  if (idx === -1) return 1;
  const next = Math.max(0, Math.min(SPEED_OPTIONS.length - 1, idx + delta));
  return SPEED_OPTIONS[next];
}

export function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds)) return '--:--';
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (n: number) => n.toString().padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
}
