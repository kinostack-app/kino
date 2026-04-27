import { useQuery } from '@tanstack/react-query';
import { Activity, Download as DownloadIcon, Upload as UploadIcon } from 'lucide-react';
import { useMemo, useState } from 'react';
import { listDownloads, speedTest } from '@/api/generated/sdk.gen';
import {
  DurationInput,
  FormField,
  NumberInput,
  SpeedInput,
  TestButton,
} from '@/components/settings/FormField';
import { useSettingsContext } from './SettingsLayout';

// ── Stats helpers ─────────────────────────────────────────────────

function formatBps(bytesPerSec: number): string {
  if (bytesPerSec <= 0) return '0 B/s';
  const units = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
  let i = 0;
  let val = bytesPerSec;
  while (val >= 1024 && i < units.length - 1) {
    val /= 1024;
    i += 1;
  }
  return `${val.toFixed(val >= 100 ? 0 : 1)} ${units[i]}`;
}

function DownloadStatsCard() {
  // No polling — key prefix-matches DOWNLOADS_KEY so the central WS
  // handler invalidates it on every download lifecycle + progress
  // event. See `state/websocket.ts`.
  const { data } = useQuery({
    queryKey: ['kino', 'downloads', 'stats'],
    queryFn: async () => {
      const { data } = await listDownloads();
      return data?.results ?? [];
    },
  });

  const stats = useMemo(() => {
    const rows = data ?? [];
    const active = rows.filter((r) => r.state === 'downloading');
    const stalled = rows.filter((r) => r.state === 'stalled');
    const queued = rows.filter((r) => r.state === 'queued' || r.state === 'grabbing');
    const seeding = rows.filter((r) => r.state === 'seeding');
    const totalDl = active.reduce((a, r) => a + r.download_speed, 0);
    const totalUl = rows.reduce((a, r) => a + r.upload_speed, 0);
    return {
      active: active.length,
      stalled: stalled.length,
      queued: queued.length,
      seeding: seeding.length,
      totalDl,
      totalUl,
    };
  }, [data]);

  return (
    <div className="rounded-lg border border-white/10 bg-[var(--bg-card)]/40 p-4 mb-6">
      <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
        <div className="flex items-center gap-2">
          <DownloadIcon size={14} className="text-sky-400" />
          <div>
            <p className="text-xs text-[var(--text-muted)]">Down</p>
            <p className="text-sm font-semibold text-white font-mono">{formatBps(stats.totalDl)}</p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <UploadIcon size={14} className="text-green-400" />
          <div>
            <p className="text-xs text-[var(--text-muted)]">Up</p>
            <p className="text-sm font-semibold text-white font-mono">{formatBps(stats.totalUl)}</p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Activity size={14} className="text-[var(--text-muted)]" />
          <div>
            <p className="text-xs text-[var(--text-muted)]">Active</p>
            <p className="text-sm font-semibold text-white font-mono">{stats.active}</p>
          </div>
        </div>
        {stats.stalled > 0 && (
          <div>
            <p className="text-xs text-[var(--text-muted)]">Stalled</p>
            <p className="text-sm font-semibold text-amber-300 font-mono">{stats.stalled}</p>
          </div>
        )}
        <div>
          <p className="text-xs text-[var(--text-muted)]">Queued</p>
          <p className="text-sm font-semibold text-white font-mono">{stats.queued}</p>
        </div>
        <div>
          <p className="text-xs text-[var(--text-muted)]">Seeding</p>
          <p className="text-sm font-semibold text-white font-mono">{stats.seeding}</p>
        </div>
      </div>
    </div>
  );
}

// ── Main page ─────────────────────────────────────────────────────

export function DownloadSettings() {
  const { config, updateField } = useSettingsContext();

  const stallTimeout = Number(config.stall_timeout ?? 30);
  const deadTimeout = Number(config.dead_timeout ?? 60);
  const ratio = Number(config.seed_ratio_limit ?? 1.0);

  // Bad invariant: a "dead" threshold lower than "stalled" means the
  // download can never be stalled before it's already considered dead.
  const timeoutError = deadTimeout < stallTimeout ? 'Dead timeout must be ≥ Stall timeout' : '';

  const [speedTestBps, setSpeedTestBps] = useState<number | null>(null);
  const [speedTestMsg, setSpeedTestMsg] = useState<string | null>(null);
  const runSpeedTest = async (): Promise<boolean> => {
    try {
      const { data } = await speedTest();
      if (data?.ok) {
        setSpeedTestBps(data.bytes_per_sec ?? null);
        setSpeedTestMsg(data.message ?? null);
        return true;
      }
      setSpeedTestBps(null);
      setSpeedTestMsg(data?.message ?? 'failed');
      return false;
    } catch (e) {
      setSpeedTestMsg(e instanceof Error ? e.message : 'failed');
      return false;
    }
  };

  return (
    <div>
      <h1 className="text-xl font-bold mb-1">Downloads</h1>
      <p className="text-sm text-[var(--text-muted)] mb-6">Torrent client settings and limits</p>

      <DownloadStatsCard />

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          Queue
        </h2>
        <FormField
          label="Max Concurrent"
          description="Simultaneous downloads"
          help="Higher values download more at once but share bandwidth. 3 is a good default for most home connections."
        >
          <NumberInput
            value={Number(config.max_concurrent_downloads ?? 3)}
            onChange={(v) => updateField('max_concurrent_downloads', v)}
            min={1}
            max={20}
            suffix="torrents"
          />
        </FormField>
      </section>

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider">
            Speed Limits
          </h2>
          <div className="flex items-center gap-3">
            {speedTestBps !== null && (
              <span className="text-xs text-[var(--text-muted)]">
                Last test: <span className="font-mono text-white">{formatBps(speedTestBps)}</span>
              </span>
            )}
            <TestButton onTest={runSpeedTest} label="Run speed test" />
          </div>
        </div>
        {speedTestMsg && !speedTestBps && (
          <p className="text-xs text-red-400 mb-3">{speedTestMsg}</p>
        )}
        <FormField
          label="Download Limit"
          description="Cap total download speed"
          help="Caps the combined download speed across all active torrents. Leave empty for unlimited. Use the speed test above to gauge your connection first."
        >
          <SpeedInput
            value={Number(config.download_speed_limit ?? 0)}
            onChange={(v) => updateField('download_speed_limit', v)}
          />
        </FormField>
        <FormField
          label="Upload Limit"
          description="Cap total upload speed"
          help="Respect your ISP upload cap — unlimited uploads during downloads can starve the TCP ACK path and slow everything down."
        >
          <SpeedInput
            value={Number(config.upload_speed_limit ?? 0)}
            onChange={(v) => updateField('upload_speed_limit', v)}
          />
        </FormField>
      </section>

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          Seeding
        </h2>
        <p className="text-xs text-[var(--text-muted)] mb-3">
          Seeding stops when <em>either</em> the ratio or the time limit is reached — whichever
          comes first.
        </p>
        <FormField
          label="Ratio Limit"
          description="Uploaded ÷ downloaded"
          help="1.0 means you've uploaded as much as you downloaded. 1.0+ is good tracker etiquette and keeps swarms healthy."
        >
          <NumberInput
            value={ratio}
            onChange={(v) => updateField('seed_ratio_limit', v)}
            min={0}
            max={100}
            step={0.1}
            suffix="×"
          />
        </FormField>
        <FormField
          label="Time Limit"
          description="0 = no time limit"
          help="Stop seeding after this duration regardless of ratio. Useful on private trackers that require minimum seed times."
        >
          <DurationInput
            value={Number(config.seed_time_limit ?? 0)}
            onChange={(v) => updateField('seed_time_limit', v)}
            unit="minutes"
          />
        </FormField>
      </section>

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          Quality
        </h2>
        <FormField
          label="Auto-upgrade"
          description="Re-search every 7 days for a better release"
          help="When on, kino periodically checks indexers for a higher-quality release of titles you've already downloaded. Triggers grab + re-import only when the new release beats the existing one by enough margin (per the quality profile cutoff). Watched movies and watched episodes are skipped."
        >
          <label className="inline-flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={Boolean(config.auto_upgrade_enabled ?? true)}
              onChange={(e) => updateField('auto_upgrade_enabled', e.target.checked)}
              className="accent-[var(--accent)] cursor-pointer h-4 w-4"
            />
            <span className="text-sm text-[var(--text-secondary)]">
              {(config.auto_upgrade_enabled ?? true) ? 'On' : 'Off'}
            </span>
          </label>
        </FormField>
      </section>

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          Disk Space
        </h2>
        <FormField
          label="Low-space warning"
          description="Surfaced in /status + the health banner"
          help="When free space at the download path drops below this, the health banner shows a 'low free space' warning. Doesn't block grabs — those are rejected separately when the release size + 2 GB buffer wouldn't fit."
        >
          <NumberInput
            value={Number(config.low_disk_threshold_gb ?? 5)}
            onChange={(v) => updateField('low_disk_threshold_gb', v)}
            min={1}
            max={1024}
            suffix="GB"
          />
        </FormField>
      </section>

      <section className="space-y-1">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          Stall Detection
        </h2>
        <FormField
          label="Stall Timeout"
          description="No progress = stalled"
          help="How long a download must go without progress before it's marked stalled. Stalled torrents re-announce to trackers to find fresh peers."
        >
          <DurationInput
            value={stallTimeout}
            onChange={(v) => updateField('stall_timeout', v)}
            unit="minutes"
            min={5}
          />
        </FormField>
        <FormField
          label="Dead Timeout"
          description="Stalled too long = failed"
          help="Must be ≥ stall timeout. Failed downloads get blocklisted so the same release isn't picked again, and a fresh release is searched."
          error={timeoutError}
        >
          <DurationInput
            value={deadTimeout}
            onChange={(v) => updateField('dead_timeout', v)}
            unit="minutes"
            min={10}
          />
        </FormField>
      </section>
    </div>
  );
}
