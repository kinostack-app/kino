import { Link, Outlet, useLocation } from '@tanstack/react-router';
import { Clock, Download, Library as LibraryIcon, Search } from 'lucide-react';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { cn } from '@/lib/utils';
import { useDownloads, useLibraryMovies, useLibraryShows } from '@/state/library-cache';

export type LibraryTab = 'all' | 'wanted' | 'downloading' | 'history';

const ACTIVE_DOWNLOAD_STATES = [
  'queued',
  'grabbing',
  'downloading',
  'paused',
  'stalled',
  'seeding',
];

interface TabDef {
  value: LibraryTab;
  label: string;
  icon: typeof LibraryIcon;
  /** Sub-route under /library — empty string for the index tab. */
  path: '' | 'wanted' | 'downloading' | 'history';
}

const TABS: TabDef[] = [
  { value: 'all', label: 'All', icon: LibraryIcon, path: '' },
  { value: 'wanted', label: 'Wanted', icon: Search, path: 'wanted' },
  { value: 'downloading', label: 'Downloading', icon: Download, path: 'downloading' },
  { value: 'history', label: 'History', icon: Clock, path: 'history' },
];

/**
 * Map the current path back to a tab value for highlighting + the
 * document title. Done by inspecting the location instead of a
 * search param because each tab is now its own route
 * (`/library/wanted` etc.).
 */
function tabFromPath(pathname: string): LibraryTab {
  const seg = pathname.replace(/^\/library\/?/, '').split('/')[0];
  if (seg === 'wanted' || seg === 'downloading' || seg === 'history') return seg;
  return 'all';
}

export function Library() {
  const location = useLocation();
  const active = tabFromPath(location.pathname);

  const tabLabels: Record<LibraryTab, string> = {
    all: 'Library',
    wanted: 'Wanted',
    downloading: 'Downloading',
    history: 'History',
  };
  useDocumentTitle(tabLabels[active]);

  // Badges come from the same caches the tab bodies query, so
  // switching tabs doesn't trigger a refetch just to keep the
  // counter honest.
  const { data: movies } = useLibraryMovies();
  const { data: shows } = useLibraryShows();
  const { data: downloads } = useDownloads();

  const wantedCount =
    (movies ?? []).filter((m) => m.status === 'wanted').length +
    (shows ?? []).filter((s) => (s.wanted_episode_count ?? 0) > 0).length;
  const downloadingCount = (downloads ?? []).filter((d) =>
    ACTIVE_DOWNLOAD_STATES.includes(d.state)
  ).length;

  const badgeFor = (value: LibraryTab): number | undefined => {
    if (value === 'wanted') return wantedCount;
    if (value === 'downloading') return downloadingCount;
    return undefined;
  };

  return (
    <div>
      {/* Sticky sub-header — chains visually with the TopNav (same
          backdrop blur, same border treatment, same gutter) so the
          tabs read as a second row of the app shell rather than an
          unrelated bar floating in the page body. */}
      <div className="sticky top-14 z-30 bg-[var(--bg-primary)]/85 backdrop-blur-xl border-b border-white/[0.06]">
        <div className="px-4 md:px-12 flex items-center gap-1 overflow-x-auto scrollbar-hide">
          {TABS.map((t) => {
            const isActive = active === t.value;
            const badge = badgeFor(t.value);
            return (
              <Link
                key={t.value}
                to={t.path ? `/library/${t.path}` : '/library'}
                className={cn(
                  'relative flex items-center gap-2 px-3.5 h-11 text-sm font-medium transition-colors whitespace-nowrap',
                  isActive ? 'text-white' : 'text-[var(--text-muted)] hover:text-white'
                )}
              >
                <t.icon size={15} strokeWidth={isActive ? 2.25 : 2} />
                {t.label}
                {badge != null && badge > 0 && (
                  <span className="px-1.5 py-0.5 rounded-full bg-white/10 text-[10px] font-semibold tabular-nums">
                    {badge}
                  </span>
                )}
                <span
                  className={cn(
                    'absolute bottom-0 left-3 right-3 h-[2px] rounded-full bg-[var(--accent)] transition-opacity',
                    isActive ? 'opacity-100' : 'opacity-0'
                  )}
                />
              </Link>
            );
          })}
        </div>
      </div>

      <div className="px-4 md:px-12 py-6 pb-24 md:pb-8">
        <Outlet />
      </div>
    </div>
  );
}
