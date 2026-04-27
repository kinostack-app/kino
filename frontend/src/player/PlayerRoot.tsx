/**
 * Unified player — one component driven entirely off
 * `/api/v1/play/{kind}/{entity_id}/prepare`. URL is chosen once
 * based on the initial bytes-available state, and never changes
 * for the session — the backend dispatcher transparently flips
 * byte source (torrent → library) when the import lands. Playhead
 * survives; no remount.
 *
 * All state the chip + overlays need comes from the one `prepare`
 * payload. Progress reports, trickplay, subtitles, HLS sessions
 * all share the entity URL shape, so nothing here branches on
 * "are we streaming or library right now" — the dispatcher's
 * answer is enough.
 */

import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useEffect, useMemo, useRef, useState } from 'react';
import { cancelDownload, prepare as playPrepare, resumeDownload } from '@/api/generated/sdk.gen';
import type { PlayKind, PlayPrepareReply, PlayState } from '@/api/generated/types.gen';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { useTrickplay } from '@/hooks/useTrickplay';
import { getTabId } from '@/lib/tab-id';
import type {
  LoadingOverlay,
  StallOverlay,
  VideoShellHandle,
  VideoSource,
} from '@/player/VideoShell';
import { VideoShell } from '@/player/VideoShell';
import { useCastStore } from '@/state/cast-store';
import { getIncognito } from '@/state/incognito';
import { DOWNLOADS_KEY, LIBRARY_MOVIES_KEY, LIBRARY_SHOWS_KEY } from '@/state/library-cache';
import { useTrickplayTick } from '@/state/trickplay-stream-store';
import { PlaybackInfoChip } from './PlaybackInfoChip';
import { ResumeDialog } from './ResumeDialog';

export interface PlayerRootProps {
  kind: PlayKind;
  entityId: number;
  /** Resume offset in seconds from an external share link. */
  initialResumeAt?: number;
}

type ResumeDecision = 'pending' | 'resume' | 'start-over';

/// A state is "bytes available" when the player can build a
/// VideoSource against `/direct` or `/master.m3u8`. The initial
/// URL choice is pinned to this — once picked, we stay on the
/// same URL for the whole session.
function hasBytes(state: PlayState): boolean {
  return state === 'streaming' || state === 'paused' || state === 'downloaded';
}

export function PlayerRoot({ kind, entityId, initialResumeAt }: PlayerRootProps) {
  const navigate = useNavigate();
  const tabId = useMemo(() => getTabId(), []);
  const currentTimeRef = useRef(0);
  // Latest paused state from the <video> element. Reported on each
  // progress tick so Trakt sees scrobble/pause while the user is
  // paused and scrobble/start only while they're actually watching.
  // Defaults to true — the player starts paused until the first
  // `play` event fires.
  const pausedRef = useRef(true);
  // Imperative handle into the VideoShell — lets the info dialog
  // pause the video while open and resume on close, without
  // lifting the <video> ref all the way up here.
  const videoHandleRef = useRef<VideoShellHandle | null>(null);

  // Single prepare poll drives everything. 3s interval is cheap
  // (one indexed DB lookup per entity) and makes state transitions
  // visible in <=3s without listening for WS events. Stays on even
  // post-bytes so the chip state / transcode progress / download
  // %age keeps refreshing.
  const { data: prepareData, error: prepareError } = useQuery<PlayPrepareReply | null>({
    queryKey: ['kino', 'play', kind, entityId, 'prepare'],
    queryFn: async () => {
      const res = await playPrepare({ path: { kind, entity_id: entityId } });
      if (res.response.status === 202) return null;
      return (res.data as PlayPrepareReply | undefined) ?? null;
    },
    refetchInterval: 3000,
    retry: false,
    // Don't cache the prepare response across component mounts —
    // when the user goes Home → clicks Resume, TanStack would
    // otherwise serve the previously-cached resume_position_secs
    // (+ trickplay_url) while the fresh fetch is in flight, which
    // makes the Resume dialog's thumbnail show the wrong frame
    // for a few hundred ms. Dropping cached data on unmount
    // costs us one extra fetch on the rare re-entry — trivially
    // cheap vs a wrong frame in the user's face.
    gcTime: 0,
    meta: {
      // Other tabs watching the same entity should refetch on
      // `watched` / `unwatched` so their resume position + CTA
      // stay in lockstep with mutations from this tab.
      // `stream_probe_ready` fires once per streaming download
      // when the background ffprobe lands — refetch so the info
      // chip + plan pick up the real source metadata.
      invalidatedBy: ['watched', 'unwatched', 'stream_probe_ready'],
    },
  });

  // Document title reflects the content: "Movie" or "Show · S01E04 · Pilot".
  // Falls back to "Now Playing" until `/prepare` resolves.
  const docTitle = prepareData
    ? prepareData.episode_label
      ? `${prepareData.title} · ${prepareData.episode_label}`
      : prepareData.title
    : 'Now Playing';
  useDocumentTitle(docTitle);

  // The snapshot pins every URL-affecting bit of state at the
  // first-bytes tick, so subsequent `/prepare` polls can't rebuild
  // `source.url` mid-session — VideoShell's source-URL effect
  // would remount <video> and the backend would stop + restart
  // ffmpeg on each poll. Bug in practice: live `server_resume_secs`
  // advances as the user watches, each tick produced a new
  // `?start_time=X` URL → HLS session thrash loop.
  const [urlSnapshot, setUrlSnapshot] = useState<{
    mode: 'direct' | 'hls';
    canDirectPlay: boolean;
    resumeSec: number;
  } | null>(null);

  // User-initiated HLS seek-past-buffered. Non-null value means the
  // URL's `?at=` will be this instead of `urlSnapshot.resumeSec`,
  // triggering a fresh ffmpeg `-ss` restart. Set by the seek-reload
  // callback VideoShell invokes when the user scrubs past the
  // playlist edge; stays set so the URL stays stable between seeks.
  const [hlsSeekOffset, setHlsSeekOffset] = useState<number | null>(null);

  // Cast → local handoff. When a Cast session ends, the store
  // captures the receiver's last playhead in `pendingResumeSec`.
  // We consume it here on the `connected` → non-connected
  // transition and promote it to `hlsSeekOffset` so local
  // playback resumes where the TV left off. Promoting (rather
  // than carrying a parallel state) keeps it sticky across
  // renders — subsequent user seeks still replace it via the
  // same `setHlsSeekOffset` path, no special cascade needed.
  //
  // Direct-play Cast handoff is a follow-up: most Cast
  // receivers can't direct-play the codecs we'd ship
  // raw, so the Cast path almost always runs through HLS
  // transcode. When direct-play Cast happens, on disconnect
  // the local video element's currentTime stays at whatever
  // the initial handoff snapshot was, not the TV's playhead.
  const castConnectedState = useCastStore((s) => s.state);
  const consumePendingResume = useCastStore((s) => s.consumePendingResume);
  const prevCastStateRef = useRef(castConnectedState);
  useEffect(() => {
    const prev = prevCastStateRef.current;
    prevCastStateRef.current = castConnectedState;
    if (prev === 'connected' && castConnectedState !== 'connected') {
      const pending = consumePendingResume();
      if (pending != null && pending > 0) {
        setHlsSeekOffset(Math.max(0, Math.floor(pending)));
      }
    }
  }, [castConnectedState, consumePendingResume]);

  const serverResumeSec = prepareData?.resume_position_secs ?? 0;
  const handoffResumeSec = initialResumeAt && initialResumeAt > 0 ? initialResumeAt : 0;
  const [userDecision, setUserDecision] = useState<'resume' | 'start-over' | null>(null);
  // `needsPrompt` only evaluates BEFORE urlSnapshot is set. The
  // 10s progress reporter keeps pushing `resume_position_secs`
  // up as the user watches, so the live value crosses the 30s
  // threshold mid-play — without this gate, `decision` would
  // flip back to `'pending'` and the dialog would reappear
  // over the running video.
  const needsPrompt = urlSnapshot === null && handoffResumeSec === 0 && serverResumeSec > 30;
  const decision: ResumeDecision = userDecision ?? (needsPrompt ? 'pending' : 'resume');

  useEffect(() => {
    if (urlSnapshot) return;
    if (!prepareData) return;
    if (!hasBytes(prepareData.state)) return;
    if (decision === 'pending') return;
    // Capture the resume-at that applies at this instant. After
    // this lands, the URL is frozen; live progress reporting still
    // writes through to the server, it just doesn't cause re-mounts.
    const resumeSec =
      handoffResumeSec > 0 ? handoffResumeSec : decision === 'resume' ? serverResumeSec : 0;
    // Streaming HLS is the safe default — partial MKVs have the
    // cue-index-at-end issue for direct play (Firefox in particular
    // drifts indefinitely). Library direct-play is fine when codecs
    // allow, so we honour the decision engine's verdict (`plan.method
    // === 'direct_play'`). `plan` is null during failed-resolve and
    // streaming states; both fall through to HLS.
    const directOk =
      prepareData.state === 'downloaded' && prepareData.plan?.method === 'direct_play';
    if (directOk) {
      setUrlSnapshot({ mode: 'direct', canDirectPlay: true, resumeSec });
    } else {
      setUrlSnapshot({ mode: 'hls', canDirectPlay: false, resumeSec });
    }
  }, [prepareData, urlSnapshot, decision, handoffResumeSec, serverResumeSec]);

  // Trickplay URL from prepare; library vs stream is the
  // dispatcher's concern, not ours. The `trickplay_stream_updated` WS
  // event bumps a per-download counter; we read it as the refresh
  // signal so the hover VTT re-fetches as new sprites land during
  // the streaming phase. Without this, the first fetch 404s (before
  // ffmpeg has emitted a sheet) and never retries — the user sees
  // a spinner forever even after the sprites are on disk.
  // Cookie auto-attaches on the trickplay fetch; bare URL is fine.
  // Cross-origin deploys would route this through `/sign-url` —
  // wired by `useTrickplay` itself when it lands.
  const trickplayUrlAuthed = prepareData?.trickplay_url ?? null;
  const trickplayRefreshSignal = useTrickplayTick(prepareData?.download_id ?? null);

  // Audio track selection (library HLS). Null = default. Changing
  // this INTENTIONALLY forces an HLS restart so ffmpeg picks the
  // new audio track — URL includes `?audio_stream=N`.
  const [audioStreamIndex, setAudioStreamIndex] = useState<number | null>(null);
  // Burn-in subtitle selection. Non-null → backend burns this
  // image subtitle (PGS/VOBSUB) into the video via
  // `-filter_complex overlay`. URL rebuild forces ffmpeg
  // restart; only fires for image subs — text subs render via
  // `<track>` without touching the server.
  const [burnInSubtitleIndex, setBurnInSubtitleIndex] = useState<number | null>(null);

  // Build the VideoSource. Key invariants:
  //   - url depends only on {kind, entityId, tabId, urlSnapshot}
  //     — stable for the session once urlSnapshot lands.
  //   - resumeAtSec applies once on first durationchange.
  //   - trackProgressAs stays set for the whole session; the unified
  //     progress endpoint accepts either kind, so VideoShell's
  //     internal 10s reporter works unchanged — we just point it at
  //     the entity URL.
  const source = useMemo<VideoSource | null>(() => {
    if (!urlSnapshot) return null;
    if (!prepareData) return null;
    if (!hasBytes(prepareData.state)) return null;
    if (decision === 'pending') return null;

    const base = `/api/v1/play/${kind}/${entityId}`;
    const qs = new URLSearchParams();
    qs.set('tab', tabId);
    // Cookie auto-attaches on `<video>`/HLS requests in cookie mode.
    // Cross-origin deploys swap to a pre-signed URL via
    // `/sign-url`; the helper wraps that detail.
    // `at` wins over `resumeSec` — once the user seeks past
    // buffered, their explicit target replaces the saved resume
    // point. Both land on ffmpeg as `-ss` via the backend's `at`
    // query param.
    // `at` wins over `resumeSec` — once the user seeks past
    // buffered (or a Cast session hands off a remote playhead
    // back to us), their explicit target replaces the saved
    // resume point. Both land on ffmpeg as `-ss` via the
    // backend's `at` query param.
    const effectiveAt = hlsSeekOffset ?? urlSnapshot.resumeSec;
    if (urlSnapshot.mode === 'hls' && effectiveAt > 0) {
      qs.set('at', String(Math.floor(effectiveAt)));
    }
    if (urlSnapshot.mode === 'hls' && audioStreamIndex != null) {
      qs.set('audio_stream', String(audioStreamIndex));
    }
    if (urlSnapshot.mode === 'hls' && burnInSubtitleIndex != null) {
      qs.set('subtitle_stream', String(burnInSubtitleIndex));
    }
    const url =
      urlSnapshot.mode === 'hls'
        ? `${base}/master.m3u8?${qs.toString()}`
        : `${base}/direct?${qs.toString()}`;

    return {
      mode: urlSnapshot.mode,
      url,
      // Header chip shows "Show · S01E04 · Pilot" for episodes, just
      // the title for movies. Falls back if `/prepare` hasn't landed.
      title:
        (prepareData.episode_label
          ? `${prepareData.title} · ${prepareData.episode_label}`
          : prepareData.title) || 'Now Playing',
      castMediaId: prepareData.media_id ?? undefined,
      trickplayUrl: trickplayUrlAuthed,
      trickplayRefreshSignal,
      // Audio + subtitle tracks flow straight through from the
      // backend's typed PlayPrepareReply. The backend emits each
      // `subtitle_track.vtt_url` without auth; we append the
      // `?api_key=` the `<video>` element can't add itself
      // (can't set headers) plus — for HLS playback that started
      // mid-file via `ffmpeg -ss` — an `offset` that tells the
      // backend to shift every cue timestamp back by the same
      // amount, so cues align with `video.currentTime` (which
      // starts at zero against the trimmed output). Image-subs
      // (burn-in required) carry `vtt_url: null` and stay null —
      // VideoShell filters them.
      subtitles: (prepareData.subtitle_tracks ?? []).map((s) => {
        if (!s.vtt_url) return { ...s, vtt_url: null };
        // Cookie auto-attaches on the `<track>` request; only
        // append `offset=` for HLS sessions that started mid-file
        // (so cues align with `video.currentTime`).
        const offset = urlSnapshot.mode === 'hls' && effectiveAt > 0 ? effectiveAt : 0;
        if (offset > 0) {
          const sep = s.vtt_url.includes('?') ? '&' : '?';
          return { ...s, vtt_url: `${s.vtt_url}${sep}offset=${offset}` };
        }
        return s;
      }),
      audioTracks: prepareData.audio_tracks ?? [],
      audioStreamIndex: audioStreamIndex ?? undefined,
      // Intro / credits timestamps feed the SkipButton. All four
      // are independent; the button renders when the playhead
      // enters an intro range or passes credits_start, and only
      // when the per-show skip flag is on (users keep themes on
      // for shows they love).
      introStartMs: prepareData.intro_start_ms ?? null,
      introEndMs: prepareData.intro_end_ms ?? null,
      creditsStartMs: prepareData.credits_start_ms ?? null,
      creditsEndMs: prepareData.credits_end_ms ?? null,
      skipEnabledForShow: prepareData.skip_enabled_for_show ?? undefined,
      // Auto-skip decision inputs — driven by the `auto_skip_intros`
      // config plus per-season state. The hook `useAutoSkipIntro`
      // reads these and triggers a silent seek when the user crosses
      // into the intro range under `on` / `smart` mode.
      showId: prepareData.show_id ?? null,
      seasonNumber: prepareData.season_number ?? null,
      seasonAnyWatched: prepareData.season_any_watched ?? false,
      autoSkipIntros: prepareData.auto_skip_intros ?? 'smart',
      onAudioStreamChange: (idx) => setAudioStreamIndex(idx),
      onBurnInSubtitleChange: (idx) => setBurnInSubtitleIndex(idx),
      resumeAtSec:
        urlSnapshot.mode === 'direct' && urlSnapshot.resumeSec > 0
          ? urlSnapshot.resumeSec
          : undefined,
      expectedDurationSec: prepareData.duration_secs ?? undefined,
      totalDurationSec: prepareData.duration_secs ?? undefined,
      // Scrubber needs to map `video.currentTime` (which restarts at
      // 0 each time ffmpeg re-spawns with `-ss`) back to source time.
      // VideoShell adds this offset on displayed time / buffered
      // checks and subtracts on seek targets.
      hlsSourceOffsetSec: urlSnapshot.mode === 'hls' && effectiveAt > 0 ? effectiveAt : undefined,
      // Past-buffered seeks in HLS call back into PlayerRoot to
      // bump the seek offset, which rebuilds the URL with `?at=`
      // and trips VideoShell's source-URL effect → new HLS load →
      // backend restarts ffmpeg at the new `-ss`.
      onSeekReload:
        urlSnapshot.mode === 'hls'
          ? (sec) => setHlsSeekOffset(Math.max(0, Math.floor(sec)))
          : undefined,
      // Direct→HLS recovery: triggered from useSourceBinding when
      // `<video>` reports `MediaError::SRC_NOT_SUPPORTED` (code 4) on a
      // direct source. Flipping the snapshot rebuilds `source.url` with
      // `/master.m3u8` (and `?at=resumeSec` so we don't lose the
      // playhead) — the URL and the mode change together so hls.js
      // doesn't end up loading the byte-range `/direct` URL.
      onForceHls:
        urlSnapshot.mode === 'direct' && (prepareData.media_id ?? null) != null
          ? () =>
              setUrlSnapshot((prev) =>
                prev ? { ...prev, mode: 'hls', canDirectPlay: false } : prev
              )
          : undefined,
    };
  }, [
    urlSnapshot,
    prepareData,
    decision,
    kind,
    entityId,
    tabId,
    audioStreamIndex,
    burnInSubtitleIndex,
    trickplayUrlAuthed,
    trickplayRefreshSignal,
    hlsSeekOffset,
  ]);

  // Progress reporting — dedicated 10s timer + beforeunload beacon.
  // Hits the unified POST /api/v1/play/{kind}/{id}/progress which
  // writes straight to the entity row regardless of source. Replaces
  // the old dual-endpoint path (stream vs library) entirely.
  useEffect(() => {
    if (!source) return;
    const report = () => {
      const pos = Math.floor(currentTimeRef.current);
      if (pos <= 0) return;
      const body = JSON.stringify({
        position_secs: pos,
        paused: pausedRef.current,
        incognito: getIncognito(),
      });
      void fetch(`/api/v1/play/${kind}/${entityId}/progress`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body,
      }).catch(() => {});
    };
    const finalBeacon = () => {
      const pos = Math.floor(currentTimeRef.current);
      if (pos <= 0) return;
      const body = new Blob(
        [
          JSON.stringify({
            position_secs: pos,
            final_tick: true,
            incognito: getIncognito(),
          }),
        ],
        { type: 'application/json' }
      );
      // sendBeacon includes same-origin cookies automatically.
      navigator.sendBeacon(`/api/v1/play/${kind}/${entityId}/progress`, body);
    };
    const timer = setInterval(report, 10_000);
    window.addEventListener('beforeunload', finalBeacon);
    return () => {
      clearInterval(timer);
      window.removeEventListener('beforeunload', finalBeacon);
      finalBeacon();
    };
  }, [kind, entityId, source]);

  // Transcode cleanup on unmount — only when we picked HLS (direct
  // play has no ffmpeg session to stop). keepalive so the request
  // survives the imminent navigation.
  const usedHls = urlSnapshot?.mode === 'hls';
  useEffect(() => {
    if (!usedHls) return;
    return () => {
      void fetch(`/api/v1/play/${kind}/${entityId}/transcode?tab=${tabId}`, {
        method: 'DELETE',
        credentials: 'include',
        keepalive: true,
      }).catch(() => {});
    };
  }, [kind, entityId, tabId, usedHls]);

  // ── Resume dialog preview thumbnail ──
  const resumeTrickplay = useTrickplay(trickplayUrlAuthed, {
    refreshSignal: trickplayRefreshSignal,
  });

  const qc = useQueryClient();
  const goBack = () => {
    if (window.history.length > 1) window.history.back();
    else void navigate({ to: '/' });
  };

  // Download id is only meaningful in stream-ish states; used by
  // the cancel-and-back action when the user bails pre-import.
  const streamDownloadId = prepareData?.download_id ?? null;
  const cancelAndGoBack = async () => {
    if (streamDownloadId != null) {
      try {
        await cancelDownload({ path: { id: streamDownloadId } });
        // Same reason as useWatchNow: this tab is about to unmount
        // the player and mount Home. WS `download_failed` will
        // invalidate these caches for other tabs, but with no
        // observers on this tab the invalidate would only mark
        // stale — Home's mount refetch then paints stale data first.
        // Fire-and-forget refetch in parallel with navigation.
        void qc.refetchQueries({ queryKey: [...LIBRARY_SHOWS_KEY] });
        void qc.refetchQueries({ queryKey: [...LIBRARY_MOVIES_KEY] });
        void qc.refetchQueries({ queryKey: [...DOWNLOADS_KEY] });
      } finally {
        goBack();
      }
    } else {
      goBack();
    }
  };

  // ── Unified info chip ──
  // Rendered for every playable state so the imported→streaming
  // transition stays informative — the old inline chip vanished on
  // the library side, leaving the top-right empty. `PlaybackInfoChip`
  // owns its own expanded panel (click to open).
  const downloaded = prepareData?.downloaded_bytes ?? 0;
  const totalSize = prepareData?.total_bytes ?? 0;
  const downloadPct = totalSize > 0 ? Math.min(100, Math.round((downloaded * 100) / totalSize)) : 0;

  // Snapshot of the play-state at the moment the info dialog opens,
  // so we only auto-resume on close when the user was actually
  // watching (not if they'd paused manually before opening the
  // dialog).
  const wasPlayingBeforeInfoOpen = useRef(false);
  const streamTopOverlay =
    prepareData != null ? (
      <div className="absolute top-4 right-4 z-20 flex items-start pointer-events-none">
        <div className="pointer-events-auto">
          <PlaybackInfoChip
            prepareData={prepareData}
            urlMode={urlSnapshot?.mode ?? 'hls'}
            onOpen={() => {
              wasPlayingBeforeInfoOpen.current = !(videoHandleRef.current?.isPaused() ?? true);
              videoHandleRef.current?.pause();
            }}
            onClose={() => {
              if (wasPlayingBeforeInfoOpen.current) videoHandleRef.current?.play();
            }}
          />
        </div>
      </div>
    ) : null;

  // Scrubber download-buffer stripe — only meaningful while the
  // torrent is still filling in; once 100% the whole bar is
  // downloaded and the stripe is redundant with the buffered layer.
  const streamSeekExtraLayer =
    prepareData?.state === 'streaming' || prepareData?.state === 'paused' ? (
      <div
        className="absolute inset-y-0 left-0 rounded-full bg-white/10"
        style={{ width: `${downloadPct}%` }}
        title={`${downloadPct}% downloaded`}
      />
    ) : null;

  // Stall overlays — paused and failed are the user-visible terminal
  // states that block playback progress.
  const logo: LoadingOverlay['logo'] =
    prepareData?.logo_content_type === 'movies' || prepareData?.logo_content_type === 'shows'
      ? {
          contentType: prepareData.logo_content_type,
          entityId: prepareData.logo_entity_id ?? 0,
          palette: prepareData.logo_palette,
        }
      : null;

  const stallTitle = prepareData
    ? prepareData.episode_label
      ? `${prepareData.title} · ${prepareData.episode_label}`
      : prepareData.title
    : '';
  const streamStallOverlay: StallOverlay | undefined =
    prepareData?.state === 'paused'
      ? {
          title: stallTitle,
          logo,
          message: 'Download paused — no more data is arriving. Resume to continue streaming.',
          action: {
            label: 'Resume download',
            onClick: () => {
              if (streamDownloadId != null) void resumeDownload({ path: { id: streamDownloadId } });
            },
          },
        }
      : prepareData?.state === 'failed'
        ? {
            title: stallTitle,
            logo,
            message: prepareData.error_message || 'Download failed — no more data will arrive.',
            action: {
              label: 'Back',
              onClick: goBack,
            },
          }
        : undefined;

  // Loading overlay — drives the multi-stage stepper. States map:
  //   searching → "Finding a release" (stage 0)
  //   queued → "Starting the download" (stage 1)
  //   grabbing → "Reading torrent metadata" (stage 2)
  //   streaming / paused / downloaded → "Buffering" (stage 3)
  const stage = (() => {
    const s = prepareData?.state;
    if (s === 'searching') return 0;
    if (s === 'queued') return 1;
    if (s === 'grabbing') return 2;
    return 3;
  })();
  const STAGES = [
    'Finding a release',
    'Starting the download',
    'Reading torrent metadata',
    'Buffering',
  ];
  const status = (() => {
    const s = prepareData?.state;
    if (prepareError) return '';
    if (s === 'searching') return 'Finding a release';
    if (s === 'queued') return 'Starting the download';
    if (s === 'grabbing') return 'Reading torrent metadata';
    if (s === 'failed') return prepareData?.error_message || 'Search failed';
    if (!source) return 'Preparing playback';
    if (downloadPct > 0 && downloadPct < 100) return `Buffering · ${downloadPct}%`;
    return 'Buffering';
  })();
  const progress = (() => {
    if (prepareError) return 0;
    const s = prepareData?.state;
    if (s === 'searching') return 25;
    if (s === 'queued') return 45;
    if (s === 'grabbing') return 55;
    if (!source) return 70;
    return 100;
  })();
  const errorState = prepareError
    ? { message: 'Couldn’t prepare this one.', action: { label: 'Back', onClick: goBack } }
    : prepareData?.state === 'failed'
      ? {
          message: prepareData.error_message || 'Search failed.',
          action: { label: 'Back', onClick: cancelAndGoBack },
        }
      : undefined;

  const loadingOverlay: LoadingOverlay = {
    title: prepareData?.title || 'Loading',
    logo,
    status,
    stages: STAGES,
    currentStage: stage,
    progress,
    error: errorState,
  };

  const resumePct =
    prepareData?.duration_secs && prepareData.duration_secs > 0 && serverResumeSec > 0
      ? Math.min(1, serverResumeSec / prepareData.duration_secs)
      : 0;
  const resumeThumbCue =
    serverResumeSec > 0 && resumeTrickplay ? resumeTrickplay.cueAt(serverResumeSec) : undefined;

  return (
    <>
      <VideoShell
        source={source}
        handleRef={videoHandleRef}
        topOverlay={streamTopOverlay ?? undefined}
        seekExtraLayer={streamSeekExtraLayer ?? undefined}
        loadingOverlay={loadingOverlay}
        stallOverlay={streamStallOverlay}
        onBack={goBack}
        onPlaybackTime={(s) => {
          currentTimeRef.current = s;
        }}
        onPlayStateChange={(paused) => {
          pausedRef.current = paused;
        }}
      />
      {decision === 'pending' && prepareData && (
        <ResumeDialog
          resumeSec={serverResumeSec}
          resumePct={resumePct}
          title={
            prepareData.episode_label
              ? `${prepareData.title} · ${prepareData.episode_label}`
              : prepareData.title
          }
          thumbCue={resumeThumbCue}
          backdropPath={prepareData.backdrop_path}
          durationSec={prepareData.duration_secs}
          onBack={goBack}
          onResume={() => setUserDecision('resume')}
          onStartOver={() => {
            void fetch(`/api/v1/play/${kind}/${entityId}/progress`, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              credentials: 'include',
              body: JSON.stringify({ position_secs: 0 }),
            }).catch(() => {});
            setUserDecision('start-over');
          }}
        />
      )}
    </>
  );
}
