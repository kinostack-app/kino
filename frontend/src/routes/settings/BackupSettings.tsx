/**
 * Settings → Backup. Subsystem 19.
 *
 * Two surfaces stitched together: schedule controls (driven by the
 * shared `useConfigEditor` SaveBar pattern, like the other settings
 * pages) plus a backups table that mutates outside the editor (each
 * action is a one-shot HTTP call).
 *
 * The schedule field is a preset (`daily` / `weekly` / `monthly` /
 * `off`); cron is reserved for a future advanced-mode toggle. We
 * deliberately don't expose cron in v1 — most users want
 * "daily at 03:00", not a 5-field expression.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Archive, Download, RotateCw, Trash2, TriangleAlert, Upload } from 'lucide-react';
import { useRef, useState } from 'react';
import {
  createBackupMutation,
  deleteBackupMutation,
  listBackupsOptions,
  listBackupsQueryKey,
  restoreBackupMutation,
} from '@/api/generated/@tanstack/react-query.gen';
import { client } from '@/api/generated/client.gen';
import type { Backup } from '@/api/generated/types.gen';
import { kinoToast } from '@/components/kino-toast';
import { cn } from '@/lib/utils';
import { useSettingsContext } from './SettingsLayout';

export function BackupSettings() {
  const { config, updateField } = useSettingsContext();
  const queryClient = useQueryClient();
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [restoreTarget, setRestoreTarget] = useState<Backup | null>(null);
  const [uploading, setUploading] = useState(false);

  const { data: backups, isLoading } = useQuery({
    ...listBackupsOptions(),
    meta: { invalidatedBy: ['backup_created', 'backup_deleted', 'backup_restored'] },
  });

  const createBackup = useMutation({
    ...createBackupMutation(),
    onSuccess: (row) => {
      kinoToast.success('Backup created', {
        id: 'backup-created',
        description: `${formatBytes(row.size_bytes)} · ${row.filename}`,
      });
    },
    onError: (err) => {
      kinoToast.failure({
        title: 'Backup failed',
        error: err instanceof Error ? err.message : String(err),
        downloadId: undefined,
        movieId: undefined,
        episodeId: undefined,
        showId: undefined,
      });
    },
  });

  const deleteBackup = useMutation({
    ...deleteBackupMutation(),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: listBackupsQueryKey() });
    },
  });

  const restoreBackup = useMutation({
    ...restoreBackupMutation(),
    onSuccess: () => {
      kinoToast.warning('Restore staged', {
        id: 'backup-restored',
        description:
          'Restart kino to load the restored database. The next process boot picks up the new state automatically.',
        // Sticky — restore needs explicit acknowledgement.
        duration: Infinity,
      });
      setRestoreTarget(null);
    },
  });

  const onUpload = async (file: File) => {
    setUploading(true);
    try {
      const form = new FormData();
      form.append('archive', file);
      const baseUrl = (typeof client.getConfig === 'function' && client.getConfig().baseUrl) || '';
      const res = await fetch(`${baseUrl}/api/v1/backups/restore-upload`, {
        method: 'POST',
        body: form,
        credentials: 'include',
      });
      if (!res.ok) {
        throw new Error(await res.text().catch(() => `${res.status}`));
      }
      kinoToast.warning('Restore staged', {
        id: 'backup-restored',
        description:
          'Restart kino to load the uploaded backup. The next process boot picks up the new state automatically.',
        duration: Infinity,
      });
    } catch (err) {
      kinoToast.failure({
        title: 'Restore failed',
        error: err instanceof Error ? err.message : String(err),
        downloadId: undefined,
        movieId: undefined,
        episodeId: undefined,
        showId: undefined,
      });
    } finally {
      setUploading(false);
      if (fileInputRef.current) fileInputRef.current.value = '';
    }
  };

  if (!config) {
    return null;
  }

  // Editor's config is typed as Record<string, unknown> — narrow at
  // the boundary, same pattern the other settings pages use.
  const backupEnabled = Boolean(config.backup_enabled ?? true);
  const backupSchedule = String(config.backup_schedule ?? 'daily');
  const backupTime = String(config.backup_time ?? '03:00');
  const backupLocation = String(config.backup_location_path ?? '');
  const backupRetention = Number(config.backup_retention_count ?? 7);

  const lastBackupAt = backups && backups.length > 0 ? formatRelative(backups[0].created_at) : null;

  return (
    <div className="space-y-8">
      <header>
        <h1 className="text-xl font-semibold mb-1">Backup &amp; restore</h1>
        <p className="text-sm text-[var(--text-muted)]">
          Backs up your settings + library metadata.{' '}
          <strong className="text-[var(--text-secondary)]">
            Your media files on disk aren&apos;t included
          </strong>{' '}
          — back those up separately.
        </p>
      </header>

      {/* Top bar: status + primary actions */}
      <section className="rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/5 p-4">
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex-1 min-w-0">
            <p className="text-[11px] uppercase tracking-wider text-[var(--text-muted)]">Status</p>
            <p className="text-sm text-white">
              {backupEnabled ? `Daily backup at ${backupTime}` : 'Backups disabled'}
              {lastBackupAt && (
                <>
                  <span className="text-[var(--text-muted)]"> · last ran </span>
                  {lastBackupAt}
                </>
              )}
              <span className="text-[var(--text-muted)]">
                {' · keep '}
                {backupRetention} most recent
              </span>
            </p>
          </div>
          <button
            type="button"
            onClick={() => createBackup.mutate({})}
            disabled={createBackup.isPending}
            className="inline-flex items-center gap-2 rounded-md bg-[var(--accent)] hover:bg-[var(--accent)]/90 px-3 py-2 text-sm font-medium text-white disabled:opacity-50"
          >
            <Archive size={14} />
            {createBackup.isPending ? 'Creating…' : 'Create backup now'}
          </button>
          <button
            type="button"
            onClick={() => fileInputRef.current?.click()}
            disabled={uploading}
            className="inline-flex items-center gap-2 rounded-md bg-white/5 hover:bg-white/10 px-3 py-2 text-sm text-[var(--text-secondary)] hover:text-white disabled:opacity-50"
          >
            <Upload size={14} />
            {uploading ? 'Uploading…' : 'Restore from file'}
          </button>
          <input
            ref={fileInputRef}
            type="file"
            accept=".tar.gz,.tgz,application/gzip"
            className="hidden"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void onUpload(f);
            }}
          />
        </div>
      </section>

      {/* Schedule */}
      <section className="rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/5 p-4 space-y-4">
        <h2 className="text-sm font-semibold text-white">Schedule</h2>
        <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
          <Field label="Frequency">
            <select
              value={backupSchedule}
              onChange={(e) => updateField('backup_schedule', e.target.value)}
              className="w-full rounded-md bg-black/40 ring-1 ring-white/10 px-3 py-2 text-sm text-white"
            >
              <option value="daily">Daily</option>
              <option value="weekly">Weekly</option>
              <option value="monthly">Monthly</option>
              <option value="off">Off</option>
            </select>
          </Field>
          <Field label="Time">
            <input
              type="time"
              value={backupTime}
              onChange={(e) => updateField('backup_time', e.target.value)}
              disabled={backupSchedule === 'off'}
              className="w-full rounded-md bg-black/40 ring-1 ring-white/10 px-3 py-2 text-sm text-white disabled:opacity-50"
            />
          </Field>
          <Field label="Keep">
            <input
              type="number"
              min={1}
              max={365}
              value={backupRetention}
              onChange={(e) => updateField('backup_retention_count', Number(e.target.value))}
              className="w-full rounded-md bg-black/40 ring-1 ring-white/10 px-3 py-2 text-sm text-white"
            />
          </Field>
          <Field label="Enabled">
            <label className="flex items-center gap-2 mt-2">
              <input
                type="checkbox"
                checked={backupEnabled}
                onChange={(e) => updateField('backup_enabled', e.target.checked)}
                className="accent-[var(--accent)]"
              />
              <span className="text-sm text-[var(--text-secondary)]">Run scheduled backups</span>
            </label>
          </Field>
        </div>
        <Field label="Location">
          <input
            type="text"
            value={backupLocation}
            onChange={(e) => updateField('backup_location_path', e.target.value)}
            placeholder="/var/lib/kino/backups"
            className="w-full rounded-md bg-black/40 ring-1 ring-white/10 px-3 py-2 text-sm text-white font-mono"
          />
          <p className="text-[11px] text-[var(--text-muted)] mt-1">
            Absolute path. Defaults to <code>{'{data_path}/backups'}</code>. Use a different drive
            (NAS mount, external SSD) if you want backups off the kino host.
          </p>
        </Field>
      </section>

      {/* Backups list */}
      <section>
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-semibold text-white">Backups</h2>
          {backups && backups.length > 0 && (
            <p className="text-xs text-[var(--text-muted)]">
              {backups.length} backup{backups.length === 1 ? '' : 's'}
            </p>
          )}
        </div>
        {isLoading ? (
          <p className="text-sm text-[var(--text-muted)]">Loading…</p>
        ) : !backups || backups.length === 0 ? (
          <div className="rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/5 p-6 text-center">
            <Archive size={20} className="text-[var(--text-muted)] mx-auto mb-2" />
            <p className="text-sm text-[var(--text-secondary)]">No backups yet.</p>
            <p className="text-xs text-[var(--text-muted)] mt-1">
              Your first scheduled run is at {backupTime}, or create one now.
            </p>
          </div>
        ) : (
          <ul className="divide-y divide-white/5 rounded-xl overflow-hidden ring-1 ring-white/5">
            {backups.map((b) => (
              <li key={b.id} className="flex items-center gap-3 bg-[var(--bg-secondary)] px-4 py-3">
                <KindBadge kind={b.kind} />
                <div className="flex-1 min-w-0">
                  <p className="text-sm text-white">{formatTimestamp(b.created_at)}</p>
                  <p className="text-[11px] text-[var(--text-muted)] truncate">
                    {b.filename} · {formatBytes(b.size_bytes)} · v{b.kino_version}
                  </p>
                </div>
                <a
                  href={`/api/v1/backups/${b.id}/download`}
                  title="Download archive"
                  className="p-2 rounded-md hover:bg-white/5 text-[var(--text-secondary)] hover:text-white"
                >
                  <Download size={14} />
                </a>
                <button
                  type="button"
                  onClick={() => setRestoreTarget(b)}
                  title="Restore from this backup"
                  className="p-2 rounded-md hover:bg-white/5 text-[var(--text-secondary)] hover:text-white"
                >
                  <RotateCw size={14} />
                </button>
                <button
                  type="button"
                  onClick={() => {
                    if (
                      confirm(
                        b.kind === 'pre_restore'
                          ? 'This is a recovery point. Delete anyway?'
                          : `Delete backup from ${formatTimestamp(b.created_at)}?`
                      )
                    ) {
                      deleteBackup.mutate({ path: { id: b.id } });
                    }
                  }}
                  title="Delete backup"
                  className="p-2 rounded-md hover:bg-red-500/10 text-[var(--text-muted)] hover:text-red-400"
                >
                  <Trash2 size={14} />
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {/* Restore confirm modal */}
      {restoreTarget && (
        <RestoreModal
          backup={restoreTarget}
          onCancel={() => setRestoreTarget(null)}
          onConfirm={() => restoreBackup.mutate({ path: { id: restoreTarget.id } })}
          submitting={restoreBackup.isPending}
        />
      )}
    </div>
  );
}

// ─── Subcomponents ──────────────────────────────────────────────

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    // biome-ignore lint/a11y/noLabelWithoutControl: generic wrapper — the actual form control is passed via children at every call site (see all Field usages above; each renders an <input> or <select>). Biome can't statically prove the children include a control
    <label className="block">
      <span className="block text-[11px] uppercase tracking-wider text-[var(--text-muted)] mb-1">
        {label}
      </span>
      {children}
    </label>
  );
}

function KindBadge({ kind }: { kind: string }) {
  const styles: Record<string, { label: string; className: string }> = {
    manual: {
      label: 'Manual',
      className: 'bg-white/10 text-[var(--text-secondary)]',
    },
    scheduled: {
      label: 'Scheduled',
      className: 'bg-blue-500/15 text-blue-300',
    },
    pre_restore: {
      label: 'Pre-restore',
      className: 'bg-amber-500/15 text-amber-300',
    },
  };
  const meta = styles[kind] ?? styles.manual;
  return (
    <span
      className={cn(
        'inline-flex items-center px-2 py-0.5 rounded text-[10px] font-semibold uppercase tracking-wider',
        meta.className
      )}
    >
      {meta.label}
    </span>
  );
}

function RestoreModal({
  backup,
  onCancel,
  onConfirm,
  submitting,
}: {
  backup: Backup;
  onCancel: () => void;
  onConfirm: () => void;
  submitting: boolean;
}) {
  return (
    <div className="fixed inset-0 z-[80] grid place-items-center bg-black/60 backdrop-blur-sm">
      <div className="w-full max-w-md rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-2xl p-5">
        <div className="flex items-start gap-3 mb-3">
          <TriangleAlert size={20} className="text-amber-300 flex-shrink-0 mt-0.5" />
          <div>
            <h2 className="text-base font-semibold text-white">
              Restore from backup of {formatTimestamp(backup.created_at)}?
            </h2>
            <p className="text-xs text-[var(--text-muted)] mt-1">
              kino v{backup.kino_version} · {formatBytes(backup.size_bytes)}
            </p>
          </div>
        </div>
        <ul className="text-sm text-[var(--text-secondary)] space-y-2 my-4">
          <li>• Replaces your current settings + database.</li>
          <li>• Your media files on disk are not touched.</li>
          <li>• Active downloads will pause and may need re-grabbing.</li>
          <li>
            • A pre-restore snapshot of the current state is saved automatically so you can undo
            this.
          </li>
          <li className="text-[var(--text-muted)]">
            • You&apos;ll need to restart kino to load the restored data.
          </li>
        </ul>
        <div className="flex gap-2 justify-end mt-5">
          <button
            type="button"
            onClick={onCancel}
            disabled={submitting}
            className="px-3 py-2 rounded-md bg-white/5 hover:bg-white/10 text-sm text-[var(--text-secondary)] disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={submitting}
            className="px-3 py-2 rounded-md bg-red-600/80 hover:bg-red-600 text-sm font-medium text-white disabled:opacity-50"
          >
            {submitting ? 'Restoring…' : 'Restore'}
          </button>
        </div>
      </div>
    </div>
  );
}

// ─── Formatting helpers ─────────────────────────────────────────

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function formatTimestamp(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function formatRelative(iso: string): string {
  const then = new Date(iso).getTime();
  const diffSec = Math.max(0, Math.floor((Date.now() - then) / 1000));
  if (diffSec < 60) return 'just now';
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)}m ago`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)}h ago`;
  return `${Math.floor(diffSec / 86400)}d ago`;
}
