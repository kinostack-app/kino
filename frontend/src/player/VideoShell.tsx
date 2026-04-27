/**
 * VideoShell — the single video UI.
 *
 * Consumed by both the library player (`/play/{mediaId}`)
 * and the torrent streaming player (`/watch/{downloadId}`).
 * Route components collapse to thin wrappers that resolve
 * a `VideoSource` from their params and hand it to this
 * shell.
 *
 * Architecture: a composer over purpose-built modules:
 *
 *   chrome/   — hand-rolled React controls (scrubber with
 *               trickplay tooltip, speed/audio/sub/cast
 *               pickers, control bar, top bar, shortcuts
 *               dialog)
 *   hls/      — hls.js session manager with two-tier retry
 *   source/   — source binding + seek funnel
 *   state/    — persisted prefs, wake lock, mediaSession,
 *               auto-forced subs, cast handoff, autohide,
 *               fullscreen, PiP, keyboard shortcuts
 *   overlays/ — loading, stall, cast, media-error
 *
 * We own every control. Zero third-party chrome library
 * — so every behavior, down to the cursor-hide timing, is
 * tunable. The file split keeps the composer under 500
 * lines of readable orchestration.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { cn } from '@/lib/utils';
import { ControlBar } from './chrome/ControlBar';
import { ShortcutsDialog } from './chrome/ShortcutsDialog';
import { SkipButton } from './chrome/SkipButton';
import { TopBar } from './chrome/TopBar';
import { useHlsSession } from './hls/useHlsSession';
import { useAutoSkipIntro } from './hooks/useAutoSkipIntro';
import { CastOverlay } from './overlays/CastOverlay';
import { LoadingOverlayView } from './overlays/LoadingOverlay';
import { MediaErrorOverlay } from './overlays/MediaErrorOverlay';
import { StallOverlayView } from './overlays/StallOverlay';
import { useSeekFunnel } from './source/useSeekFunnel';
import { useResumeSeekRef, useSourceBinding } from './source/useSourceBinding';
import { useAutoForcedSubtitles } from './state/useAutoForcedSubtitles';
import { useAutoHide } from './state/useAutoHide';
import { useCastHandoff } from './state/useCastHandoff';
import { useFullscreen } from './state/useFullscreen';
import { useKeyboardShortcuts } from './state/useKeyboardShortcuts';
import { useMediaSessionMetadata } from './state/useMediaSessionMetadata';
import { PREFERENCE_DEFAULTS, usePersistedPreferences } from './state/usePersistedPreferences';
import { usePictureInPicture } from './state/usePictureInPicture';
import { useWakeLock } from './state/useWakeLock';
import { stepSpeed, type VideoShellProps } from './types';

export function VideoShell({
  source,
  handleRef,
  topOverlay,
  seekExtraLayer,
  onBack,
  onPlaybackTime,
  onFirstPlay,
  onPlayStateChange,
  loadingOverlay,
  stallOverlay,
}: VideoShellProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  // The fullscreen target is our *inner* wrapper — a plain
  // block-layout div filling an outer `fixed inset-0`
  // modal layer. Fullscreening a plain block avoids the
  // Firefox/Linux edge-bars artefact that appears when
  // you fullscreen an element that's already
  // `position: fixed`; the UA `:fullscreen` defaults
  // conflict with our layout and leave a few pixels of
  // gutter on every side.
  const containerRef = useRef<HTMLDivElement>(null);

  // ── Player state ──────────────────────────────────────
  const [playing, setPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [nativeDuration, setNativeDuration] = useState(0);
  const [buffered, setBuffered] = useState(0);
  const [volume, setVolume] = useState(PREFERENCE_DEFAULTS.volume);
  // Muted is intentionally session-only — autoplay policies
  // can force mute on first load; persisting that silently
  // silences every subsequent session.
  const [muted, setMuted] = useState(false);
  const [playbackRate, setPlaybackRate] = useState(PREFERENCE_DEFAULTS.playbackRate);
  const [subtitleStreamIndex, setSubtitleStreamIndex] = useState<number | null>(null);
  const [overlayFading, setOverlayFading] = useState(false);
  const [overlayDismissed, setOverlayDismissed] = useState(false);
  const [helpOpen, setHelpOpen] = useState(false);
  const firstPlayFiredRef = useRef(false);

  // Duration reconciliation. Range-requested MKVs report
  // duration against the buffered byte range, not the full
  // file. When expected and native differ by >10%, trust
  // expected — otherwise the scrubber visibly "grows" as
  // more of the file loads, which feels broken.
  const duration = (() => {
    const expected = source?.expectedDurationSec;
    if (!expected || expected <= 0) return nativeDuration;
    if (!Number.isFinite(nativeDuration) || nativeDuration <= 0) return expected;
    const ratio = nativeDuration / expected;
    return ratio < 0.9 ? expected : nativeDuration;
  })();

  // HLS source-offset ref — lets callbacks that read
  // `video.currentTime` convert to source-time without
  // threading `source` through dep arrays.
  const hlsOffsetRef = useRef(0);
  hlsOffsetRef.current = source?.hlsSourceOffsetSec ?? 0;

  // ── Subsystem hooks ───────────────────────────────────
  const isStalledRef = useRef(false);
  isStalledRef.current = Boolean(stallOverlay);

  const [fatalError, setFatalError] = useState<string | null>(null);

  const hls = useHlsSession({
    videoRef,
    isStalledRef,
    onFatal: setFatalError,
  });

  // When a stall clears, poke hls.js to re-request
  // fragments so playback resumes where it left off.
  useEffect(() => {
    if (!stallOverlay) hls.poke();
  }, [stallOverlay, hls]);

  const resumeSeekAppliedRef = useResumeSeekRef();
  const sourceBinding = useSourceBinding(videoRef, source, hls, resumeSeekAppliedRef);
  const mediaError = sourceBinding.mediaError ?? fatalError;

  const seekToSource = useSeekFunnel({
    videoRef,
    source,
    setIsLoading: sourceBinding.setIsLoading,
    setHasPlayedOnce: sourceBinding.setHasPlayedOnce,
    onSourceTimeChange: setCurrentTime,
  });

  usePersistedPreferences(videoRef, source?.url, volume, muted, playbackRate);
  useWakeLock(playing);
  useMediaSessionMetadata(videoRef, source, playing);
  useAutoSkipIntro({ currentTime, source, onSeek: seekToSource });

  const userHasPickedSubtitleRef = useAutoForcedSubtitles(source, setSubtitleStreamIndex);

  const castHandoff = useCastHandoff(videoRef, source, hls.stop);

  const fullscreen = useFullscreen(containerRef);
  const pip = usePictureInPicture(videoRef);
  const autoHide = useAutoHide(containerRef, videoRef);

  // ── Imperative handle ────────────────────────────────
  useEffect(() => {
    if (!handleRef) return;
    handleRef.current = {
      pause: () => videoRef.current?.pause(),
      play: () => {
        videoRef.current?.play().catch(() => {
          // Autoplay-gesture block / pending source swap —
          // nothing to recover.
        });
      },
      isPaused: () => videoRef.current?.paused ?? true,
    };
    return () => {
      handleRef.current = null;
    };
  }, [handleRef]);

  // ── Subtitle <track> mode sync ────────────────────────
  // biome-ignore lint/correctness/useExhaustiveDependencies: source?.subtitles is a re-apply trigger, not a dep we read.
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    const tracks = video.textTracks;
    for (let i = 0; i < tracks.length; i++) {
      const t = tracks[i];
      const match = subtitleStreamIndex != null && t.label === `sub-${subtitleStreamIndex}`;
      t.mode = match ? 'showing' : 'disabled';
    }
  }, [subtitleStreamIndex, source?.subtitles]);

  // ── Loading overlay fade ──────────────────────────────
  useEffect(() => {
    if (!sourceBinding.hasPlayedOnce) return;
    setOverlayFading(true);
    const t = setTimeout(() => setOverlayDismissed(true), 500);
    return () => clearTimeout(t);
  }, [sourceBinding.hasPlayedOnce]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: URL is the intentional trigger.
  useEffect(() => {
    setOverlayFading(false);
    setOverlayDismissed(false);
  }, [source?.url]);

  // ── Navigation ────────────────────────────────────────
  const goBack = useCallback(() => {
    if (onBack) return onBack();
    if (window.history.length > 1) window.history.back();
    else window.location.href = '/';
  }, [onBack]);

  // ── Keyboard shortcuts ───────────────────────────────
  useKeyboardShortcuts(
    useCallback(
      (key) => {
        const video = videoRef.current;
        if (!video) return;
        autoHide.show();
        switch (key) {
          case 'Space':
          case 'k':
            if (video.paused) void video.play();
            else video.pause();
            return true;
          case 'ArrowLeft':
            seekToSource(Math.max(0, currentTime - 10));
            return true;
          case 'ArrowRight':
            seekToSource(Math.min(duration || 0, currentTime + 10));
            return true;
          case 'ArrowUp':
            video.volume = Math.min(1, video.volume + 0.1);
            setVolume(video.volume);
            return true;
          case 'ArrowDown':
            video.volume = Math.max(0, video.volume - 0.1);
            setVolume(video.volume);
            return true;
          case 'f':
            fullscreen.toggle();
            return true;
          case 'm':
            video.muted = !video.muted;
            setMuted(video.muted);
            return true;
          case 'c':
            // TODO: open subtitle picker — currently the
            // picker owns its own open state, no external
            // trigger. Stays a no-op until we lift it.
            return false;
          case 'p':
            void pip.toggle();
            return true;
          case '>':
          case '.':
            setPlaybackRate((r) => stepSpeed(r, +1));
            return true;
          case '<':
          case ',':
            setPlaybackRate((r) => stepSpeed(r, -1));
            return true;
          case '?':
            setHelpOpen((o) => !o);
            return true;
          case 'Escape':
            if (helpOpen) {
              setHelpOpen(false);
              return true;
            }
            if (fullscreen.isFullscreen) {
              void document.exitFullscreen().catch(() => {});
              return true;
            }
            goBack();
            return true;
          default:
            return false;
        }
      },
      [autoHide, currentTime, duration, fullscreen, goBack, helpOpen, pip, seekToSource]
    )
  );

  // ── Handlers wired to child components ───────────────
  const onMuteToggle = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    v.muted = !v.muted;
    setMuted(v.muted);
  }, []);

  const onVolumeChange = useCallback((v: number) => {
    const video = videoRef.current;
    if (!video) return;
    video.volume = v;
    video.muted = false;
    setVolume(v);
    setMuted(false);
  }, []);

  /** Pause the video and report whether it was playing
   *  before — the caller uses the boolean to decide
   *  whether to resume on dialog close. Used by pickers
   *  that want the user's attention on a choice without
   *  dialogue playing underneath. */
  const onRequestPause = useCallback(() => {
    const v = videoRef.current;
    if (!v) return false;
    const wasPlaying = !v.paused;
    if (wasPlaying) v.pause();
    return wasPlaying;
  }, []);

  const onRequestResume = useCallback(() => {
    videoRef.current?.play().catch(() => {
      // Autoplay-gesture block — nothing to recover.
    });
  }, []);

  // ── Render ────────────────────────────────────────────
  return (
    <div className="fixed inset-0 bg-black z-50 select-none">
      <div
        ref={containerRef}
        role="application"
        tabIndex={-1}
        data-player-root
        className="relative w-full h-full bg-black"
        onClick={(e) => {
          // Click on the video itself (or on this
          // container in a non-hit-area region) toggles
          // play/pause.
          if (e.target === e.currentTarget || (e.target as HTMLElement).tagName === 'VIDEO') {
            const v = videoRef.current;
            if (!v) return;
            if (v.paused) void v.play();
            else v.pause();
          }
        }}
        onKeyDown={() => {
          // Presence of this handler keeps the a11y
          // linter happy; real shortcuts come through
          // the window listener.
        }}
      >
        <video
          ref={videoRef}
          crossOrigin="anonymous"
          playsInline
          className="absolute inset-0 w-full h-full object-contain"
          onWaiting={() => sourceBinding.setIsLoading(true)}
          onCanPlay={() => sourceBinding.setIsLoading(false)}
          onStalled={() => sourceBinding.setIsLoading(true)}
          onPlay={() => {
            setPlaying(true);
            autoHide.show();
            onPlayStateChange?.(false);
          }}
          onPlaying={() => {
            sourceBinding.setIsLoading(false);
            if (!firstPlayFiredRef.current) {
              firstPlayFiredRef.current = true;
              sourceBinding.setHasPlayedOnce(true);
              onFirstPlay?.();
            }
          }}
          onPause={() => {
            setPlaying(false);
            autoHide.show();
            onPlayStateChange?.(true);
          }}
          onSeeking={() => sourceBinding.setIsLoading(true)}
          onTimeUpdate={() => {
            const offset = source?.hlsSourceOffsetSec ?? 0;
            const t = (videoRef.current?.currentTime ?? 0) + offset;
            setCurrentTime(t);
            onPlaybackTime?.(t);
          }}
          onDurationChange={() => {
            const v = videoRef.current;
            if (!v) return;
            setNativeDuration(v.duration ?? 0);
            // Apply resume-at seek once per source.
            const resume = source?.resumeAtSec;
            if (
              resume != null &&
              resume > 0 &&
              !resumeSeekAppliedRef.current &&
              Number.isFinite(v.duration) &&
              v.duration > 0
            ) {
              resumeSeekAppliedRef.current = true;
              const offset = source?.hlsSourceOffsetSec ?? 0;
              v.currentTime = Math.min(Math.max(0, resume - offset), v.duration - 1);
              void v.play().catch(() => {});
            }
          }}
          onProgress={() => {
            const v = videoRef.current;
            if (v && v.buffered.length > 0) {
              const offset = source?.hlsSourceOffsetSec ?? 0;
              setBuffered(v.buffered.end(v.buffered.length - 1) + offset);
            }
          }}
          onVolumeChange={() => {
            const v = videoRef.current;
            if (!v) return;
            setVolume(v.volume);
            setMuted(v.muted);
          }}
          onRateChange={() => {
            const v = videoRef.current;
            if (!v) return;
            setPlaybackRate(v.playbackRate);
          }}
          onError={() => {
            sourceBinding.handleVideoError(videoRef.current?.error?.code);
          }}
          onEnded={() => {
            setPlaying(false);
            autoHide.show();
          }}
        >
          {/* Placeholder captions track — keeps a11y
            linters happy for sources without subtitles. */}
          <track kind="captions" />
          {(source?.subtitles ?? [])
            .filter((sub): sub is typeof sub & { vtt_url: string } => sub.vtt_url != null)
            .map((sub) => (
              // Key by the full URL (which bakes in the
              // HLS source offset) so a reload at a new
              // `-ss` point remounts the track with the
              // updated VTT; otherwise the browser caches
              // the first-fetched cues and subtitles stay
              // shifted by the prior offset.
              <track
                key={`${sub.stream_index}:${sub.vtt_url}`}
                kind="subtitles"
                src={sub.vtt_url}
                srcLang={sub.language ?? undefined}
                label={`sub-${sub.stream_index}`}
              />
            ))}
        </video>

        {/* Cast takeover card */}
        {castHandoff.isCasting && castHandoff.castMediaId != null && (
          <CastOverlay deviceName={castHandoff.castDeviceName ?? null} />
        )}

        {/* Media error overlay */}
        {mediaError && <MediaErrorOverlay message={mediaError} onBack={goBack} />}

        {/* Stall recovery overlay */}
        {stallOverlay && sourceBinding.isLoading && overlayDismissed && (
          <StallOverlayView overlay={stallOverlay} />
        )}

        {/* Initial loading overlay */}
        {loadingOverlay && !overlayDismissed && (
          <LoadingOverlayView overlay={loadingOverlay} fading={overlayFading} />
        )}

        {/* Top overlay passthrough — e.g. streaming
          download badge. Fades alongside the main
          chrome. */}
        {topOverlay && (
          <div
            className={cn(
              'transition-opacity duration-300',
              autoHide.visible ? 'opacity-100' : 'opacity-0 pointer-events-none'
            )}
          >
            {topOverlay}
          </div>
        )}

        {/* Skip intro/credits — always visible (sits above
          the autohide-controlled chrome layer so viewers
          can still skip while controls are hidden). */}
        <SkipButton currentTime={currentTime} source={source} onSeek={seekToSource} />

        {/* Chrome layer — fades with autohide. Two-row
          layout (top + bottom); center area is
          intentionally empty so clicks pass through to
          the video's toggle-play handler. */}
        <div
          className={cn(
            'absolute inset-0 z-10 flex flex-col justify-between pointer-events-none transition-opacity duration-300',
            autoHide.visible ? 'opacity-100' : 'opacity-0'
          )}
        >
          <div className="pointer-events-auto">
            <TopBar title={source?.title} onBack={goBack} />
          </div>
          <div className="pointer-events-auto">
            <ControlBar
              source={source}
              videoRef={videoRef}
              hlsOffsetRef={hlsOffsetRef}
              playing={playing}
              currentTime={currentTime}
              duration={duration}
              buffered={buffered}
              seekExtraLayer={seekExtraLayer}
              volume={volume}
              muted={muted}
              playbackRate={playbackRate}
              isFullscreen={fullscreen.isFullscreen}
              isPip={pip.isPip}
              pipSupported={pip.supported}
              subtitleStreamIndex={subtitleStreamIndex}
              setSubtitleStreamIndex={setSubtitleStreamIndex}
              userHasPickedSubtitleRef={userHasPickedSubtitleRef}
              onRequestPause={onRequestPause}
              onRequestResume={onRequestResume}
              onSeek={seekToSource}
              onPlaybackRateChange={setPlaybackRate}
              onMuteToggle={onMuteToggle}
              onVolumeChange={onVolumeChange}
              onToggleFullscreen={fullscreen.toggle}
              onTogglePip={() => void pip.toggle()}
              onOpenShortcuts={() => setHelpOpen(true)}
            />
          </div>
        </div>

        {helpOpen && <ShortcutsDialog onClose={() => setHelpOpen(false)} />}
      </div>
    </div>
  );
}

export { ArrowDown } from 'lucide-react';
export type {
  LoadingOverlay,
  StallOverlay,
  VideoShellHandle,
  VideoShellProps,
  VideoSource,
} from './types';
// Re-exports for compat with call sites that still import
// from here. `formatTime` is used by the torrent overlay
// for its download-speed badge; ArrowDown is imported
// alongside for the same reason.
export { formatTime, SPEED_OPTIONS } from './types';
