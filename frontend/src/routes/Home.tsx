import { useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { SlidersHorizontal, TriangleAlert } from 'lucide-react';
import { useMemo, useState } from 'react';
import {
  discoverMoviesOptions,
  discoverShowsOptions,
  trendingMoviesOptions,
  trendingShowsOptions,
  upNextOptions,
} from '@/api/generated/@tanstack/react-query.gen';
import {
  getHomePreferences,
  listItems,
  listLists,
  recommendations as traktRecommendations,
  trending as traktTrending,
} from '@/api/generated/sdk.gen';
import type {
  HomePreferences,
  ListItemView,
  List as ListRow,
  TmdbDiscoverMovie,
  TmdbDiscoverShow,
  HomeRow as TraktHomeRow,
} from '@/api/generated/types.gen';
import { ContinueCard } from '@/components/ContinueCard';
import { CustomiseHomeDrawer } from '@/components/CustomiseHomeDrawer';
import { HeroBanner, type HeroItem } from '@/components/HeroBanner';
import { MediaRow } from '@/components/MediaRow';
import { PosterCardSkeleton } from '@/components/PosterCardSkeleton';
import { TmdbMovieCard } from '@/components/TmdbMovieCard';
import { TmdbShowCard } from '@/components/TmdbShowCard';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { useWatchNow } from '@/hooks/useWatchNow';
import { tmdbImage } from '@/lib/api';
import { cn } from '@/lib/utils';
import type { InvalidationRule } from '@/state/invalidation';

function SkeletonRow({ title }: { title: string }) {
  return (
    <MediaRow title={title}>
      {Array.from({ length: 8 }, (_, i) => (
        <PosterCardSkeleton key={`skel-${title}-${String(i)}`} />
      ))}
    </MediaRow>
  );
}

function ErrorRow({ title, onRetry }: { title: string; onRetry?: () => void }) {
  return (
    <MediaRow title={title}>
      <div className="flex items-center gap-2 px-2 py-6 text-xs text-[var(--text-muted)]">
        <TriangleAlert size={14} className="text-amber-400" />
        <span>Couldn&apos;t load this row.</span>
        {onRetry && (
          <button
            type="button"
            onClick={onRetry}
            className="text-[var(--text-secondary)] hover:text-white underline underline-offset-2"
          >
            Retry
          </button>
        )}
      </div>
    </MediaRow>
  );
}

// ── Per-row components ────────────────────────────────────────────

const CONTINUE_WATCHING_INVALIDATED_BY: InvalidationRule[] = [
  'imported',
  'upgraded',
  'content_removed',
  'watched',
  'unwatched',
  'trakt_synced',
  // Mid-session progress ticks (every ~10 s) — keeps the Up Next
  // row's progress bar + card order live without a page refresh
  // while the user is watching in another tab.
  'playback_progress',
];

function UpNextSection() {
  const q = useQuery({
    ...upNextOptions(),
    meta: { invalidatedBy: CONTINUE_WATCHING_INVALIDATED_BY },
  });
  if (q.isLoading) return <SkeletonRow title="Up Next" />;
  const items = q.data ?? [];
  if (items.length === 0) return null; // auto-hide when empty
  return (
    <MediaRow title="Up Next">
      {items.map((item) => (
        <ContinueCard key={item.id} item={item} />
      ))}
    </MediaRow>
  );
}

function TrendingMoviesSection() {
  const q = useQuery(trendingMoviesOptions());
  const items = q.data?.results ?? [];
  if (q.isLoading) return <SkeletonRow title="Trending Movies" />;
  if (q.isError) return <ErrorRow title="Trending Movies" onRetry={() => q.refetch()} />;
  return (
    <MediaRow title="Trending Movies" href="/discover">
      {items.map((m) => (
        <TmdbMovieCard
          key={m.id}
          id={m.id}
          title={m.title}
          releaseDate={m.release_date}
          posterPath={m.poster_path}
        />
      ))}
    </MediaRow>
  );
}

function TrendingShowsSection() {
  const q = useQuery(trendingShowsOptions());
  const items = q.data?.results ?? [];
  if (q.isLoading) return <SkeletonRow title="Trending TV Shows" />;
  if (q.isError) return <ErrorRow title="Trending TV Shows" onRetry={() => q.refetch()} />;
  return (
    <MediaRow title="Trending TV Shows" href="/discover">
      {items.map((s) => (
        <TmdbShowCard
          key={s.id}
          id={s.id}
          name={s.name}
          firstAirDate={s.first_air_date}
          posterPath={s.poster_path}
        />
      ))}
    </MediaRow>
  );
}

function PopularMoviesSection() {
  const q = useQuery(discoverMoviesOptions());
  const items = q.data?.results ?? [];
  if (q.isLoading) return <SkeletonRow title="Popular Movies" />;
  if (q.isError) return <ErrorRow title="Popular Movies" onRetry={() => q.refetch()} />;
  return (
    <MediaRow title="Popular Movies" href="/discover">
      {items.map((m) => (
        <TmdbMovieCard
          key={m.id}
          id={m.id}
          title={m.title}
          releaseDate={m.release_date}
          posterPath={m.poster_path}
        />
      ))}
    </MediaRow>
  );
}

function PopularShowsSection() {
  const q = useQuery(discoverShowsOptions());
  const items = q.data?.results ?? [];
  if (q.isLoading) return <SkeletonRow title="Popular TV Shows" />;
  if (q.isError) return <ErrorRow title="Popular TV Shows" onRetry={() => q.refetch()} />;
  return (
    <MediaRow title="Popular TV Shows" href="/discover">
      {items.map((s) => (
        <TmdbShowCard
          key={s.id}
          id={s.id}
          name={s.name}
          firstAirDate={s.first_air_date}
          posterPath={s.poster_path}
        />
      ))}
    </MediaRow>
  );
}

// ── Trakt rows ───────────────────────────────────────────────────

/** Render a cached Trakt row (`recommendations` or `trending`). The
 *  backend payload is already interleaved + tmdb-id-keyed, so the
 *  client just fans items out by kind so each card renders via the
 *  existing TMDB poster components. Auto-hides when the row is empty
 *  — typical when Trakt isn't connected, or the cache hasn't filled
 *  yet, or recommendations haven't been generated yet server-side. */
function TraktRowFromEndpoint({
  title,
  endpoint,
  queryKey,
}: {
  title: string;
  endpoint: 'recommendations' | 'trending';
  queryKey: string;
}) {
  const q = useQuery({
    queryKey: ['kino', 'integrations', 'trakt', queryKey],
    queryFn: async () => {
      const fn = endpoint === 'recommendations' ? traktRecommendations : traktTrending;
      const res = await fn();
      return (res.data as TraktHomeRow | undefined) ?? { items: [] };
    },
    // Daily backend refresh drives the underlying cache; the client
    // check every hour is enough to pick up that refresh.
    refetchInterval: 60 * 60 * 1000,
    meta: {
      invalidatedBy: ['trakt_connected', 'trakt_disconnected', 'trakt_synced', 'rated'],
    },
  });
  const items = q.data?.items ?? [];
  if (items.length === 0) return null; // auto-hide per spec
  return (
    <MediaRow title={title}>
      {items.map((item) => {
        if (item.kind === 'show') {
          return (
            <TmdbShowCard
              key={`t-${item.kind}-${item.tmdb_id ?? item.title}`}
              id={item.tmdb_id ?? 0}
              name={item.title}
              firstAirDate={item.year ? `${item.year}-01-01` : undefined}
            />
          );
        }
        return (
          <TmdbMovieCard
            key={`t-${item.kind}-${item.tmdb_id ?? item.title}`}
            id={item.tmdb_id ?? 0}
            title={item.title}
            year={item.year ?? undefined}
          />
        );
      })}
    </MediaRow>
  );
}

function RecommendationsSection() {
  return (
    <TraktRowFromEndpoint
      title="Recommended for you"
      endpoint="recommendations"
      queryKey="recommendations"
    />
  );
}

function TrendingTraktSection() {
  return <TraktRowFromEndpoint title="Trending on Trakt" endpoint="trending" queryKey="trending" />;
}

// Map of stable row IDs → renderer. Adding a new row type later is
// one entry here + one entry in the backend catalogue. Unknown IDs
// from server-side section_order are silently dropped — see
// `docs/subsystems/18-ui-customisation.md` § Migration strategy.
//
// Dynamic IDs: IDs prefixed with `list:` resolve at render time via
// [`resolveSection`]. This lets pinned lists participate in
// section_order / hidden_rows like any built-in row without mirror
// state on the backend.
const SECTION_REGISTRY: Record<string, () => React.ReactNode> = {
  up_next: UpNextSection,
  recommendations: RecommendationsSection,
  trending_trakt: TrendingTraktSection,
  trending_movies: TrendingMoviesSection,
  trending_shows: TrendingShowsSection,
  popular_movies: PopularMoviesSection,
  popular_shows: PopularShowsSection,
};

export const BUILT_IN_SECTION_IDS = Object.keys(SECTION_REGISTRY);

/** Resolve a section ID to a renderer. Built-ins come from the
 *  static registry; `list:<id>` expands to a dynamic `ListRowSection`
 *  keyed on the numeric list id. Unknown IDs return null so future
 *  rename / remove is a no-op. */
function resolveSection(id: string): (() => React.ReactNode) | null {
  if (id.startsWith('list:')) {
    const listId = Number(id.slice(5));
    if (!Number.isFinite(listId)) return null;
    return () => <ListRowSection listId={listId} />;
  }
  return SECTION_REGISTRY[id] ?? null;
}

/** Source-origin mark for list rows on Home. Same visual language as
 *  the one in the Customise drawer / /lists cards — Trakt circle-mark
 *  for Trakt sources, compact text pill for MDBList / TMDB. Renders
 *  before the row title so users can see "this row comes from a list"
 *  without any separate "Your lists" section header. */
function ListSourceMark({ sourceType }: { sourceType: string }) {
  if (sourceType.startsWith('trakt_')) {
    return <img src="/trakt-mark.svg" alt="Trakt" className="h-4 w-4 opacity-80 shrink-0" />;
  }
  const label =
    sourceType === 'mdblist' ? 'MDBList' : sourceType === 'tmdb_list' ? 'TMDB' : sourceType;
  return (
    <span className="shrink-0 px-1.5 py-0.5 rounded text-[9px] font-semibold uppercase tracking-wider bg-white/5 text-[var(--text-muted)] ring-1 ring-white/5">
      {label}
    </span>
  );
}

/** One row per pinned list. Title comes from the list row; body is a
 *  horizontal scroll of poster cards drawn from the joined
 *  acquisition-state items endpoint. Auto-hides when the list has no
 *  items so Home doesn't show an empty row the moment a list's added
 *  but not yet polled. */
function ListRowSection({ listId }: { listId: number }) {
  const listQ = useQuery({
    queryKey: ['kino', 'lists'],
    queryFn: async () => {
      const r = await listLists();
      return (r.data as ListRow[] | undefined) ?? [];
    },
  });
  const itemsQ = useQuery({
    queryKey: ['kino', 'lists', listId, 'items'],
    queryFn: async () => {
      const r = await listItems({ path: { id: listId } });
      return (r.data as ListItemView[] | undefined) ?? [];
    },
  });
  const list = listQ.data?.find((l) => l.id === listId);
  const items = itemsQ.data ?? [];
  if (!list || items.length === 0) return null;
  return (
    <MediaRow
      title={list.title}
      href={`/lists/${listId}`}
      titlePrefix={<ListSourceMark sourceType={list.source_type} />}
    >
      {items.map((it) =>
        it.item_type === 'show' ? (
          <TmdbShowCard
            key={`li-${it.id}`}
            id={it.tmdb_id}
            name={it.title}
            posterPath={it.poster_path ?? undefined}
          />
        ) : (
          <TmdbMovieCard
            key={`li-${it.id}`}
            id={it.tmdb_id}
            title={it.title}
            posterPath={it.poster_path ?? undefined}
          />
        )
      )}
    </MediaRow>
  );
}

// ── Greeting ──────────────────────────────────────────────────────

/** "Good evening, Robert" header above Home rows. Time-of-day comes
 *  from the *client's* local clock — a user travelling won't see
 *  "Good morning" at 11pm just because the server's in another zone.
 *  Name is null/missing → "Good evening" alone, no trailing comma. */
function Greeting({ name, heroEnabled }: { name?: string | null; heroEnabled: boolean }) {
  const period = greetingPeriod(new Date().getHours());
  const text = name && name.trim().length > 0 ? `Good ${period}, ${name.trim()}` : `Good ${period}`;
  return (
    <h1
      className={cn(
        'px-4 md:px-12 text-base md:text-lg font-medium text-white/90',
        // Tucks under the hero's shadow when one's on; first thing
        // visible on the page when hero's off.
        heroEnabled ? 'pt-3 pb-1' : 'pt-1 pb-2'
      )}
    >
      {text}
    </h1>
  );
}

function greetingPeriod(hour: number): 'morning' | 'afternoon' | 'evening' {
  if (hour >= 5 && hour < 12) return 'morning';
  if (hour >= 12 && hour < 18) return 'afternoon';
  return 'evening';
}

// ── Hero ──────────────────────────────────────────────────────────

/** Mixes 3 trending movies + 2 trending shows so TV-heavy users
 *  aren't shown an all-movie banner. Kept in its own hook so the
 *  section-registry renderers stay free of hero-specific data. */
function useHeroItems(): HeroItem[] {
  const trending = useQuery(trendingMoviesOptions());
  const trendingShows = useQuery(trendingShowsOptions());
  const movies: TmdbDiscoverMovie[] = trending.data?.results ?? [];
  const shows: TmdbDiscoverShow[] = trendingShows.data?.results ?? [];
  return useMemo(() => {
    const m: HeroItem[] = movies.slice(0, 3).map((x) => ({
      tmdbId: x.id,
      kind: 'movie',
      title: x.title,
      year: x.release_date ? Number.parseInt(x.release_date.slice(0, 4), 10) : undefined,
      overview: x.overview,
      backdropUrl: tmdbImage(x.backdrop_path, 'w1280'),
    }));
    const s: HeroItem[] = shows.slice(0, 2).map((x) => ({
      tmdbId: x.id,
      kind: 'show',
      title: x.name,
      year: x.first_air_date ? Number.parseInt(x.first_air_date.slice(0, 4), 10) : undefined,
      overview: x.overview,
      backdropUrl: tmdbImage(x.backdrop_path, 'w1280'),
    }));
    const out: HeroItem[] = [];
    const max = Math.max(m.length, s.length);
    for (let i = 0; i < max; i++) {
      if (m[i]) out.push(m[i]);
      if (s[i]) out.push(s[i]);
    }
    return out;
  }, [movies, shows]);
}

// ── Page ──────────────────────────────────────────────────────────

export function Home() {
  useDocumentTitle('Home');
  const navigate = useNavigate();

  // Preferences drive layout. Defaults (v1 row order, hero on) are
  // served by the backend when no row exists — first render always
  // has something sensible to show.
  const { data: prefs } = useQuery<HomePreferences | null>({
    queryKey: ['kino', 'preferences', 'home'],
    queryFn: async () => {
      const res = await getHomePreferences();
      return (res.data as HomePreferences | undefined) ?? null;
    },
    // Prefs rarely change and server always returns defaults, so don't
    // refetch on window focus — would flicker the registry renderers.
    refetchOnWindowFocus: false,
  });

  const heroEnabled = prefs?.hero_enabled ?? true;
  const order = prefs?.section_order ?? Object.keys(SECTION_REGISTRY);
  const hidden = useMemo(() => new Set(prefs?.section_hidden ?? []), [prefs?.section_hidden]);
  const [customiseOpen, setCustomiseOpen] = useState(false);

  const heroItems = useHeroItems();

  const { watchNow } = useWatchNow();
  const onHeroPlay = (item: HeroItem) => {
    if (item.tmdbId == null) return;
    if (item.kind === 'show') {
      watchNow({ kind: 'show_smart_play', showTmdbId: item.tmdbId, title: item.title });
      return;
    }
    watchNow({ kind: 'movie', tmdbId: item.tmdbId, title: item.title });
  };
  const onHeroMoreInfo = (item: HeroItem) => {
    if (item.tmdbId == null) return;
    const to = item.kind === 'show' ? '/show/$tmdbId' : '/movie/$tmdbId';
    navigate({ to, params: { tmdbId: String(item.tmdbId) } });
  };

  return (
    <div className="relative">
      {/* Customise button — positioned at the top-right of Home's
          render area, not the viewport. Anchored to this `relative`
          wrapper so it stays in the same spot whether the hero is
          rendered or not, and correctly moves down with any
          HealthBanner that renders above Home in the layout tree.
          Subtle dark backdrop keeps it legible against both the
          bright hero and the plain-dark no-hero state. */}
      <button
        type="button"
        onClick={() => setCustomiseOpen(true)}
        title="Customise Home"
        aria-label="Customise Home"
        className="absolute top-2 right-4 z-20 p-2 rounded-md bg-black/40 backdrop-blur-sm text-white/80 hover:text-white hover:bg-black/60 transition"
      >
        <SlidersHorizontal size={16} />
      </button>

      {heroEnabled && (
        <HeroBanner items={heroItems} onPlay={onHeroPlay} onMoreInfo={onHeroMoreInfo} />
      )}

      <div
        className={
          heroEnabled
            ? '-mt-16 relative z-10 pb-24 md:pb-8 space-y-2'
            : 'pt-4 pb-24 md:pb-8 space-y-2'
        }
      >
        <Greeting name={prefs?.greeting_name} heroEnabled={heroEnabled} />

        {order
          // Hidden rows don't render at all — auto-hide for empty
          // rows happens inside each component instead.
          .filter((id) => !hidden.has(id))
          .map((id) => {
            const Section = resolveSection(id);
            // Unknown ID from a future backend version — skip silently.
            if (!Section) return null;
            return <Section key={id} />;
          })}
      </div>

      {prefs && (
        <CustomiseHomeDrawer
          open={customiseOpen}
          onClose={() => setCustomiseOpen(false)}
          prefs={prefs}
        />
      )}
    </div>
  );
}
