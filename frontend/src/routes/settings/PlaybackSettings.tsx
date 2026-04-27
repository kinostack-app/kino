import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Activity,
  AlertTriangle,
  Check,
  Download,
  Lightbulb,
  RotateCcw,
  Sparkles,
  Square,
  X,
} from 'lucide-react';
import { useState } from 'react';
import {
  cancelFfmpegDownload,
  getFfmpegDownload,
  probe,
  revertFfmpegToSystem,
  startFfmpegDownload,
  stopTranscodeSession,
  testTranscode,
  transcodeSessions,
  transcodeStats,
} from '@/api/generated/sdk.gen';
import type {
  BackendStatus,
  FfmpegDownloadState,
  HwBackend,
  HwCapabilities,
  TestTranscodeResult,
} from '@/api/generated/types.gen';
import {
  FormField,
  NumberInput,
  SelectInput,
  TestButton,
  TextInput,
  Toggle,
} from '@/components/settings/FormField';
import { cn } from '@/lib/utils';
import { useSettingsContext } from './SettingsLayout';

// ── Constants ────────────────────────────────────────────────────

/** Display label for each hardware backend — matches backend `HwBackend::label`. */
const BACKEND_LABEL: Record<HwBackend, string> = {
  vaapi: 'VAAPI',
  nvenc: 'NVENC',
  qsv: 'Quick Sync',
  videotoolbox: 'VideoToolbox',
  amf: 'AMF',
};

const HW_OPTIONS = [
  { value: 'none', label: 'None (CPU)' },
  { value: 'vaapi', label: 'VAAPI (Intel/AMD)' },
  { value: 'nvenc', label: 'NVENC (NVIDIA)' },
  { value: 'qsv', label: 'Quick Sync (Intel)' },
  { value: 'videotoolbox', label: 'VideoToolbox (macOS)' },
];

function hwAccelLabel(value: string): string {
  return HW_OPTIONS.find((o) => o.value === value)?.label ?? value;
}

/**
 * Render a short, human-readable duration. Seconds up to 60,
 * minutes up to 60, then hours + minutes. Keeps the session row
 * width stable without a monospace font.
 */
function formatDuration(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) {
    const s = secs % 60;
    return s === 0 ? `${mins}m` : `${mins}m ${s}s`;
  }
  const hrs = Math.floor(mins / 60);
  const m = mins % 60;
  return m === 0 ? `${hrs}h` : `${hrs}h ${m}m`;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

/**
 * Highest-priority available backend for this host. Mirrors
 * `HwCapabilities::suggested` on the backend — priority order
 * VideoToolbox → NVENC → VAAPI → QSV → AMF, first available wins.
 */
function suggestedBackend(caps: HwCapabilities | null): HwBackend | 'none' {
  if (!caps) return 'none';
  const priority: HwBackend[] = ['videotoolbox', 'nvenc', 'vaapi', 'qsv', 'amf'];
  for (const p of priority) {
    if (caps.backends.some((b) => b.backend === p && b.state.status === 'available')) {
      return p;
    }
  }
  return 'none';
}

// ── Main page ────────────────────────────────────────────────────

export function PlaybackSettings() {
  const { config, updateField } = useSettingsContext();
  const { data: lastProbe, refetch: refetchProbe } = useQuery({
    queryKey: ['kino', 'playback', 'probe'],
    queryFn: async () => {
      const { data } = await probe();
      return data ?? null;
    },
    staleTime: Number.POSITIVE_INFINITY,
    // Probe result changes when a download flips the
    // ffmpeg_path or a revert restores system ffmpeg; the
    // download modal refetches explicitly on Done, but any
    // *other* tab / window that's open on this page needs
    // the event-driven path to stay in sync.
    meta: {
      invalidatedBy: ['ffmpeg_download_completed', 'ffmpeg_download_failed'],
    },
  });

  return (
    <div>
      <h1 className="text-xl font-bold mb-1">Playback</h1>
      <p className="text-sm text-[var(--text-muted)] mb-6">
        Transcoding, hardware acceleration, and streaming
      </p>

      <ActiveTranscodesCard />

      <EngineSection probe={lastProbe ?? null} onProbeRefetch={() => void refetchProbe()} />

      <TranscodingBehaviourSection
        probe={lastProbe ?? null}
        config={config}
        updateField={updateField}
        onProbeRefetch={() => void refetchProbe()}
      />

      <IntroCreditsSection config={config} updateField={updateField} />

      <CastSection config={config} updateField={updateField} />
    </div>
  );
}

// ── Engine section ───────────────────────────────────────────────

/**
 * The "what ffmpeg am I running, and is it OK?" panel. Combines
 * four previously scattered concerns: probe result, feature
 * checks, download/revert actions, binary-path override. Every
 * piece of info the user would ever need to troubleshoot a
 * playback issue starts here.
 */
function EngineSection({
  probe,
  onProbeRefetch,
}: {
  probe: HwCapabilities | null;
  onProbeRefetch: () => void;
}) {
  return (
    <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider">
          Engine
        </h2>
        <TestButton
          onTest={async () => {
            onProbeRefetch();
            return true;
          }}
          label="Re-probe"
        />
      </div>
      <FfmpegStatusCard probe={probe} onProbeRefetch={onProbeRefetch} />
    </section>
  );
}

/**
 * Consolidates the probe summary (version + Jellyfin flag +
 * feature badges + software codec list + HW backends) with the
 * two action paths the user might take (download a better
 * ffmpeg, or revert to system). One card, obvious state, clear
 * next step.
 */
function FfmpegStatusCard({
  probe,
  onProbeRefetch,
}: {
  probe: HwCapabilities | null;
  onProbeRefetch: () => void;
}) {
  if (!probe) {
    return (
      <div className="rounded-lg bg-white/5 ring-1 ring-white/10 p-3 text-xs text-[var(--text-muted)]">
        Probing FFmpeg…
      </div>
    );
  }
  const outOfDate =
    probe.ffmpeg_ok && typeof probe.ffmpeg_major === 'number' && probe.ffmpeg_major < 7;
  const missingFeature = probe.ffmpeg_ok && (!probe.has_libplacebo || !probe.has_libass);
  const needsAttention = !probe.ffmpeg_ok || outOfDate || missingFeature;

  return (
    <div
      className={cn(
        'rounded-lg ring-1 overflow-hidden',
        probe.ffmpeg_ok && !needsAttention
          ? 'bg-green-500/5 ring-green-500/20'
          : probe.ffmpeg_ok
            ? 'bg-amber-500/5 ring-amber-500/20'
            : 'bg-red-500/10 ring-red-500/20'
      )}
    >
      {/* Header: version + Jellyfin tag */}
      <div className="px-4 py-3 border-b border-white/5">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <p
              className={cn('text-sm font-medium', probe.ffmpeg_ok ? 'text-white' : 'text-red-200')}
            >
              {probe.ffmpeg_ok ? `FFmpeg ${probe.ffmpeg_major ?? '?'}.x` : 'FFmpeg not reachable'}
              {probe.is_jellyfin_build && (
                <span className="ml-2 text-[10px] uppercase tracking-wider text-green-300 bg-green-500/10 px-1.5 py-0.5 rounded">
                  Jellyfin build
                </span>
              )}
              {outOfDate && !probe.is_jellyfin_build && (
                <span className="ml-2 text-[10px] uppercase tracking-wider text-amber-300">
                  Out of date
                </span>
              )}
            </p>
            {probe.ffmpeg_version && (
              <p className="mt-0.5 font-mono text-[11px] text-[var(--text-muted)] truncate">
                {probe.ffmpeg_version}
              </p>
            )}
          </div>
        </div>
      </div>

      {/* Features + codecs */}
      {probe.ffmpeg_ok && (
        <div className="px-4 py-3 border-b border-white/5 space-y-2">
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[11px]">
            <span className="uppercase tracking-wider text-[10px] text-[var(--text-muted)]">
              Features
            </span>
            <FeatureBadge label="libplacebo" ok={probe.has_libplacebo} hint="HDR tone-mapping" />
            <FeatureBadge label="libass" ok={probe.has_libass} hint="styled subtitles" />
          </div>
          {probe.software_codecs.length > 0 && (
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
              <span className="uppercase tracking-wider text-[10px] text-[var(--text-muted)]">
                Software codecs
              </span>
              {probe.software_codecs.map((c) => (
                <span
                  key={c}
                  className="font-mono text-[10px] px-1.5 py-0.5 rounded bg-white/5 ring-1 ring-white/10 text-[var(--text-secondary)]"
                >
                  {c}
                </span>
              ))}
            </div>
          )}
          <HwBackendsRow probe={probe} />
        </div>
      )}

      {/* Action row */}
      <FfmpegActionRow probe={probe} onProbeRefetch={onProbeRefetch} />
    </div>
  );
}

function HwBackendsRow({ probe }: { probe: HwCapabilities }) {
  // Show only backends that are either currently working or
  // *could* work on this machine (NotCompiled / NotApplicable stay
  // hidden — there's nothing for the user to act on). This is the
  // same set the driver-hints block below operates on.
  const actionable: BackendStatus[] = probe.backends.filter(
    (b) => b.state.status === 'available' || b.state.status === 'unavailable'
  );
  if (actionable.length === 0) {
    return (
      <p className="text-[11px] text-[var(--text-muted)] italic">
        No hardware encoders detected on this host — all transcoding uses the CPU.
      </p>
    );
  }
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px]">
      <span className="uppercase tracking-wider text-[10px] text-[var(--text-muted)]">
        Hardware
      </span>
      {actionable.map((b) => {
        const on = b.state.status === 'available';
        return (
          <span
            key={b.backend}
            className={cn(
              'inline-flex items-center gap-1',
              on ? 'text-green-300' : 'text-amber-300'
            )}
          >
            {on ? <Check size={11} /> : <AlertTriangle size={11} />}
            {BACKEND_LABEL[b.backend]}
          </span>
        );
      })}
    </div>
  );
}

/**
 * The action row at the bottom of the FFmpeg status card. Two
 * mutually-exclusive states:
 * - System ffmpeg in use → [Download jellyfin-ffmpeg] button
 * - Bundled ffmpeg in use → version info + [Revert to system] link
 */
function FfmpegActionRow({
  probe,
  onProbeRefetch,
}: {
  probe: HwCapabilities;
  onProbeRefetch: () => void;
}) {
  const [showDownload, setShowDownload] = useState(false);
  const isBundled = probe.is_jellyfin_build;

  return (
    <>
      <div className="px-4 py-3 flex items-center justify-between gap-3 text-[11px]">
        {isBundled ? (
          <>
            <span className="text-[var(--text-secondary)]">
              Using bundled jellyfin-ffmpeg {probe.ffmpeg_major ?? ''}.x — all features enabled
            </span>
            <RevertButton onAfter={onProbeRefetch} />
          </>
        ) : (
          <>
            <span className="text-[var(--text-secondary)]">
              {probe.ffmpeg_ok
                ? 'Download a newer build for modern GPUs + HDR / ASS features.'
                : 'Download a self-contained ffmpeg build — no system install required.'}
            </span>
            <button
              type="button"
              onClick={() => setShowDownload(true)}
              className="inline-flex items-center gap-1.5 h-8 px-3 rounded-md bg-[var(--accent)]/15 hover:bg-[var(--accent)]/25 text-[var(--accent)] ring-1 ring-[var(--accent)]/30 text-[11px] font-medium transition"
            >
              <Download size={12} />
              Download jellyfin-ffmpeg
            </button>
          </>
        )}
      </div>
      {showDownload && (
        <DownloadModal
          onClose={() => setShowDownload(false)}
          onComplete={() => {
            setShowDownload(false);
            onProbeRefetch();
          }}
        />
      )}
    </>
  );
}

function RevertButton({ onAfter }: { onAfter: () => void }) {
  const qc = useQueryClient();
  const [showConfirm, setShowConfirm] = useState(false);
  const revertMutation = useMutation({
    mutationFn: async () => {
      await revertFfmpegToSystem();
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'playback', 'probe'] });
      setShowConfirm(false);
      onAfter();
    },
  });
  return (
    <>
      <button
        type="button"
        onClick={() => setShowConfirm(true)}
        disabled={revertMutation.isPending}
        className="inline-flex items-center gap-1 text-[var(--text-muted)] hover:text-white disabled:opacity-50 transition"
      >
        <RotateCcw size={11} />
        Revert to system
      </button>
      {showConfirm && (
        <ConfirmDialog
          title="Revert to system ffmpeg"
          body={
            <>
              The bundled jellyfin-ffmpeg in <span className="font-mono">/data/bin/</span> will be
              removed and kino will fall back to the system{' '}
              <span className="font-mono">ffmpeg</span> on <span className="font-mono">$PATH</span>.
              You can download the bundle again at any time.
            </>
          }
          confirmLabel="Revert"
          confirmVariant="danger"
          pending={revertMutation.isPending}
          error={revertMutation.isError ? (revertMutation.error as Error).message : null}
          onConfirm={() => revertMutation.mutate()}
          onCancel={() => setShowConfirm(false)}
        />
      )}
    </>
  );
}

/**
 * Lightweight confirmation modal matching the Download modal's
 * visual language. Used for destructive-ish actions (revert
 * ffmpeg) that used to live in a browser-native
 * `window.confirm` — those look out of place against the rest
 * of the app chrome.
 */
function ConfirmDialog({
  title,
  body,
  confirmLabel,
  confirmVariant = 'primary',
  pending,
  error,
  onConfirm,
  onCancel,
}: {
  title: string;
  body: React.ReactNode;
  confirmLabel: string;
  confirmVariant?: 'primary' | 'danger';
  pending: boolean;
  error: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="confirm-dialog-title"
      onClick={(e) => {
        if (e.target === e.currentTarget && !pending) onCancel();
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape' && !pending) onCancel();
      }}
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/85 backdrop-blur-sm p-4"
    >
      <div className="w-[min(460px,100%)] rounded-2xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-2xl overflow-hidden">
        <div className="px-5 py-4 border-b border-white/5">
          <h3 id="confirm-dialog-title" className="text-base font-semibold text-white">
            {title}
          </h3>
        </div>
        <div className="px-5 py-4 text-sm text-[var(--text-secondary)] leading-relaxed">{body}</div>
        {error && <div className="px-5 pb-3 text-xs text-red-300">{error}</div>}
        <div className="px-5 py-3 bg-black/20 flex items-center justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            disabled={pending}
            className="h-9 px-3 rounded-lg text-sm text-[var(--text-secondary)] hover:text-white disabled:opacity-50 transition"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={pending}
            className={cn(
              'h-9 px-4 rounded-lg font-semibold text-sm transition disabled:opacity-50',
              confirmVariant === 'danger'
                ? 'bg-red-500/90 hover:bg-red-500 text-white'
                : 'bg-[var(--accent)] hover:bg-[var(--accent)]/90 text-black'
            )}
          >
            {pending ? 'Working…' : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * User-facing download flow. The first screen is consent (source,
 * target, license disclosure); the second is live progress. One
 * modal, two visual states keyed on tracker status.
 */
function DownloadModal({ onClose, onComplete }: { onClose: () => void; onComplete: () => void }) {
  const qc = useQueryClient();
  // Poll the tracker while active — short interval during
  // download, stop when terminal state reached.
  const { data: state } = useQuery({
    queryKey: ['kino', 'playback', 'ffmpeg', 'download'],
    queryFn: async () => {
      const { data } = await getFfmpegDownload();
      return data ?? ({ status: 'idle' } satisfies FfmpegDownloadState);
    },
    refetchInterval: (q) => (q.state.data?.status === 'running' ? 500 : false),
  });

  const startMutation = useMutation({
    mutationFn: async () => {
      const { data } = await startFfmpegDownload();
      return data ?? null;
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'playback', 'ffmpeg', 'download'] });
    },
  });

  const cancelMutation = useMutation({
    mutationFn: async () => {
      await cancelFfmpegDownload();
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['kino', 'playback', 'ffmpeg', 'download'] });
    },
  });

  const status = state?.status ?? 'idle';
  const running = status === 'running';
  const completed = status === 'completed';
  const failed = status === 'failed';

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="ffmpeg-download-title"
      onClick={(e) => {
        if (e.target === e.currentTarget && !running) onClose();
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape' && !running) onClose();
      }}
      className="fixed inset-0 z-[60] flex items-center justify-center bg-black/85 backdrop-blur-sm p-4"
    >
      <div className="w-[min(520px,100%)] rounded-2xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-2xl overflow-hidden">
        <div className="px-5 py-4 border-b border-white/5">
          <h3 id="ffmpeg-download-title" className="text-base font-semibold text-white">
            Download jellyfin-ffmpeg
          </h3>
          <p className="mt-0.5 text-xs text-[var(--text-muted)]">
            Replaces the ffmpeg kino uses — your system ffmpeg is untouched.
          </p>
        </div>

        <div className="px-5 py-4 space-y-3 text-xs">
          <InfoRow label="Source">
            <a
              href="https://github.com/jellyfin/jellyfin-ffmpeg"
              target="_blank"
              rel="noreferrer"
              className="font-mono text-[var(--accent)] hover:underline"
            >
              github.com/jellyfin/jellyfin-ffmpeg
            </a>
            <span className="ml-2 text-[10px] uppercase tracking-wider text-[var(--text-muted)]">
              GPLv3
            </span>
          </InfoRow>
          <InfoRow label="Target">
            <span className="font-mono text-[var(--text-secondary)]">
              {completed && state?.status === 'completed' ? state.path : '/data/bin/ffmpeg'}
            </span>
          </InfoRow>
          <InfoRow label="Includes">
            <span className="text-[var(--text-secondary)]">
              libplacebo (HDR), libass (subtitles), NVENC + VAAPI + QSV + VideoToolbox + AMF
            </span>
          </InfoRow>

          {running && state?.status === 'running' && (
            <div className="pt-2">
              <div className="flex items-center justify-between text-[11px] text-[var(--text-muted)] mb-1 tabular-nums">
                <span>
                  {formatBytes(state.bytes)} / {formatBytes(state.total)}
                </span>
                <span>{state.total > 0 ? Math.round((state.bytes / state.total) * 100) : 0}%</span>
              </div>
              <div className="h-1.5 rounded-full bg-white/10 overflow-hidden">
                <div
                  className="h-full bg-[var(--accent)] rounded-full transition-[width]"
                  style={{
                    width: `${state.total > 0 ? (state.bytes / state.total) * 100 : 0}%`,
                  }}
                />
              </div>
            </div>
          )}

          {completed && state?.status === 'completed' && (
            <p className="mt-2 flex items-center gap-2 text-green-300">
              <Check size={14} /> Installed jellyfin-ffmpeg {state.version}
            </p>
          )}

          {failed && state?.status === 'failed' && (
            <p className="mt-2 flex items-start gap-2 text-red-300">
              <X size={14} className="mt-0.5 flex-shrink-0" />
              <span className="break-words">{state.reason}</span>
            </p>
          )}

          {startMutation.isError && (
            <p className="text-red-300">
              Couldn&apos;t start download: {(startMutation.error as Error).message}
            </p>
          )}
        </div>

        <div className="px-5 py-3 bg-black/20 flex items-center justify-end gap-2">
          {!running && !completed && (
            <button
              type="button"
              onClick={onClose}
              className="h-9 px-3 rounded-lg text-sm text-[var(--text-secondary)] hover:text-white transition"
            >
              Cancel
            </button>
          )}
          {running && (
            <button
              type="button"
              onClick={() => cancelMutation.mutate()}
              disabled={cancelMutation.isPending}
              className="h-9 px-3 rounded-lg text-sm text-[var(--text-secondary)] hover:text-white disabled:opacity-50 transition"
            >
              Cancel download
            </button>
          )}
          {(status === 'idle' || failed) && (
            <button
              type="button"
              onClick={() => startMutation.mutate()}
              disabled={startMutation.isPending}
              className="h-9 px-4 rounded-lg bg-[var(--accent)] text-black font-semibold text-sm hover:bg-[var(--accent)]/90 disabled:opacity-50 transition inline-flex items-center gap-2"
            >
              <Download size={14} />
              {failed ? 'Retry download' : 'Download'}
            </button>
          )}
          {completed && (
            <button
              type="button"
              onClick={onComplete}
              className="h-9 px-4 rounded-lg bg-[var(--accent)] text-black font-semibold text-sm hover:bg-[var(--accent)]/90 transition"
            >
              Done
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function InfoRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start gap-3">
      <span className="w-20 flex-shrink-0 uppercase tracking-wider text-[10px] text-[var(--text-muted)] pt-0.5">
        {label}
      </span>
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}

function FeatureBadge({ label, ok, hint }: { label: string; ok: boolean; hint: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 font-mono',
        ok ? 'text-green-300' : 'text-amber-300'
      )}
      title={hint}
    >
      {ok ? <Check size={11} /> : <X size={11} />}
      {label}
    </span>
  );
}

// ── Transcoding behaviour section ────────────────────────────────

/**
 * The "knobs" section — everything about how the transcoder
 * behaves when it runs. Split from Engine so the page reads
 * "is my engine healthy?" → "how should it behave?" rather than
 * mixing health + configuration into one slab.
 */
function TranscodingBehaviourSection({
  probe,
  config,
  updateField,
  onProbeRefetch,
}: {
  probe: HwCapabilities | null;
  config: Record<string, unknown>;
  updateField: (key: string, value: unknown) => void;
  onProbeRefetch: () => void;
}) {
  const [lastTranscode, setLastTranscode] = useState<TestTranscodeResult | null>(null);
  const runTestTranscode = async (): Promise<boolean> => {
    try {
      const { data } = await testTranscode();
      if (!data) return false;
      setLastTranscode(data);
      return Boolean(data.ok);
    } catch {
      return false;
    }
  };

  const applySuggestedAccel = async () => {
    let p: HwCapabilities | null = probe;
    if (!p) {
      onProbeRefetch();
      p = probe;
    }
    updateField('hw_acceleration', suggestedBackend(p));
  };

  const unavailableBackends: BackendStatus[] =
    probe?.backends.filter((b) => b.state.status === 'unavailable') ?? [];
  const hasHints = unavailableBackends.length > 0;

  return (
    <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider">
          Transcoding
        </h2>
        <TestButton onTest={runTestTranscode} label="Test transcode" />
      </div>

      {lastTranscode && (
        <div
          className={cn(
            'rounded-lg p-3 mb-3 ring-1 text-xs',
            lastTranscode.ok
              ? 'bg-green-500/5 ring-green-500/20 text-[var(--text-secondary)]'
              : 'bg-red-500/10 ring-red-500/20 text-red-300'
          )}
        >
          <p className="font-medium text-white">{lastTranscode.message}</p>
          <p className="mt-1 text-[var(--text-muted)]">
            {hwAccelLabel(lastTranscode.hw_acceleration)} · {lastTranscode.duration_ms}ms
          </p>
          {lastTranscode.stderr_tail && (
            <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-all font-mono text-[10px] leading-snug text-red-200/80 bg-black/30 rounded p-2 ring-1 ring-red-500/10">
              {lastTranscode.stderr_tail}
            </pre>
          )}
        </div>
      )}

      <FormField
        label="Enabled"
        description="Allow on-the-fly transcoding for clients that can't direct-play"
        help="Disable only if every client you use supports direct play. Leaving it on is safe — kino skips transcoding when the source is already compatible."
      >
        <Toggle
          checked={Boolean(config.transcoding_enabled)}
          onChange={(v) => updateField('transcoding_enabled', v)}
        />
      </FormField>
      <FormField
        label="Hardware acceleration"
        description="Offload encoding to GPU / iGPU"
        help="Pick a backend that matches your machine. The Engine panel above shows which ones the probe detected; Detect uses that result to auto-select."
      >
        <div className="flex items-center gap-2">
          <SelectInput
            value={String(config.hw_acceleration ?? 'none')}
            onChange={(v) => updateField('hw_acceleration', v)}
            options={HW_OPTIONS}
          />
          <button
            type="button"
            onClick={applySuggestedAccel}
            title={
              probe
                ? `Detected: ${hwAccelLabel(suggestedBackend(probe))}`
                : 'Probing first, then click to auto-select'
            }
            className="inline-flex items-center gap-1.5 h-9 px-3 rounded-lg bg-white/5 hover:bg-white/10 text-sm text-[var(--text-secondary)] hover:text-white ring-1 ring-white/10 transition"
          >
            <Sparkles size={14} />
            Detect
          </button>
          {probe && suggestedBackend(probe) !== (config.hw_acceleration ?? 'none') && (
            <span className="text-xs text-amber-400">
              Suggested: {hwAccelLabel(suggestedBackend(probe))}
            </span>
          )}
        </div>
        {hasHints && (
          <div className="mt-3 rounded-lg bg-amber-500/5 ring-1 ring-amber-500/20 p-3">
            <p className="text-xs font-medium text-amber-300 mb-2">
              GPU backends compiled in but not running — install drivers to enable hardware
              acceleration:
            </p>
            <ul className="space-y-1.5">
              {unavailableBackends.map((b) => {
                const hint = b.state.status === 'unavailable' ? b.state.hint : '';
                return (
                  <li
                    key={b.backend}
                    className="flex items-start gap-2 text-[11px] leading-snug text-[var(--text-secondary)]"
                  >
                    <Lightbulb size={12} className="mt-0.5 flex-shrink-0 text-amber-400/80" />
                    <span>
                      <span className="font-mono font-semibold text-amber-200/90">
                        {BACKEND_LABEL[b.backend]}:
                      </span>{' '}
                      {hint}
                    </span>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </FormField>
      <FormField
        label="Max concurrent"
        description="Session cap — protects CPU / disk"
        help="New transcode requests get rejected when this many are already running. 2 is safe for CPU-only; 4–8 is fine with hardware acceleration."
      >
        <NumberInput
          value={Number(config.max_concurrent_transcodes ?? 2)}
          onChange={(v) => updateField('max_concurrent_transcodes', v)}
          min={1}
          max={32}
          suffix="sessions"
        />
      </FormField>
      <FormField
        label="FFmpeg path (advanced)"
        description="Override auto-detected binary"
        help="Leave blank to let kino pick: bundled if present, otherwise $PATH. Use an absolute path to a custom build if you need specific codecs. The Engine panel above reflects whatever is actually in use."
      >
        <TextInput
          value={String(config.ffmpeg_path ?? '')}
          onChange={(v) => updateField('ffmpeg_path', v)}
          placeholder="(auto)"
        />
      </FormField>
    </section>
  );
}

// ── Intro & credits section ──────────────────────────────────────

function IntroCreditsSection({
  config,
  updateField,
}: {
  config: Record<string, unknown>;
  updateField: (key: string, value: unknown) => void;
}) {
  return (
    <section className="space-y-1 pb-6 border-b border-white/5 mb-6">
      <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
        Intro &amp; Credits
      </h2>
      <p className="text-xs text-[var(--text-muted)] mb-4 max-w-xl">
        Kino compares audio fingerprints across episodes of a season to find shared intros and
        credits, then shows a Skip button at the right moment. Analysis runs in the background after
        import; you won&apos;t see buttons until a second episode of the same season has landed.
      </p>
      <FormField
        label="Detect intros"
        description="Fingerprint each new season to find the shared opening sequence"
      >
        <Toggle
          checked={Boolean(config.intro_detect_enabled ?? true)}
          onChange={(v) => updateField('intro_detect_enabled', v)}
        />
      </FormField>
      <FormField label="Detect credits" description="Same treatment for end-of-episode credits">
        <Toggle
          checked={Boolean(config.credits_detect_enabled ?? true)}
          onChange={(v) => updateField('credits_detect_enabled', v)}
        />
      </FormField>
      <FormField
        label="Auto-skip intros"
        description="Smart = show the button on the first episode of a season, auto-skip the rest"
      >
        <select
          value={String(config.auto_skip_intros ?? 'smart')}
          onChange={(e) => updateField('auto_skip_intros', e.target.value)}
          className="h-9 px-2 text-sm rounded-md bg-[var(--bg-card)] ring-1 ring-white/10 text-white focus:outline-none focus:ring-[var(--accent)]/40"
        >
          <option value="off">Off</option>
          <option value="on">Always</option>
          <option value="smart">Smart</option>
        </select>
      </FormField>
      <FormField
        label="Auto-skip credits"
        description="Skip straight to the end of the episode when credits start"
      >
        <Toggle
          checked={Boolean(config.auto_skip_credits ?? false)}
          onChange={(v) => updateField('auto_skip_credits', v)}
        />
      </FormField>
      <FormField
        label="Minimum intro length"
        description="Don't show Skip for intros shorter than this"
      >
        <NumberInput
          value={Number(config.intro_min_length_s ?? 15)}
          onChange={(v) => updateField('intro_min_length_s', v)}
          min={5}
          max={60}
          suffix="seconds"
        />
      </FormField>
      <FormField
        label="Max concurrent analyses"
        description="Shared with trickplay generation; playback transcoding has its own budget"
      >
        <NumberInput
          value={Number(config.max_concurrent_intro_analyses ?? 2)}
          onChange={(v) => updateField('max_concurrent_intro_analyses', v)}
          min={1}
          max={8}
          suffix="jobs"
        />
      </FormField>
    </section>
  );
}

// ── Cast section ─────────────────────────────────────────────────

function CastSection({
  config,
  updateField,
}: {
  config: Record<string, unknown>;
  updateField: (key: string, value: unknown) => void;
}) {
  return (
    <section className="space-y-1">
      <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
        Cast
      </h2>
      <FormField
        label="Receiver App ID"
        description="Custom Google Cast receiver — optional"
        help="Leave empty to use the default kino receiver. Only set this if you've published your own custom Chromecast receiver app via the Google Cast Developer Console."
      >
        <TextInput
          value={String(config.cast_receiver_app_id ?? '')}
          onChange={(v) => updateField('cast_receiver_app_id', v)}
          placeholder="CC1AD845"
        />
      </FormField>
    </section>
  );
}

// ── Active transcodes card ───────────────────────────────────────

function ActiveTranscodesCard() {
  const queryClient = useQueryClient();
  const { data: stats } = useQuery({
    queryKey: ['kino', 'playback', 'transcode-stats'],
    queryFn: async () => {
      const { data } = await transcodeStats();
      return data ?? null;
    },
    refetchInterval: 3000,
  });
  const { data: sessions } = useQuery({
    queryKey: ['kino', 'playback', 'transcode-sessions'],
    queryFn: async () => {
      const { data } = await transcodeSessions();
      return data ?? [];
    },
    refetchInterval: 3000,
  });

  const active = stats?.active_sessions ?? 0;
  const max = stats?.max_concurrent ?? 0;
  const full = max > 0 && active >= max;
  const list = sessions ?? [];

  const handleStop = async (sessionId: string) => {
    try {
      await stopTranscodeSession({ path: { session_id: sessionId } });
    } finally {
      queryClient.invalidateQueries({ queryKey: ['kino', 'playback', 'transcode-sessions'] });
      queryClient.invalidateQueries({ queryKey: ['kino', 'playback', 'transcode-stats'] });
    }
  };

  return (
    <div className="rounded-lg border border-white/10 bg-[var(--bg-card)]/40 p-4 mb-6">
      <div className="flex items-center gap-4">
        <Activity
          size={14}
          className={cn(
            full ? 'text-amber-400' : active > 0 ? 'text-sky-400' : 'text-[var(--text-muted)]'
          )}
        />
        <div>
          <p className="text-xs text-[var(--text-muted)]">Active transcodes</p>
          <p className="text-sm font-semibold text-white font-mono">
            {active} / {max}
          </p>
        </div>
        {stats?.enabled === false && (
          <span className="text-xs text-[var(--text-muted)] italic ml-auto">
            Transcoding disabled
          </span>
        )}
      </div>

      {list.length > 0 && (
        <ul className="mt-3 divide-y divide-white/5 border-t border-white/5 -mx-4 -mb-4">
          {list.map((s) => (
            <li
              key={s.session_id}
              className="flex items-center gap-3 px-4 py-2 text-xs hover:bg-white/[0.02]"
            >
              <span className="flex-1 truncate text-[var(--text-secondary)]">
                {s.title ?? `Media #${s.media_id}`}
              </span>
              <span className="text-[var(--text-muted)] tabular-nums">
                {formatDuration(s.started_at_secs_ago)}
              </span>
              <span
                className={cn(
                  'tabular-nums',
                  s.idle_secs > 30 ? 'text-amber-400' : 'text-[var(--text-muted)]'
                )}
                title="Seconds since the client last fetched a segment"
              >
                idle {formatDuration(s.idle_secs)}
              </span>
              <button
                type="button"
                onClick={() => handleStop(s.session_id)}
                title="Stop this transcode session"
                className="inline-flex items-center gap-1 px-2 py-1 rounded bg-white/5 hover:bg-red-500/15 hover:text-red-300 text-[var(--text-muted)] ring-1 ring-white/10 transition"
              >
                <Square size={10} className="fill-current" />
                Stop
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
