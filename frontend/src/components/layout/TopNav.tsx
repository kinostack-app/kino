import { useQuery } from '@tanstack/react-query';
import { Link, useRouterState } from '@tanstack/react-router';
import {
  CalendarDays,
  Eye,
  EyeOff,
  Film,
  Home,
  Library,
  ListTodo,
  Search,
  Settings,
} from 'lucide-react';
import { getHealth, listLists, status as traktStatus } from '@/api/generated/sdk.gen';
import type {
  HealthResponse,
  HealthStatus,
  List as ListRow,
  TraktStatus,
} from '@/api/generated/types.gen';
import { CastButton } from '@/components/cast/CastButton';
import { cn } from '@/lib/utils';
import { setIncognito, useIncognito } from '@/state/incognito';

const navItems = [
  { label: 'Search', icon: Search, href: '/search', hideLabel: true },
  { label: 'Home', icon: Home, href: '/' },
  { label: 'Discover', icon: Film, href: '/discover' },
  { label: 'Library', icon: Library, href: '/library' },
  { label: 'Lists', icon: ListTodo, href: '/lists' },
  { label: 'Calendar', icon: CalendarDays, href: '/calendar' },
];

export function TopNav() {
  const router = useRouterState();
  const active = router.location.pathname;
  // Lists nav entry shows a badge with the count of followed lists.
  // Shares its query key with /lists so a single fetch powers both.
  const { data: lists } = useQuery<ListRow[]>({
    queryKey: ['kino', 'lists'],
    queryFn: async () => {
      const r = await listLists();
      return (r.data as ListRow[] | undefined) ?? [];
    },
    meta: {
      invalidatedBy: ['list_bulk_growth', 'list_unreachable', 'list_auto_added', 'list_deleted'],
    },
  });
  const listsCount = lists?.length ?? 0;

  const isActive = (href: string) => (href === '/' ? active === '/' : active.startsWith(href));

  return (
    <nav className="fixed top-0 left-0 right-0 z-50 h-14 flex items-center px-4 md:px-6 gap-3 bg-[var(--bg-primary)]/85 backdrop-blur-xl border-b border-white/[0.06]">
      {/* Brand */}
      <Link to="/" className="flex items-center gap-2 mr-1 group">
        <img
          src="/kino-mark.svg"
          alt="kino"
          className="h-6 w-auto transition-transform group-hover:scale-105"
        />
        <span className="text-lg font-semibold tracking-tight hidden sm:block">kino</span>
      </Link>

      {/* Nav items — underline-accent active state */}
      <div className="hidden md:flex items-center h-full">
        {navItems.map((item) => {
          const act = isActive(item.href);
          return (
            <Link
              key={item.href}
              to={item.href}
              className={cn(
                'relative flex items-center gap-2 h-full px-3.5 text-sm font-medium transition-colors',
                act ? 'text-white' : 'text-[var(--text-secondary)] hover:text-white'
              )}
            >
              <item.icon size={15} strokeWidth={act ? 2.25 : 2} />
              {!item.hideLabel && item.label}
              {item.href === '/lists' && listsCount > 0 && (
                <span className="ml-0.5 px-1.5 py-px rounded-full bg-white/10 text-[10px] font-semibold tabular-nums text-[var(--text-secondary)] min-w-[1.25rem] text-center">
                  {listsCount}
                </span>
              )}
              <span
                className={cn(
                  'absolute bottom-0 left-3 right-3 h-[2px] rounded-full bg-[var(--accent)] transition-opacity',
                  act ? 'opacity-100' : 'opacity-0'
                )}
              />
            </Link>
          );
        })}
      </div>

      <div className="flex-1" />

      {/* Mobile search icon */}
      <Link
        to="/search"
        className="sm:hidden p-2 rounded-md text-[var(--text-secondary)] hover:text-white"
      >
        <Search size={20} />
      </Link>

      <IncognitoToggle />

      <CastButton />

      {/* Settings */}
      <Link
        to="/settings"
        className={cn(
          'p-2 rounded-md transition',
          active.startsWith('/settings')
            ? 'text-white bg-white/5'
            : 'text-[var(--text-secondary)] hover:text-white hover:bg-white/5'
        )}
      >
        <Settings size={18} />
      </Link>

      <HealthDot />
    </nav>
  );
}

/**
 * Small circular indicator linking to /health. Colour mirrors the
 * top-level `overall` status. Hidden while the first fetch is in
 * flight to avoid a momentary grey flash.
 *
 * Shares its queryKey with the Health page so a single cache entry
 * drives both — fixing a bug where the dot stayed stale for 30s
 * after the Health page refetched or a WS event invalidated state.
 * Event-driven invalidation in `state/websocket.ts` targets the same
 * key so the dot reacts the moment a subsystem changes.
 */
/**
 * Per-tab "don't tell Trakt about this session" toggle. Only renders
 * when Trakt is connected — no value when scrobbling isn't a thing.
 * Resets on tab close (sessionStorage). Open eye = scrobbling on,
 * crossed eye = incognito.
 */
function IncognitoToggle() {
  const { data: trakt } = useQuery<TraktStatus | null>({
    queryKey: ['kino', 'integrations', 'trakt', 'status'],
    queryFn: async () => {
      const res = await traktStatus();
      return (res.data as TraktStatus | undefined) ?? null;
    },
    // No polling — meta-driven invalidation fires on connect/
    // disconnect/sync events.
    meta: { invalidatedBy: ['trakt_connected', 'trakt_disconnected', 'trakt_synced'] },
  });
  const incognito = useIncognito();
  if (!trakt?.connected) return null;
  const label = incognito
    ? 'Trakt scrobbling paused for this tab — click to resume'
    : 'Pause Trakt scrobbling for this tab';
  return (
    <button
      type="button"
      onClick={() => setIncognito(!incognito)}
      title={label}
      aria-label={label}
      aria-pressed={incognito}
      className={cn(
        'p-2 rounded-md transition',
        incognito
          ? 'text-amber-400 bg-amber-500/10 hover:bg-amber-500/15'
          : 'text-[var(--text-secondary)] hover:text-white hover:bg-white/5'
      )}
    >
      {incognito ? <EyeOff size={16} /> : <Eye size={16} />}
    </button>
  );
}

function HealthDot() {
  const { data } = useQuery<HealthResponse | null>({
    queryKey: ['kino', 'health'],
    queryFn: async () => {
      const res = await getHealth();
      return (res.data as HealthResponse | undefined) ?? null;
    },
    // No polling — shares the `['kino', 'health']` cache with the
    // Health page; meta invalidations keep both fresh in real time.
    meta: {
      invalidatedBy: [
        'health_warning',
        'health_recovered',
        'indexer_changed',
        'config_changed',
        'download_started',
        'download_complete',
        'download_failed',
        'download_cancelled',
      ],
    },
  });

  if (!data) return null;

  const overall: HealthStatus = data.overall;
  const color =
    overall === 'critical'
      ? 'bg-red-500'
      : overall === 'degraded'
        ? 'bg-amber-500'
        : overall === 'operational'
          ? 'bg-emerald-500'
          : 'bg-white/20';

  const label = {
    operational: 'Health · All systems operational',
    degraded: 'Health · Minor issues detected',
    critical: 'Health · Issues detected',
    unknown: 'Health · Status unknown',
  }[overall];

  return (
    <Link
      to="/health"
      title={label}
      aria-label={label}
      className="p-2 rounded-md text-[var(--text-secondary)] hover:text-white hover:bg-white/5 transition grid place-items-center"
    >
      <span
        className={cn(
          'block w-2 h-2 rounded-full',
          color,
          overall === 'critical' && 'motion-safe:animate-pulse'
        )}
      />
    </Link>
  );
}
