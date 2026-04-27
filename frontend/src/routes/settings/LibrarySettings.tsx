import { AlertTriangle, Check, FolderOpen, HardDrive, Loader2, Play } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { testPath } from '@/api/generated/sdk.gen';
import { FormField, TextInput, Toggle } from '@/components/settings/FormField';
import { NamingFormatInput } from '@/components/settings/NamingFormatInput';
import { PathBrowser } from '@/components/settings/PathBrowser';
import { cn } from '@/lib/utils';
import { useSettingsContext } from './SettingsLayout';

// Tokens accepted by each naming format. Keep in lockstep with the
// backend renderer in `import/naming.rs`.
const MOVIE_TOKENS = [
  'title',
  'year',
  'quality',
  'resolution',
  'source',
  'codec',
  'hdr',
  'audio',
  'group',
  'imdb',
  'tmdb',
];
const EPISODE_TOKENS = [
  'show',
  'title',
  'season',
  'episode',
  'quality',
  'resolution',
  'source',
  'codec',
  'group',
];
const MULTI_EP_TOKENS = [...EPISODE_TOKENS, 'episode_end'];
const SEASON_TOKENS = ['season'];

// One-click naming presets for the common media server ecosystems.
// Chosen to match each app's auto-match heuristics closely so existing
// libraries stay discoverable after a re-scan.
const NAMING_PRESETS: Record<
  string,
  { label: string; movie: string; episode: string; multiEp: string; season: string }
> = {
  plex: {
    label: 'Plex',
    movie: '{title} ({year}) [{quality}]',
    episode: '{show} - S{season:00}E{episode:00} - {title} [{quality}]',
    multiEp: '{show} - S{season:00}E{episode:00}-E{episode_end:00} - {title} [{quality}]',
    season: 'Season {season:00}',
  },
  jellyfin: {
    label: 'Jellyfin',
    movie: '{title} ({year})',
    episode: '{show} S{season:00}E{episode:00} {title}',
    multiEp: '{show} S{season:00}E{episode:00}-E{episode_end:00} {title}',
    season: 'Season {season}',
  },
  emby: {
    label: 'Emby',
    movie: '{title} ({year}) - {quality}',
    episode: '{show} - S{season:00}E{episode:00} - {title}',
    multiEp: '{show} - S{season:00}E{episode:00}-E{episode_end:00} - {title}',
    season: 'Season {season:00}',
  },
  kodi: {
    label: 'Kodi',
    movie: '{title} ({year})',
    episode: '{show}.S{season:00}E{episode:00}.{title}',
    multiEp: '{show}.S{season:00}E{episode:00}-E{episode_end:00}.{title}',
    season: 'Season {season:00}',
  },
};

interface PathProbe {
  status: 'idle' | 'testing' | 'ok' | 'bad';
  message?: string;
  deviceId?: number;
  freeBytes?: number;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let i = -1;
  let val = n;
  while (val >= 1024 && i < units.length - 1) {
    val /= 1024;
    i += 1;
  }
  return `${val.toFixed(val >= 100 ? 0 : 1)} ${units[i]}`;
}

function previewNaming(format: string, tokens: Record<string, string>): string {
  let result = format;
  for (const [key, val] of Object.entries(tokens)) {
    result = result.replace(`{${key}}`, val);
    result = result.replace(`{${key}:00}`, val.padStart(2, '0'));
  }
  return result;
}

export function LibrarySettings() {
  const { config, updateField } = useSettingsContext();
  const mediaPath = String(config.media_library_path ?? '');
  const downloadPath = String(config.download_path ?? '');

  const movieFormat = String(config.movie_naming_format ?? '{title} ({year}) [{quality}]');
  const episodeFormat = String(
    config.episode_naming_format ?? '{show} - S{season:00}E{episode:00} - {title} [{quality}]'
  );
  const multiEpFormat = String(
    config.multi_episode_naming_format ??
      '{show} - S{season:00}E{episode:00}-E{episode_end:00} - {title} [{quality}]'
  );
  const seasonFormat = String(config.season_folder_format ?? 'Season {season:00}');

  const [mediaProbe, setMediaProbe] = useState<PathProbe>({ status: 'idle' });
  const [downloadProbe, setDownloadProbe] = useState<PathProbe>({ status: 'idle' });
  const [browserFor, setBrowserFor] = useState<null | 'media' | 'download'>(null);

  // Min visible duration for the spinner — matches the shared
  // TestButton's MIN_TESTING_MS. testPath() typically returns in
  // <50ms, so without this the button flashes through testing and
  // the result looks like it just "appeared" with no progression.
  const MIN_TESTING_MS = 300;

  const runProbe = useCallback(
    async (path: string, set: React.Dispatch<React.SetStateAction<PathProbe>>) => {
      if (!path) return;
      // Preserve the last known deviceId / freeBytes while we re-probe.
      // Without this the "X GB free" line disappears during the spinner
      // and comes back when the probe completes, reflowing the row.
      set((prev) => ({ ...prev, status: 'testing' }));
      const start = Date.now();
      let next: PathProbe;
      try {
        const { data } = await testPath({ query: { path } });
        if (!data) throw new Error('no response');
        next = {
          status: data.error ? 'bad' : 'ok',
          message: data.error ?? `free: ${data.free_bytes ? formatBytes(data.free_bytes) : '?'}`,
          deviceId: data.device_id ?? undefined,
          freeBytes: data.free_bytes ?? undefined,
        };
      } catch (e) {
        next = { status: 'bad', message: e instanceof Error ? e.message : 'failed' };
      }
      const elapsed = Date.now() - start;
      if (elapsed < MIN_TESTING_MS) {
        await new Promise((r) => setTimeout(r, MIN_TESTING_MS - elapsed));
      }
      set(next);
    },
    []
  );

  // Auto-probe whenever a path value settles. Debounced so typing into
  // the field doesn't fire a cascade of probes (and red flashes) on
  // every keystroke — the badge updates ~400ms after the user stops.
  useEffect(() => {
    if (!mediaPath) return;
    const t = setTimeout(() => runProbe(mediaPath, setMediaProbe), 400);
    return () => clearTimeout(t);
  }, [mediaPath, runProbe]);
  useEffect(() => {
    if (!downloadPath) return;
    const t = setTimeout(() => runProbe(downloadPath, setDownloadProbe), 400);
    return () => clearTimeout(t);
  }, [downloadPath, runProbe]);

  const useHardlinks = Boolean(config.use_hardlinks);
  const differentFs =
    mediaProbe.deviceId !== undefined &&
    downloadProbe.deviceId !== undefined &&
    mediaProbe.deviceId !== downloadProbe.deviceId;
  const unknownFs =
    (mediaProbe.deviceId === undefined && mediaProbe.status === 'ok') ||
    (downloadProbe.deviceId === undefined && downloadProbe.status === 'ok');

  const moviePreview = previewNaming(movieFormat, {
    title: 'The Matrix',
    year: '1999',
    quality: 'Bluray-1080p',
    resolution: '1080p',
    source: 'Bluray',
    codec: 'x264',
    group: 'GROUP',
    imdb: 'tt0133093',
    tmdb: '603',
  });
  const episodePreview = previewNaming(episodeFormat, {
    show: 'Breaking Bad',
    title: 'Ozymandias',
    season: '5',
    episode: '14',
    quality: 'Bluray-1080p',
    resolution: '1080p',
    source: 'Bluray',
    codec: 'x264',
    group: 'DON',
  });
  const multiEpPreview = previewNaming(multiEpFormat, {
    show: 'Breaking Bad',
    title: 'Felina',
    season: '5',
    episode: '15',
    episode_end: '16',
    quality: 'Bluray-1080p',
    resolution: '1080p',
    source: 'Bluray',
    codec: 'x264',
    group: 'DON',
  });
  const seasonPreview = previewNaming(seasonFormat, { season: '5' });

  return (
    <div>
      <h1 className="text-xl font-bold mb-1">Media Library</h1>
      <p className="text-sm text-[var(--text-muted)] mb-6">File paths, naming, and organization</p>

      <section className="space-y-1 border-b border-white/5 pb-6 mb-6">
        <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider mb-3">
          Paths
        </h2>
        <FormField
          label="Media Library"
          description="Where organized media files are stored"
          help="This is where kino places renamed files after import. Should be on the same filesystem as Download Path if using hardlinks."
        >
          <div className="space-y-1.5">
            <div className="flex gap-2">
              <div className="flex-1">
                <TextInput
                  value={mediaPath}
                  onChange={(v) => updateField('media_library_path', v)}
                  placeholder="/media/library"
                />
              </div>
              <BrowseButton onClick={() => setBrowserFor('media')} />
              <PathStatus probe={mediaProbe} onTest={() => runProbe(mediaPath, setMediaProbe)} />
            </div>
            {mediaProbe.freeBytes !== undefined && (
              <p
                className={cn(
                  'text-xs text-[var(--text-muted)]',
                  mediaProbe.status === 'testing' && 'opacity-50'
                )}
              >
                {formatBytes(mediaProbe.freeBytes)} free
              </p>
            )}
          </div>
        </FormField>
        <FormField
          label="Download Path"
          description="Where torrents download to"
          help="Temporary storage for active downloads. Files are moved/linked to Media Library after import."
        >
          <div className="space-y-1.5">
            <div className="flex gap-2">
              <div className="flex-1">
                <TextInput
                  value={downloadPath}
                  onChange={(v) => updateField('download_path', v)}
                  placeholder="/media/downloads"
                />
              </div>
              <BrowseButton onClick={() => setBrowserFor('download')} />
              <PathStatus
                probe={downloadProbe}
                onTest={() => runProbe(downloadPath, setDownloadProbe)}
              />
            </div>
            {downloadProbe.freeBytes !== undefined && (
              <p
                className={cn(
                  'text-xs text-[var(--text-muted)]',
                  downloadProbe.status === 'testing' && 'opacity-50'
                )}
              >
                {formatBytes(downloadProbe.freeBytes)} free
              </p>
            )}
          </div>
        </FormField>

        <PathBrowser
          open={browserFor !== null}
          onOpenChange={(o) => !o && setBrowserFor(null)}
          title={browserFor === 'media' ? 'Select Media Library' : 'Select Download Path'}
          startPath={browserFor === 'media' ? mediaPath || '/' : downloadPath || '/'}
          onSelect={(picked) => {
            if (browserFor === 'media') updateField('media_library_path', picked);
            else if (browserFor === 'download') updateField('download_path', picked);
          }}
        />
        <FormField
          label="Use Hardlinks"
          description="Link instead of copy (saves disk space)"
          help="Hardlinks share disk blocks between download and library copies. Requires both paths on the same filesystem. Falls back to copy if hardlink fails."
        >
          <div className="space-y-1.5">
            <Toggle checked={useHardlinks} onChange={(v) => updateField('use_hardlinks', v)} />
            {useHardlinks && differentFs && (
              <div className="flex items-start gap-2 rounded-lg bg-amber-500/10 ring-1 ring-amber-500/20 p-2 text-xs text-amber-300">
                <AlertTriangle size={14} className="flex-shrink-0 mt-0.5" />
                <span>
                  Media Library and Download Path are on different filesystems. Hardlinks will fail
                  and fall back to copying (still correct, just uses 2× disk).
                </span>
              </div>
            )}
            {useHardlinks && unknownFs && (
              <p className="text-xs text-[var(--text-muted)] flex items-start gap-1.5">
                <HardDrive size={12} className="flex-shrink-0 mt-0.5" />
                Filesystem match can't be determined on this platform — imports will copy on
                failure.
              </p>
            )}
          </div>
        </FormField>
      </section>

      <section className="space-y-1">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-semibold text-[var(--text-secondary)] uppercase tracking-wider">
            Naming
          </h2>
          <div className="flex items-center gap-2">
            <span className="text-xs text-[var(--text-muted)]">Preset:</span>
            <select
              onChange={(e) => {
                const preset = NAMING_PRESETS[e.target.value];
                if (!preset) return;
                updateField('movie_naming_format', preset.movie);
                updateField('episode_naming_format', preset.episode);
                updateField('multi_episode_naming_format', preset.multiEp);
                updateField('season_folder_format', preset.season);
                e.target.value = '';
              }}
              defaultValue=""
              className="h-7 pl-2 pr-7 rounded-md bg-white/5 text-xs text-[var(--text-secondary)] hover:bg-white/10 focus:outline-none focus:ring-1 focus:ring-[var(--accent)] cursor-pointer"
            >
              <option value="" disabled>
                Apply preset…
              </option>
              {Object.entries(NAMING_PRESETS).map(([key, p]) => (
                <option key={key} value={key}>
                  {p.label}
                </option>
              ))}
            </select>
          </div>
        </div>
        <FormField
          label="Movie Format"
          description="Template for movie filenames"
          preview={`${moviePreview}.mkv`}
          help="Click + Token to insert. {title} and {year} disambiguate same-named films."
        >
          <NamingFormatInput
            value={movieFormat}
            onChange={(v) => updateField('movie_naming_format', v)}
            validTokens={MOVIE_TOKENS}
            requiredTokens={['title', 'year']}
          />
        </FormField>
        <FormField
          label="Episode Format"
          description="Single-episode filenames"
          preview={`${episodePreview}.mkv`}
          help="Click + Token to insert. {show} / {season} / {episode} are needed for Plex/Jellyfin matching."
        >
          <NamingFormatInput
            value={episodeFormat}
            onChange={(v) => updateField('episode_naming_format', v)}
            validTokens={EPISODE_TOKENS}
            requiredTokens={['show', 'season', 'episode']}
          />
        </FormField>
        <FormField
          label="Multi-episode Format"
          description="For double- / triple-episode files"
          preview={`${multiEpPreview}.mkv`}
          help="Used when a single file contains multiple episodes. Add {episode_end} for the final episode of the range."
        >
          <NamingFormatInput
            value={multiEpFormat}
            onChange={(v) => updateField('multi_episode_naming_format', v)}
            validTokens={MULTI_EP_TOKENS}
            requiredTokens={['show', 'season', 'episode', 'episode_end']}
          />
        </FormField>
        <FormField
          label="Season Folder"
          description="Template for season folder names"
          preview={seasonPreview}
          help="Only {season} is available for folder names."
        >
          <NamingFormatInput
            value={seasonFormat}
            onChange={(v) => updateField('season_folder_format', v)}
            validTokens={SEASON_TOKENS}
            requiredTokens={['season']}
          />
        </FormField>
      </section>
    </div>
  );
}

function BrowseButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      title="Browse server folders"
      className="h-9 w-9 rounded-lg bg-white/5 hover:bg-white/10 text-[var(--text-muted)] hover:text-white ring-1 ring-white/10 transition flex items-center justify-center flex-shrink-0"
      aria-label="Browse server folders"
    >
      <FolderOpen size={14} />
    </button>
  );
}

function PathStatus({ probe, onTest }: { probe: PathProbe; onTest: () => void }) {
  // Mirrors the shared TestButton's visual contract: label stays at
  // "Test" in every state; only the leading icon + colour change.
  // Keeps the row width constant so adjacent fields don't reflow.
  return (
    <button
      type="button"
      onClick={onTest}
      disabled={probe.status === 'testing'}
      title={probe.message ?? 'Verify the path exists and is writable'}
      data-state={probe.status}
      className={cn(
        'inline-flex items-center gap-1.5 h-9 px-3 rounded-lg text-sm font-medium transition flex-shrink-0 ring-1',
        probe.status === 'ok' && 'bg-green-500/10 text-green-300 ring-green-500/20',
        probe.status === 'bad' && 'bg-red-500/10 text-red-300 ring-red-500/20',
        probe.status === 'idle' &&
          'bg-white/5 text-[var(--text-secondary)] hover:bg-white/10 hover:text-white ring-white/10',
        probe.status === 'testing' &&
          'bg-white/5 text-[var(--text-muted)] ring-white/10 cursor-progress'
      )}
    >
      <span className="w-3.5 h-3.5 inline-flex items-center justify-center flex-shrink-0">
        {probe.status === 'idle' && <Play size={12} className="fill-current" />}
        {probe.status === 'testing' && <Loader2 size={14} className="animate-spin" />}
        {probe.status === 'ok' && <Check size={14} />}
        {probe.status === 'bad' && <AlertTriangle size={14} />}
      </span>
      Test
    </button>
  );
}
