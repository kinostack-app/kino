/**
 * Persistent top-right chip during playback, shown in both streaming
 * and imported modes. Compact pill by default; clicking opens a
 * slide-in panel with full technical specs (video codec, HDR, audio
 * tracks, subtitles, transcode mode + reasons, live encode stats).
 *
 * One source of truth: `PlayPrepareReply`. Every field rendered here
 * is a generated type field. No hand-rolled string unions for
 * playback method / HDR label / codec names — those narrow on the
 * backend's generated enums where possible, or use pretty-print
 * helpers that consume the backend's lowercase strings.
 */

import {
  ArrowDown,
  AudioLines,
  Captions,
  ChevronRight,
  Cpu,
  Film,
  Gauge,
  Info,
  Languages,
  X,
  Zap,
} from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type {
  AudioTrack,
  BrowserFamily,
  ClientOs,
  DetectedClient,
  PlaybackMethod,
  PlayPrepareReply,
  SubtitleTrack,
  TranscodeReason,
  VideoTrackInfo,
} from '@/api/generated/types.gen';
import { cn } from '@/lib/utils';

// ─── Public surface ───────────────────────────────────────────────

interface PlaybackInfoChipProps {
  prepareData: PlayPrepareReply;
  /** URL mode the VideoShell actually spawned — `'hls'` or `'direct'`. */
  urlMode: 'hls' | 'direct';
  /** Fired when the user opens the detail panel. Callers use this
   *  to pause the underlying video so it's not running behind the
   *  translucent backdrop. */
  onOpen?: () => void;
  /** Fired when the panel closes. Callers restore playback state
   *  (typically resume if the user was playing before they opened
   *  the panel). */
  onClose?: () => void;
}

export function PlaybackInfoChip({ prepareData, urlMode, onOpen, onClose }: PlaybackInfoChipProps) {
  const [open, setOpen] = useState(false);
  const handleOpen = () => {
    setOpen(true);
    onOpen?.();
  };
  const handleClose = () => {
    setOpen(false);
    onClose?.();
  };
  return (
    <>
      <ChipButton prepareData={prepareData} urlMode={urlMode} onClick={handleOpen} />
      {open && <InfoPanel prepareData={prepareData} urlMode={urlMode} onClose={handleClose} />}
    </>
  );
}

// ─── Compact chip ─────────────────────────────────────────────────

function ChipButton({
  prepareData,
  urlMode,
  onClick,
}: {
  prepareData: PlayPrepareReply;
  urlMode: 'hls' | 'direct';
  onClick: () => void;
}) {
  const state = prepareData.state;

  // Error and paused states get their own compact pill — no detail
  // worth showing, and the colour coding carries the message.
  if (state === 'failed') {
    return (
      <PillShell variant="danger">
        <StatusDot tone="danger" />
        <span className="uppercase tracking-wider text-[10px]">Download failed</span>
      </PillShell>
    );
  }
  if (state === 'paused') {
    const pct = pctDownloaded(prepareData);
    return (
      <PillShell variant="muted">
        <span className="uppercase tracking-wider text-[10px]">Paused</span>
        <Sep />
        <span className="tabular-nums">{pct}%</span>
      </PillShell>
    );
  }

  const method = prepareData.plan?.method ?? (state === 'streaming' ? 'transcode' : null);
  const methodLabel = methodToLabel(state, method);
  const tone = methodToTone(state, method);

  const video = prepareData.video;
  const sourceCodec = prettyVideoCodec(prepareData.video_codec ?? video?.codec);
  const resLabel = nominalResolution(video?.width, video?.height);
  const hdr = hdrShortLabel(video?.hdr_format, video?.color_transfer);

  const downloadPct = state === 'streaming' ? pctDownloaded(prepareData) : null;
  const speed = prepareData.download_speed ?? 0;

  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'group pointer-events-auto flex items-center gap-2 px-3 py-1.5 rounded-full',
        'bg-black/60 backdrop-blur-md ring-1 ring-white/10 text-[11px] font-medium text-white/90',
        'hover:bg-black/70 hover:ring-white/20 transition',
        'focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]'
      )}
      aria-label="Show playback details"
    >
      <StatusDot tone={tone} pulse={state === 'streaming'} />
      <span
        className="uppercase tracking-wider text-[10px]"
        style={{ color: tone === 'streaming' ? 'var(--accent)' : undefined }}
      >
        {methodLabel}
      </span>
      {(sourceCodec || resLabel || hdr) && (
        <>
          <Sep />
          <span className="tabular-nums flex items-center gap-1.5">
            {sourceCodec && <span>{sourceCodec}</span>}
            {resLabel && (
              <>
                {sourceCodec && <span className="text-white/30">·</span>}
                <span>{resLabel}</span>
              </>
            )}
            {hdr && (
              <>
                <span className="text-white/30">·</span>
                <span className={cn(hdr === 'SDR' ? 'text-white/60' : 'text-amber-200')}>
                  {hdr}
                </span>
              </>
            )}
          </span>
        </>
      )}
      {state === 'streaming' && (
        <>
          <Sep />
          <ArrowDown size={11} className="text-white/60" />
          <span className="tabular-nums">{formatSpeed(speed)}</span>
          {downloadPct != null && (
            <>
              <Sep />
              <span className="tabular-nums">{downloadPct}%</span>
            </>
          )}
        </>
      )}
      <span className="ml-0.5 text-white/40 group-hover:text-white/70 transition">
        <ChevronRight size={12} />
      </span>
      <span className="sr-only">· URL mode {urlMode}</span>
    </button>
  );
}

function PillShell({
  children,
  variant,
}: {
  children: React.ReactNode;
  variant: 'muted' | 'danger';
}) {
  return (
    <div
      className={cn(
        'flex items-center gap-2 px-3 py-1.5 rounded-full bg-black/60 backdrop-blur-md text-[11px] font-medium ring-1',
        variant === 'danger' ? 'text-red-300 ring-red-500/40' : 'text-white/70 ring-white/10'
      )}
    >
      {children}
    </div>
  );
}

function Sep() {
  return <span className="text-white/30">·</span>;
}

function StatusDot({
  tone,
  pulse,
}: {
  tone: 'direct' | 'remux' | 'transcode' | 'streaming' | 'danger';
  pulse?: boolean;
}) {
  const color =
    tone === 'direct'
      ? 'bg-emerald-400'
      : tone === 'remux'
        ? 'bg-amber-400'
        : tone === 'transcode'
          ? 'bg-rose-400'
          : tone === 'streaming'
            ? 'bg-[var(--accent)]'
            : 'bg-red-400';
  return (
    <span
      className={cn('inline-block w-1.5 h-1.5 rounded-full', color, pulse && 'animate-pulse')}
    />
  );
}

// ─── Expanded panel ───────────────────────────────────────────────

function InfoPanel({
  prepareData,
  urlMode,
  onClose,
}: {
  prepareData: PlayPrepareReply;
  urlMode: 'hls' | 'direct';
  onClose: () => void;
}) {
  const overlayRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onClose]);

  useEffect(() => {
    panelRef.current?.focus();
  }, []);

  return createPortal(
    <div
      ref={overlayRef}
      className={cn(
        'fixed inset-0 z-[70] flex items-center justify-center p-4 md:p-8',
        // Soft tint over the player so it stays visible but
        // obviously demoted — the panel itself is translucent too,
        // so the backdrop does most of the "step back" work.
        'bg-black/45 backdrop-blur-[3px]',
        'animate-in fade-in-0 duration-150'
      )}
      onClick={(e) => {
        if (e.target === overlayRef.current) onClose();
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') onClose();
      }}
      role="dialog"
      aria-modal="true"
      aria-labelledby="playback-info-title"
    >
      <div
        ref={panelRef}
        tabIndex={-1}
        className={cn(
          'relative w-full max-w-5xl max-h-[88vh] flex flex-col',
          // Frosted-glass card: translucent surface with a heavy
          // backdrop blur so the player shape bleeds through but
          // text stays readable. The thin ring does the visual
          // work of a border without the hard edge.
          'bg-[var(--bg-secondary)]/80 backdrop-blur-2xl',
          'rounded-2xl ring-1 ring-white/10',
          'shadow-[0_20px_60px_-10px_rgba(0,0,0,0.8)]',
          'focus:outline-none overflow-hidden',
          'animate-in zoom-in-95 fade-in-0 duration-200'
        )}
      >
        <header className="flex items-center justify-between px-6 py-4 border-b border-white/5 bg-white/[0.03]">
          <div className="flex items-center gap-3 min-w-0">
            <div className="w-9 h-9 rounded-xl bg-white/5 grid place-items-center shrink-0">
              <Info size={17} className="text-white/70" />
            </div>
            <div className="min-w-0">
              <h2 id="playback-info-title" className="font-semibold text-white truncate">
                Playback details
              </h2>
              <p className="text-[12px] text-[var(--text-muted)] -mt-0.5 truncate">
                {prepareData.title}
                {prepareData.episode_label ? ` · ${prepareData.episode_label}` : ''}
              </p>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="w-9 h-9 rounded-lg grid place-items-center text-white/60 hover:text-white hover:bg-white/10 transition shrink-0"
            aria-label="Close"
          >
            <X size={17} />
          </button>
        </header>

        <div className="overflow-y-auto">
          <div className="p-6 grid gap-5 md:grid-cols-2">
            {/* Left column: playback state + video specs — the stuff
                the user cares about first. */}
            <div className="space-y-5 min-w-0">
              <PlaybackSection prepareData={prepareData} urlMode={urlMode} />
              <VideoSection
                video={prepareData.video ?? undefined}
                container={prepareData.container ?? undefined}
              />
              <SourceSection prepareData={prepareData} />
              {prepareData.state === 'streaming' && <DownloadSection prepareData={prepareData} />}
            </div>
            {/* Right column: the list-shaped sections (audio,
                subtitles). Their content caps its own height so a
                film with 30 subtitle tracks doesn't stretch the
                whole dialog. */}
            <div className="space-y-5 min-w-0">
              <AudioSection
                tracks={prepareData.audio_tracks ?? []}
                selectedIndex={prepareData.plan?.selected_audio_stream ?? undefined}
              />
              <SubtitleSection tracks={prepareData.subtitle_tracks ?? []} />
            </div>
          </div>
        </div>
      </div>
    </div>,
    document.body
  );
}

// ─── Sections ─────────────────────────────────────────────────────

function SectionCard({
  title,
  icon,
  children,
  rightSlot,
}: {
  title: string;
  icon: React.ReactNode;
  children: React.ReactNode;
  rightSlot?: React.ReactNode;
}) {
  return (
    <section className="rounded-xl bg-white/5 border border-white/10 overflow-hidden">
      <header className="flex items-center justify-between gap-2 px-4 py-2.5 border-b border-white/10 bg-white/[0.02]">
        <div className="flex items-center gap-2 text-[11px] font-semibold uppercase tracking-wider text-white/70">
          <span className="text-[var(--accent)]">{icon}</span>
          {title}
        </div>
        {rightSlot}
      </header>
      <div className="px-4 py-3">{children}</div>
    </section>
  );
}

function Row({
  label,
  value,
  highlight,
}: {
  label: string;
  value: React.ReactNode;
  highlight?: boolean;
}) {
  if (value == null || value === '' || value === false) return null;
  return (
    <div className="flex items-baseline justify-between gap-4 py-1 text-sm">
      <span className="text-[var(--text-muted)] text-[12px] shrink-0">{label}</span>
      <span
        className={cn(
          'text-right text-white tabular-nums break-all',
          highlight && 'text-[var(--accent)] font-medium'
        )}
      >
        {value}
      </span>
    </div>
  );
}

function PlaybackSection({
  prepareData,
  urlMode,
}: {
  prepareData: PlayPrepareReply;
  urlMode: 'hls' | 'direct';
}) {
  const state = prepareData.state;
  const method = prepareData.plan?.method ?? (state === 'streaming' ? 'transcode' : null);
  const tone = methodToTone(state, method);
  const methodLabel = methodToLabel(state, method);
  const reasons = prepareData.plan?.transcode_reasons ?? [];
  const hwBackend = prepareData.hw_backend;
  const progress = prepareData.live_progress;
  const detected = prepareData.detected_client;

  return (
    <SectionCard
      title="Playback"
      icon={<Zap size={13} />}
      rightSlot={
        <div className="flex items-center gap-1.5">
          <StatusDot tone={tone} pulse={state === 'streaming'} />
          <span className="text-[11px] font-medium text-white/90">{methodLabel}</span>
        </div>
      }
    >
      <Row label="URL mode" value={urlMode.toUpperCase()} />
      {detected && (
        <Row
          label="Detected client"
          value={
            <span title={detected.ua_display ?? undefined}>{detectedClientLabel(detected)}</span>
          }
        />
      )}
      {(method === 'transcode' || state === 'streaming') && (
        <>
          <Row
            label="Acceleration"
            value={hwBackend ? hwBackendLabel(hwBackend) : 'Software (CPU)'}
          />
          {progress && (
            <>
              <Row
                label="Encode speed"
                value={formatEncodeSpeed(progress.speed)}
                highlight={progress.speed < 1}
              />
              {progress.bitrate_kbps != null && (
                <Row label="Output bitrate" value={formatBitrateKbps(progress.bitrate_kbps)} />
              )}
            </>
          )}
        </>
      )}
      {reasons.length > 0 && (
        <div className="pt-2">
          <div className="text-[11px] text-[var(--text-muted)] mb-1.5">Transcoding because</div>
          <div className="flex flex-wrap gap-1">
            {reasons.map((r) => (
              <span
                key={r}
                className="px-2 py-0.5 rounded-md bg-rose-500/15 text-rose-200 text-[10.5px] ring-1 ring-rose-500/30"
                title={transcodeReasonTooltip(r)}
              >
                {transcodeReasonLabel(r)}
              </span>
            ))}
          </div>
        </div>
      )}
    </SectionCard>
  );
}

function VideoSection({ video, container }: { video?: VideoTrackInfo; container?: string }) {
  if (!video) {
    return (
      <SectionCard title="Video" icon={<Film size={13} />}>
        <div className="text-[12px] text-[var(--text-muted)]">
          No probe data yet. For streaming sources this lands once import completes.
        </div>
      </SectionCard>
    );
  }
  const hdr = hdrFullLabel(video.hdr_format, video.color_transfer);
  const bitDepth = video.bit_depth ?? bitDepthFromPixFmt(video.pixel_format);
  return (
    <SectionCard title="Video" icon={<Film size={13} />}>
      <Row label="Codec" value={prettyVideoCodec(video.codec)} />
      <Row
        label="Resolution"
        value={
          video.width && video.height
            ? `${video.width} × ${video.height}${nominalResolution(video.width, video.height) ? ` · ${nominalResolution(video.width, video.height)}` : ''}`
            : null
        }
      />
      <Row
        label="Frame rate"
        value={video.framerate ? `${video.framerate.toFixed(3).replace(/\.?0+$/, '')} fps` : null}
      />
      <Row label="HDR" value={hdr} highlight={hdr !== 'SDR'} />
      <Row label="Bit depth" value={bitDepth ? `${bitDepth}-bit` : null} />
      <Row label="Colour space" value={video.color_space?.toUpperCase()} />
      <Row label="Colour primaries" value={video.color_primaries?.toUpperCase()} />
      <Row label="Transfer" value={video.color_transfer?.toUpperCase()} />
      <Row label="Bitrate" value={video.bitrate ? formatBitrateBps(video.bitrate) : null} />
      <Row label="Pixel format" value={video.pixel_format} />
      {container && <Row label="Container" value={container.toUpperCase()} />}
    </SectionCard>
  );
}

function AudioSection({ tracks, selectedIndex }: { tracks: AudioTrack[]; selectedIndex?: number }) {
  return (
    <SectionCard
      title={tracks.length > 1 ? `Audio (${tracks.length})` : 'Audio'}
      icon={<AudioLines size={13} />}
    >
      {tracks.length === 0 ? (
        <div className="text-[12px] text-[var(--text-muted)]">No audio tracks detected.</div>
      ) : (
        <div className="space-y-2 max-h-[280px] overflow-y-auto pr-1 -mr-1">
          {tracks.map((t) => {
            const isSelected = selectedIndex === t.stream_index;
            return (
              <div
                key={t.stream_index}
                className={cn(
                  'rounded-lg border p-3 transition',
                  isSelected
                    ? 'border-[var(--accent)]/50 bg-[var(--accent)]/5'
                    : 'border-white/10 bg-white/[0.02]'
                )}
              >
                <div className="flex items-center justify-between gap-2 mb-1.5">
                  <div className="text-[13px] font-medium text-white truncate">
                    {t.language ? t.language.toUpperCase() : `Track ${t.stream_index}`}
                    {isSelected && (
                      <span className="ml-2 text-[10px] uppercase tracking-wider text-[var(--accent)]">
                        Active
                      </span>
                    )}
                  </div>
                  <div className="flex items-center gap-1">
                    {t.is_default && <Badge tone="neutral">Default</Badge>}
                    {t.is_atmos && <Badge tone="amber">Atmos</Badge>}
                    {(t.roles ?? [])
                      .filter((r) => r !== 'main')
                      .map((r) => (
                        <Badge key={r} tone="neutral">
                          {r}
                        </Badge>
                      ))}
                  </div>
                </div>
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11.5px] text-white/70 tabular-nums">
                  <span>{prettyAudioCodec(t.codec)}</span>
                  {channelsLabel(t.channel_layout, t.channels) && (
                    <span>{channelsLabel(t.channel_layout, t.channels)}</span>
                  )}
                  {t.sample_rate != null && <span>{formatSampleRate(t.sample_rate)}</span>}
                  {t.bit_depth != null && <span>{t.bit_depth}-bit</span>}
                  {t.bitrate != null && <span>{formatBitrateBps(t.bitrate)}</span>}
                </div>
                {t.title && (
                  <div
                    className="text-[11px] text-[var(--text-muted)] mt-1 truncate"
                    title={t.title}
                  >
                    {t.title}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </SectionCard>
  );
}

function SubtitleSection({ tracks }: { tracks: SubtitleTrack[] }) {
  if (tracks.length === 0) {
    return (
      <SectionCard title="Subtitles" icon={<Captions size={13} />}>
        <div className="text-[12px] text-[var(--text-muted)]">No subtitle tracks.</div>
      </SectionCard>
    );
  }
  return (
    <SectionCard title={`Subtitles (${tracks.length})`} icon={<Captions size={13} />}>
      <div className="space-y-1.5 max-h-[320px] overflow-y-auto pr-1 -mr-1">
        {tracks.map((t) => (
          <div
            key={t.stream_index}
            className="flex items-center justify-between gap-2 rounded-md bg-white/[0.03] border border-white/5 px-3 py-2"
          >
            <div className="flex items-center gap-2 min-w-0">
              <Languages size={12} className="text-white/40 shrink-0" />
              <span className="text-[12.5px] text-white truncate">
                {t.language ? t.language.toUpperCase() : t.title || `Track ${t.stream_index}`}
              </span>
            </div>
            <div className="flex items-center gap-1 shrink-0">
              {t.is_default && <Badge tone="neutral">Default</Badge>}
              {t.is_forced && <Badge tone="amber">Forced</Badge>}
              {t.is_hearing_impaired && <Badge tone="neutral">SDH</Badge>}
              {t.is_external && <Badge tone="neutral">External</Badge>}
              {t.is_commentary && <Badge tone="neutral">Commentary</Badge>}
              <span className="text-[10px] text-[var(--text-muted)] uppercase ml-1">
                {prettySubtitleCodec(t.codec)}
              </span>
            </div>
          </div>
        ))}
      </div>
    </SectionCard>
  );
}

function SourceSection({ prepareData }: { prepareData: PlayPrepareReply }) {
  const isStreaming = prepareData.state === 'streaming';
  return (
    <SectionCard title="Source" icon={<Gauge size={13} />}>
      <Row
        label={isStreaming ? 'Stream size' : 'File size'}
        value={
          (isStreaming ? prepareData.total_bytes : prepareData.file_size_bytes)
            ? formatBytes(
                (isStreaming ? prepareData.total_bytes : prepareData.file_size_bytes) as number
              )
            : null
        }
      />
      <Row
        label="Duration"
        value={prepareData.duration_secs ? formatDuration(prepareData.duration_secs) : null}
      />
      <Row
        label="Total bitrate"
        value={
          prepareData.total_bitrate_bps ? formatBitrateBps(prepareData.total_bitrate_bps) : null
        }
      />
      {prepareData.media_id != null && <Row label="Media ID" value={`#${prepareData.media_id}`} />}
      {prepareData.download_id != null && (
        <Row label="Download ID" value={`#${prepareData.download_id}`} />
      )}
    </SectionCard>
  );
}

function DownloadSection({ prepareData }: { prepareData: PlayPrepareReply }) {
  const pct = pctDownloaded(prepareData);
  const downloaded = prepareData.downloaded_bytes ?? 0;
  const total = prepareData.total_bytes ?? 0;
  return (
    <SectionCard
      title="Download"
      icon={<Cpu size={13} />}
      rightSlot={<span className="text-[11px] text-[var(--accent)] tabular-nums">{pct}%</span>}
    >
      <div className="mb-3">
        <div className="h-1.5 w-full rounded-full bg-white/10 overflow-hidden">
          <div className="h-full rounded-full bg-[var(--accent)]" style={{ width: `${pct}%` }} />
        </div>
      </div>
      <Row label="Speed" value={formatSpeed(prepareData.download_speed ?? 0)} />
      <Row
        label="Downloaded"
        value={
          total ? `${formatBytes(downloaded)} / ${formatBytes(total)}` : formatBytes(downloaded)
        }
      />
    </SectionCard>
  );
}

function Badge({ children, tone }: { children: React.ReactNode; tone: 'neutral' | 'amber' }) {
  return (
    <span
      className={cn(
        'px-1.5 py-0.5 rounded text-[10px] uppercase tracking-wider font-medium',
        tone === 'amber'
          ? 'bg-amber-400/15 text-amber-200 ring-1 ring-amber-400/30'
          : 'bg-white/10 text-white/80 ring-1 ring-white/10'
      )}
    >
      {children}
    </span>
  );
}

// ─── Format / label helpers ──────────────────────────────────────

function methodToLabel(state: PlayPrepareReply['state'], method: PlaybackMethod | null): string {
  if (state === 'streaming') return 'Streaming';
  switch (method) {
    case 'direct_play':
      return 'Direct play';
    case 'remux':
      return 'Remux';
    case 'transcode':
      return 'Transcode';
    default:
      return 'Playing';
  }
}

function methodToTone(
  state: PlayPrepareReply['state'],
  method: PlaybackMethod | null
): 'direct' | 'remux' | 'transcode' | 'streaming' | 'danger' {
  if (state === 'streaming') return 'streaming';
  switch (method) {
    case 'direct_play':
      return 'direct';
    case 'remux':
      return 'remux';
    case 'transcode':
      return 'transcode';
    default:
      return 'direct';
  }
}

function pctDownloaded(p: PlayPrepareReply): number {
  const downloaded = p.downloaded_bytes ?? 0;
  const total = p.total_bytes ?? 0;
  if (total <= 0) return 0;
  return Math.min(100, Math.round((downloaded * 100) / total));
}

function nominalResolution(w?: number | null, h?: number | null): string | null {
  if (!w || !h) return null;
  const long = Math.max(w, h);
  if (long >= 7000) return '8K';
  if (long >= 3500) return '4K';
  if (long >= 2500) return '1440p';
  if (long >= 1800) return '1080p';
  if (long >= 1200) return '720p';
  if (long >= 700) return '480p';
  return `${h}p`;
}

function prettyVideoCodec(c?: string | null): string | null {
  if (!c) return null;
  const lc = c.toLowerCase();
  switch (lc) {
    case 'h264':
    case 'avc':
      return 'H.264';
    case 'hevc':
    case 'h265':
      return 'HEVC';
    case 'av1':
      return 'AV1';
    case 'vp9':
      return 'VP9';
    case 'mpeg4':
      return 'MPEG-4';
    case 'mpeg2video':
      return 'MPEG-2';
    default:
      return lc.toUpperCase();
  }
}

function prettyAudioCodec(c: string): string {
  const lc = c.toLowerCase();
  switch (lc) {
    case 'aac':
      return 'AAC';
    case 'ac3':
      return 'AC-3';
    case 'eac3':
      return 'E-AC-3';
    case 'truehd':
      return 'TrueHD';
    case 'dts':
      return 'DTS';
    case 'flac':
      return 'FLAC';
    case 'opus':
      return 'Opus';
    case 'mp3':
      return 'MP3';
    case 'pcm_s16le':
    case 'pcm_s24le':
      return 'PCM';
    default:
      return lc.toUpperCase();
  }
}

function prettySubtitleCodec(c: string): string {
  const lc = c.toLowerCase();
  switch (lc) {
    case 'subrip':
    case 'srt':
      return 'SRT';
    case 'ass':
      return 'ASS';
    case 'ssa':
      return 'SSA';
    case 'webvtt':
      return 'VTT';
    case 'mov_text':
      return 'MOV';
    case 'hdmv_pgs_subtitle':
    case 'pgssub':
      return 'PGS';
    case 'dvd_subtitle':
    case 'dvdsub':
      return 'VOBSUB';
    case 'dvb_subtitle':
      return 'DVB';
    default:
      return lc.toUpperCase();
  }
}

function hdrShortLabel(hdrFormat?: string | null, colorTransfer?: string | null): string | null {
  const full = hdrFullLabel(hdrFormat, colorTransfer);
  if (!full || full === 'SDR') return null;
  // Tighter for chip: "Dolby Vision" → "DV", keep HDR10/HDR10+ as-is.
  if (full.startsWith('Dolby Vision')) return 'DV';
  if (full === 'HDR10+') return 'HDR10+';
  return full;
}

function hdrFullLabel(hdrFormat?: string | null, colorTransfer?: string | null): string | null {
  const hf = hdrFormat?.toLowerCase() ?? '';
  if (hf.includes('dolby vision') || hf.includes('dovi')) return 'Dolby Vision';
  if (hf.includes('hdr10+') || hf.includes('hdr10plus')) return 'HDR10+';
  if (hf.includes('hdr10') || hf === 'pq') return 'HDR10';
  if (hf === 'hlg') return 'HLG';
  // Fall back to color transfer when no hdr_format came through.
  const ct = colorTransfer?.toLowerCase() ?? '';
  if (ct.includes('smpte2084') || ct === 'pq') return 'HDR10';
  if (ct.includes('arib') || ct === 'hlg') return 'HLG';
  if (ct === 'bt709' || ct === 'bt470bg' || ct === 'smpte170m' || ct === '') {
    // Only report SDR when we have enough signal to be confident;
    // empty everywhere → null (just hide the label) rather than
    // lying about SDR on a probe-less streaming source.
    return hdrFormat != null || colorTransfer != null ? 'SDR' : null;
  }
  return null;
}

function channelsLabel(layout?: string | null, channels?: number | null): string | null {
  if (layout) {
    const lc = layout.toLowerCase();
    if (lc === 'stereo') return 'Stereo';
    if (lc === 'mono') return 'Mono';
    // Layouts like "5.1(side)" → "5.1"
    const match = lc.match(/^(\d\.\d)/);
    if (match) return match[1];
    return layout;
  }
  if (channels) return `${channels} ch`;
  return null;
}

function detectedClientLabel(d: DetectedClient): string {
  const family = browserFamilyLabel(d.family);
  const os = clientOsLabel(d.os);
  if (!family) return d.preset;
  return os ? `${family} · ${os}` : family;
}

function browserFamilyLabel(f: BrowserFamily): string {
  switch (f) {
    case 'firefox':
      return 'Firefox';
    case 'chromium':
      return 'Chrome / Chromium';
    case 'edge':
      return 'Edge';
    case 'safari':
      return 'Safari';
    case 'fire_tv':
      return 'Fire TV';
    case 'chromecast':
      return 'Chromecast';
    case 'apple_tv':
      return 'Apple TV';
    case 'lg_webos':
      return 'LG webOS';
    case 'samsung_tizen':
      return 'Samsung Tizen';
    case 'unknown':
      return '';
  }
}

function clientOsLabel(os: ClientOs): string {
  switch (os) {
    case 'windows':
      return 'Windows';
    case 'mac_os':
      return 'macOS';
    case 'linux':
      return 'Linux';
    case 'ios':
      return 'iOS';
    case 'android':
      return 'Android';
    case 'other':
      return '';
  }
}

function hwBackendLabel(b: string): string {
  switch (b.toLowerCase()) {
    case 'nvenc':
      return 'NVENC (NVIDIA)';
    case 'vaapi':
      return 'VA-API';
    case 'qsv':
      return 'Quick Sync (Intel)';
    case 'videotoolbox':
      return 'VideoToolbox (Apple)';
    case 'amf':
      return 'AMF (AMD)';
    default:
      return b.toUpperCase();
  }
}

function transcodeReasonLabel(r: TranscodeReason): string {
  switch (r) {
    case 'container_not_supported':
      return 'Container';
    case 'video_codec_not_supported':
      return 'Video codec';
    case 'video_profile_not_supported':
      return 'Video profile';
    case 'video_level_not_supported':
      return 'Video level';
    case 'video_bit_depth_not_supported':
      return 'Bit depth';
    case 'video_range_type_not_supported':
      return 'HDR / tone map';
    case 'video_resolution_not_supported':
      return 'Resolution';
    case 'video_framerate_not_supported':
      return 'Frame rate';
    case 'video_bitrate_not_supported':
      return 'Video bitrate';
    case 'audio_codec_not_supported':
      return 'Audio codec';
    case 'audio_channels_not_supported':
      return 'Audio channels';
    case 'audio_sample_rate_not_supported':
      return 'Sample rate';
    case 'audio_bit_depth_not_supported':
      return 'Audio bit depth';
    case 'audio_bitrate_not_supported':
      return 'Audio bitrate';
    case 'subtitle_codec_not_supported':
      return 'Burn-in subs';
  }
}

function transcodeReasonTooltip(r: TranscodeReason): string {
  switch (r) {
    case 'container_not_supported':
      return "Container format can't be direct-played in this browser.";
    case 'video_codec_not_supported':
      return "Video codec isn't supported — re-encoding to H.264.";
    case 'video_range_type_not_supported':
      return 'HDR content being tone-mapped to SDR for display.';
    case 'audio_codec_not_supported':
      return "Audio codec isn't supported — re-encoding to AAC.";
    case 'audio_channels_not_supported':
      return "Audio layout isn't supported — downmixing.";
    case 'subtitle_codec_not_supported':
      return 'Image-based subtitle being burnt into the video.';
    default:
      return `Transcoding because: ${r.replaceAll('_', ' ')}`;
  }
}

function bitDepthFromPixFmt(p?: string | null): number | null {
  if (!p) return null;
  const lc = p.toLowerCase();
  if (lc.includes('10le') || lc.includes('10be') || lc.includes('p010')) return 10;
  if (lc.includes('12le') || lc.includes('12be')) return 12;
  if (lc.includes('16le') || lc.includes('16be')) return 16;
  return 8;
}

function formatBitrateBps(bps: number): string {
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(bps >= 10_000_000 ? 1 : 2)} Mbps`;
  if (bps >= 1_000) return `${Math.round(bps / 1_000)} kbps`;
  return `${bps} bps`;
}

function formatBitrateKbps(kbps: number): string {
  return formatBitrateBps(kbps * 1000);
}

function formatEncodeSpeed(speed: number): string {
  if (speed < 0.01) return '< 0.01×';
  return `${speed.toFixed(2)}×`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(0)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

function formatSpeed(bps: number): string {
  if (bps <= 0) return '—';
  if (bps < 1024) return `${bps.toFixed(0)} B/s`;
  if (bps < 1024 ** 2) return `${(bps / 1024).toFixed(0)} KB/s`;
  return `${(bps / 1024 ** 2).toFixed(1)} MB/s`;
}

function formatSampleRate(hz: number): string {
  if (hz >= 1000) return `${(hz / 1000).toFixed(hz % 1000 === 0 ? 0 : 1)} kHz`;
  return `${hz} Hz`;
}

function formatDuration(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m.toString().padStart(2, '0')}m ${s.toString().padStart(2, '0')}s`;
  return `${m}m ${s.toString().padStart(2, '0')}s`;
}
