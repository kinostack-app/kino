import {
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  redirect,
} from '@tanstack/react-router';
import { CastMiniBar } from '@/components/cast/CastMiniBar';
import { AppErrorBoundary } from '@/components/ErrorBoundary';
import { HealthBanner } from '@/components/HealthBanner';
import { BottomNav } from '@/components/layout/BottomNav';
import { TopNav } from '@/components/layout/TopNav';
import { AllTab } from '@/components/library/AllTab';
import { DownloadingTab } from '@/components/library/DownloadingTab';
import { HistoryTab } from '@/components/library/HistoryTab';
import { WantedTab } from '@/components/library/WantedTab';
import { RouteErrorCard } from '@/components/RouteErrorCard';
import { Calendar } from '@/routes/Calendar';
import { Discover } from '@/routes/Discover';
import { Health } from '@/routes/Health';
import { Home } from '@/routes/Home';
import { Library } from '@/routes/Library';
import { ListDetail } from '@/routes/ListDetail';
import { Lists } from '@/routes/Lists';
import { MovieDetail } from '@/routes/MovieDetail';
import { Search } from '@/routes/Search';
import { ShowDetail } from '@/routes/ShowDetail';
import { AutomationSettings } from '@/routes/settings/AutomationSettings';
import { BackupSettings } from '@/routes/settings/BackupSettings';
import { DevicesSettings } from '@/routes/settings/DevicesSettings';
import { DownloadSettings } from '@/routes/settings/DownloadSettings';
import { GeneralSettings } from '@/routes/settings/GeneralSettings';
import { IndexerSettings } from '@/routes/settings/IndexerSettings';
import { IntegrationsSettings } from '@/routes/settings/IntegrationsSettings';
import { LibrarySettings } from '@/routes/settings/LibrarySettings';
import { LogsSettings } from '@/routes/settings/LogsSettings';
import { MetadataSettings } from '@/routes/settings/MetadataSettings';
import { NotificationSettings } from '@/routes/settings/NotificationSettings';
import { PlaybackSettings } from '@/routes/settings/PlaybackSettings';
import { QualitySettings } from '@/routes/settings/QualitySettings';
import { SettingsLayout } from '@/routes/settings/SettingsLayout';
import { TasksSettings } from '@/routes/settings/TasksSettings';
import { VpnSettings } from '@/routes/settings/VpnSettings';
import { UnifiedPlayerRoute } from '@/routes/UnifiedPlayerRoute';

const rootRoute = createRootRoute({
  component: () => (
    <div className="min-h-screen bg-[var(--bg-primary)] text-[var(--text-primary)]">
      <TopNav />
      <main className="pt-14">
        <HealthBanner />
        {/* Per-route boundary — a crash in one page (ShowDetail,
            MovieDetail, …) renders the compact "This page crashed"
            card here instead of blanking the whole app. The TopNav
            and BottomNav stay interactive so users can navigate
            away. The outer `AppErrorBoundary` in App.tsx is the
            root catch-all for crashes outside the route tree. */}
        <AppErrorBoundary compact>
          <Outlet />
        </AppErrorBoundary>
      </main>
      <BottomNav />
      <CastMiniBar />
    </div>
  ),
  notFoundComponent: () => (
    <div className="min-h-[60vh] flex flex-col items-center justify-center gap-3 text-center px-6">
      <p className="text-xs font-semibold uppercase tracking-wider text-[var(--accent)]">404</p>
      <h1 className="text-xl font-semibold text-white">Page not found</h1>
      <p className="text-sm text-[var(--text-secondary)] max-w-sm">
        The link you followed doesn&apos;t match a page in kino. It may have been renamed or
        removed.
      </p>
      <a
        href="/"
        className="mt-2 px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold"
      >
        Go home
      </a>
    </div>
  ),
});

const homeRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  component: Home,
});

const discoverRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/discover',
  component: Discover,
});

const movieDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/movie/$tmdbId',
  component: MovieDetail,
  errorComponent: RouteErrorCard,
});

const showDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/show/$tmdbId',
  component: ShowDetail,
  errorComponent: RouteErrorCard,
  validateSearch: (search: Record<string, unknown>): { s?: string; follow?: '1' } => {
    // `s` = active season number (URL-backed so deep-linking and
    // back/forward navigate across seasons correctly).
    // `follow=1` = auto-open the Follow dialog on mount. Set when a
    // show card's "+" button redirects here so the user makes the
    // monitoring decision with full context (overview, seasons, air
    // dates) instead of one-click-committing to bulk-grabbing every
    // season from the Home grid. Stripped after open so refresh
    // doesn't re-trigger it.
    const out: { s?: string; follow?: '1' } = {};
    if (typeof search.s === 'string' && search.s.length > 0) out.s = search.s;
    if (search.follow === '1') out.follow = '1';
    return out;
  },
});

const searchRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/search',
  component: Search,
  validateSearch: (search: Record<string, unknown>): { q?: string } => {
    return typeof search.q === 'string' && search.q.length > 0 ? { q: search.q } : {};
  },
});

// Library is a layout route: sticky sub-header + Outlet. Each tab
// is its own child route (`/library`, `/library/wanted`, etc.) so
// the URL reflects the tab, back/forward buttons work, and
// bookmarks stay valid.
const libraryLayoutRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/library',
  component: Library,
  // Legacy `?tab=X` bookmarks get forwarded to the new path so we
  // don't break URLs that were shared before the migration.
  beforeLoad: ({ search }) => {
    const tab = (search as { tab?: string }).tab;
    if (tab === 'wanted' || tab === 'downloading' || tab === 'history') {
      throw redirect({ to: `/library/${tab}` });
    }
    if (tab === 'all') {
      throw redirect({ to: '/library' });
    }
  },
});

const libraryIndexRoute = createRoute({
  getParentRoute: () => libraryLayoutRoute,
  path: '/',
  component: AllTab,
});
const libraryWantedRoute = createRoute({
  getParentRoute: () => libraryLayoutRoute,
  path: 'wanted',
  component: WantedTab,
});
const libraryDownloadingRoute = createRoute({
  getParentRoute: () => libraryLayoutRoute,
  path: 'downloading',
  component: DownloadingTab,
});
const libraryHistoryRoute = createRoute({
  getParentRoute: () => libraryLayoutRoute,
  path: 'history',
  component: HistoryTab,
});

const libraryRouteTree = libraryLayoutRoute.addChildren([
  libraryIndexRoute,
  libraryWantedRoute,
  libraryDownloadingRoute,
  libraryHistoryRoute,
]);

const calendarRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/calendar',
  component: Calendar,
});

const listsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/lists',
  component: Lists,
});

const listDetailRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/lists/$id',
  component: ListDetail,
  errorComponent: RouteErrorCard,
});

const healthRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/health',
  component: Health,
});

// Settings is a layout route: sidebar + Outlet + SaveBar. Each category
// is a child route with its own URL (e.g. /settings/indexers) so the
// address bar reflects where the user is and the browser's back/forward
// buttons work naturally.
const settingsLayoutRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/settings',
  component: SettingsLayout,
});

const settingsIndexRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: '/',
  beforeLoad: () => {
    throw redirect({ to: '/settings/general' });
  },
});

const settingsGeneralRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'general',
  component: GeneralSettings,
});
const settingsLibraryRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'library',
  component: LibrarySettings,
});
const settingsMetadataRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'metadata',
  component: MetadataSettings,
});
const settingsVpnRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'vpn',
  component: VpnSettings,
});
const settingsIndexersRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'indexers',
  component: IndexerSettings,
});
const settingsQualityRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'quality',
  component: QualitySettings,
});
const settingsDownloadsRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'downloads',
  component: DownloadSettings,
});
const settingsPlaybackRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'playback',
  component: PlaybackSettings,
});
const settingsAutomationRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'automation',
  component: AutomationSettings,
});
const settingsNotificationsRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'notifications',
  component: NotificationSettings,
});
const settingsIntegrationsRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'integrations',
  component: IntegrationsSettings,
});
const settingsDevicesRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'devices',
  component: DevicesSettings,
});
const settingsTasksRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'tasks',
  component: TasksSettings,
});
const settingsBackupRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'backup',
  component: BackupSettings,
});
const settingsLogsRoute = createRoute({
  getParentRoute: () => settingsLayoutRoute,
  path: 'logs',
  component: LogsSettings,
});

const settingsRouteTree = settingsLayoutRoute.addChildren([
  settingsIndexRoute,
  settingsGeneralRoute,
  settingsLibraryRoute,
  settingsMetadataRoute,
  settingsVpnRoute,
  settingsIndexersRoute,
  settingsQualityRoute,
  settingsDownloadsRoute,
  settingsPlaybackRoute,
  settingsAutomationRoute,
  settingsNotificationsRoute,
  settingsIntegrationsRoute,
  settingsDevicesRoute,
  settingsTasksRoute,
  settingsBackupRoute,
  settingsLogsRoute,
]);

/// Unified player URL: one route for both kinds, entity id is the
/// backend dispatcher's key into the byte source.
const unifiedPlayerRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/play/$kind/$entityId',
  component: UnifiedPlayerRoute,
  errorComponent: RouteErrorCard,
  validateSearch: (search: Record<string, unknown>): { resume_at?: number } => {
    const v = search.resume_at;
    if (typeof v === 'number' && v >= 0) return { resume_at: v };
    if (typeof v === 'string') {
      const n = Number(v);
      if (Number.isFinite(n) && n >= 0) return { resume_at: n };
    }
    return {};
  },
});

// Legacy redirects — old top-level pages now live as Library tabs.
const wantedRedirectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/wanted',
  beforeLoad: () => {
    throw redirect({ to: '/library/wanted' });
  },
});

const downloadsRedirectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/downloads',
  beforeLoad: () => {
    throw redirect({ to: '/library/downloading' });
  },
});

const activityRedirectRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/activity',
  beforeLoad: () => {
    throw redirect({ to: '/library/history' });
  },
});

const routeTree = rootRoute.addChildren([
  homeRoute,
  discoverRoute,
  movieDetailRoute,
  showDetailRoute,
  searchRoute,
  libraryRouteTree,
  listsRoute,
  listDetailRoute,
  calendarRoute,
  healthRoute,
  settingsRouteTree,
  unifiedPlayerRoute,
  wantedRedirectRoute,
  downloadsRedirectRoute,
  activityRedirectRoute,
]);

export const router = createRouter({ routeTree });

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router;
  }
}
