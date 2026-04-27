import { keepPreviousData, useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useNavigate, useSearch } from '@tanstack/react-router';
import { ArrowUp, CheckCircle2, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import {
  genresOptions,
  trendingMoviesOptions,
  trendingShowsOptions,
} from '@/api/generated/@tanstack/react-query.gen';
import { discoverMovies, discoverShows } from '@/api/generated/sdk.gen';
import type { TmdbDiscoverMovie, TmdbDiscoverShow } from '@/api/generated/types.gen';
import { PosterCardSkeleton } from '@/components/PosterCardSkeleton';
import { TmdbMovieCard } from '@/components/TmdbMovieCard';
import { TmdbShowCard } from '@/components/TmdbShowCard';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { cn } from '@/lib/utils';

type Tab = 'trending' | 'popular';
type ContentType = 'movies' | 'shows';
type SortOption =
  | 'popularity.desc'
  | 'vote_average.desc'
  | 'primary_release_date.desc'
  | 'revenue.desc';

const sortOptions: { value: SortOption; label: string; movieOnly?: boolean }[] = [
  { value: 'popularity.desc', label: 'Popular' },
  { value: 'vote_average.desc', label: 'Top Rated' },
  { value: 'primary_release_date.desc', label: 'Newest' },
  // Revenue is meaningless for TV — TMDB doesn't track TV revenue
  // and returns effectively-random ordering, so hide for shows.
  { value: 'revenue.desc', label: 'Revenue', movieOnly: true },
];

const currentYear = new Date().getFullYear();

const yearPresets: { label: string; from?: number; to?: number }[] = [
  { label: 'All Time' },
  { label: String(currentYear), from: currentYear, to: currentYear },
  { label: String(currentYear - 1), from: currentYear - 1, to: currentYear - 1 },
  { label: '2020s', from: 2020, to: 2029 },
  { label: '2010s', from: 2010, to: 2019 },
  { label: '2000s', from: 2000, to: 2009 },
  { label: 'Classic', from: 1900, to: 1999 },
];

const ratingOptions: { value: string; label: string }[] = [
  { value: '', label: 'Any rating' },
  { value: '7', label: '★ 7+' },
  { value: '8', label: '★ 8+' },
  { value: '9', label: '★ 9+' },
];

interface DiscoverSearch {
  tab?: Tab;
  type?: ContentType;
  genre?: string;
  sort?: SortOption;
  from?: string;
  to?: string;
  rating?: string;
}

export function Discover() {
  useDocumentTitle('Discover');
  const search = useSearch({ strict: false }) as DiscoverSearch;
  const navigate = useNavigate();
  const loadMoreRef = useRef<HTMLDivElement>(null);
  const [showTopButton, setShowTopButton] = useState(false);

  const tab: Tab = search.tab ?? 'popular';
  const contentType: ContentType = search.type ?? 'movies';
  const genreId = search.genre ? Number(search.genre) : null;
  const yearFrom = search.from ? Number(search.from) : null;
  const yearTo = search.to ? Number(search.to) : null;
  const ratingGte = search.rating ? Number(search.rating) : null;

  // Revenue sort is movie-only; fall back to Popular when the user
  // switches to shows with Revenue selected so the grid isn't empty.
  const rawSort: SortOption = search.sort ?? 'popularity.desc';
  const sortBy: SortOption =
    rawSort === 'revenue.desc' && contentType === 'shows' ? 'popularity.desc' : rawSort;

  const setFilter = (updates: Partial<DiscoverSearch>) => {
    navigate({
      to: '/discover',
      search: (prev: DiscoverSearch) => ({
        ...prev,
        ...updates,
      }),
      replace: true,
    });
  };

  // Fetch genres
  const genresQuery = useQuery(genresOptions());
  const genres = contentType === 'movies' ? genresQuery.data?.movie : genresQuery.data?.tv;

  // Trending — single page
  const trendingM = useQuery({
    ...trendingMoviesOptions(),
    enabled: tab === 'trending' && contentType === 'movies',
  });
  const trendingS = useQuery({
    ...trendingShowsOptions(),
    enabled: tab === 'trending' && contentType === 'shows',
  });

  // Popular — infinite scroll with filters. keepPreviousData keeps the
  // existing grid on screen while a new filter request runs, so
  // flipping sort/year doesn't flash-clear and shove skeletons in.
  const filterKey = `${genreId}-${yearFrom}-${yearTo}-${sortBy}-${ratingGte}`;

  const popularM = useInfiniteQuery({
    queryKey: ['discover', 'movies', 'popular', filterKey],
    queryFn: async ({ pageParam = 1 }) => {
      const { data } = await discoverMovies({
        query: {
          page: pageParam,
          genre_id: genreId ?? undefined,
          year_from: yearFrom ?? undefined,
          year_to: yearTo ?? undefined,
          sort_by: sortBy,
          vote_average_gte: ratingGte ?? undefined,
        },
      });
      if (!data) throw new Error('empty discover response');
      return data;
    },
    getNextPageParam: (lastPage) =>
      lastPage.page < Math.min(lastPage.total_pages, 500) ? lastPage.page + 1 : undefined,
    initialPageParam: 1,
    enabled: tab === 'popular' && contentType === 'movies',
    placeholderData: keepPreviousData,
  });

  const popularS = useInfiniteQuery({
    queryKey: ['discover', 'shows', 'popular', filterKey],
    queryFn: async ({ pageParam = 1 }) => {
      const { data } = await discoverShows({
        query: {
          page: pageParam,
          genre_id: genreId ?? undefined,
          year_from: yearFrom ?? undefined,
          year_to: yearTo ?? undefined,
          sort_by: sortBy,
          vote_average_gte: ratingGte ?? undefined,
        },
      });
      if (!data) throw new Error('empty discover response');
      return data;
    },
    getNextPageParam: (lastPage) =>
      lastPage.page < Math.min(lastPage.total_pages, 500) ? lastPage.page + 1 : undefined,
    initialPageParam: 1,
    enabled: tab === 'popular' && contentType === 'shows',
    placeholderData: keepPreviousData,
  });

  const activeInfinite = contentType === 'movies' ? popularM : popularS;

  // Intersection observer
  useEffect(() => {
    if (tab !== 'popular') return;
    const el = loadMoreRef.current;
    if (!el) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (
          entries[0].isIntersecting &&
          activeInfinite.hasNextPage &&
          !activeInfinite.isFetchingNextPage
        ) {
          activeInfinite.fetchNextPage();
        }
      },
      { threshold: 0.1 }
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [tab, activeInfinite]);

  // Scroll-to-top button — visible once the user has scrolled past
  // the first viewport. Using window scroll (not element) because the
  // Discover page is the scroll container.
  useEffect(() => {
    const onScroll = () => setShowTopButton(window.scrollY > 600);
    onScroll();
    window.addEventListener('scroll', onScroll, { passive: true });
    return () => window.removeEventListener('scroll', onScroll);
  }, []);

  // Collect results with proper typing per path. Trending returns a
  // flat `results` array; popular returns paginated pages we flatten.
  let movieResults: TmdbDiscoverMovie[] = [];
  let showResults: TmdbDiscoverShow[] = [];
  let isLoading = false;

  if (tab === 'trending') {
    if (contentType === 'movies') {
      movieResults = trendingM.data?.results ?? [];
      isLoading = trendingM.isLoading;
    } else {
      showResults = trendingS.data?.results ?? [];
      isLoading = trendingS.isLoading;
    }
  } else if (contentType === 'movies') {
    // TMDB's /discover/movie reorders by popularity between page
    // fetches, which can surface the same movie on consecutive pages.
    // flatMap would then produce duplicates and React warns about
    // duplicate keys. Dedupe by id, preserving first-seen order.
    const seen = new Set<number>();
    movieResults =
      popularM.data?.pages
        .flatMap((p) => p.results)
        .filter((m) => {
          if (seen.has(m.id)) return false;
          seen.add(m.id);
          return true;
        }) ?? [];
    isLoading = popularM.isLoading;
  } else {
    const seen = new Set<number>();
    showResults =
      popularS.data?.pages
        .flatMap((p) => p.results)
        .filter((s) => {
          if (seen.has(s.id)) return false;
          seen.add(s.id);
          return true;
        }) ?? [];
    isLoading = popularS.isLoading;
  }

  const hasActiveFilters =
    genreId !== null ||
    yearFrom !== null ||
    yearTo !== null ||
    ratingGte !== null ||
    sortBy !== 'popularity.desc';

  const visibleSortOptions = sortOptions.filter((o) => !o.movieOnly || contentType === 'movies');

  const atEnd = tab === 'popular' && !activeInfinite.hasNextPage && !activeInfinite.isFetching;
  const hasResults = contentType === 'movies' ? movieResults.length > 0 : showResults.length > 0;

  return (
    <div className="px-4 md:px-12 py-6 pb-24 md:pb-8">
      {/* Controls row */}
      <div className="flex items-center gap-2 overflow-x-auto scrollbar-hide">
        <div className="flex items-center gap-1 bg-[var(--bg-card)] rounded-lg p-0.5 flex-shrink-0">
          {(['trending', 'popular'] as Tab[]).map((t) => (
            <button
              key={t}
              type="button"
              onClick={() => setFilter({ tab: t })}
              className={cn(
                'px-3 py-1 rounded-md text-xs font-medium transition-colors capitalize',
                tab === t ? 'bg-white/10 text-white' : 'text-[var(--text-muted)] hover:text-white'
              )}
            >
              {t}
            </button>
          ))}
        </div>

        <div className="flex items-center gap-1 bg-[var(--bg-card)] rounded-lg p-0.5 flex-shrink-0">
          {(['movies', 'shows'] as ContentType[]).map((ct) => (
            <button
              key={ct}
              type="button"
              onClick={() => setFilter({ type: ct, genre: undefined })}
              className={cn(
                'px-3 py-1 rounded-md text-xs font-medium transition-colors capitalize',
                contentType === ct
                  ? 'bg-white/10 text-white'
                  : 'text-[var(--text-muted)] hover:text-white'
              )}
            >
              {ct}
            </button>
          ))}
        </div>

        {tab === 'popular' && (
          <>
            <div className="w-px h-5 bg-white/10 flex-shrink-0" />

            <select
              value={sortBy}
              onChange={(e) => setFilter({ sort: e.target.value as SortOption })}
              className="h-7 px-2 rounded-md bg-[var(--bg-card)] border border-white/10 text-xs text-white flex-shrink-0"
            >
              {visibleSortOptions.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>

            <select
              value={search.rating ?? ''}
              onChange={(e) => setFilter({ rating: e.target.value ? e.target.value : undefined })}
              className="h-7 px-2 rounded-md bg-[var(--bg-card)] border border-white/10 text-xs text-white flex-shrink-0"
            >
              {ratingOptions.map((o) => (
                <option key={o.value || 'any'} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>

            {/* Year — preset buttons on md+; dropdown on mobile to
                avoid horizontal overflow swamping the controls row. */}
            <div className="w-px h-5 bg-white/10 flex-shrink-0" />
            <select
              value={yearPresets.findIndex((yp) =>
                yp.from ? yearFrom === yp.from && yearTo === yp.to : !yearFrom && !yearTo
              )}
              onChange={(e) => {
                const yp = yearPresets[Number(e.target.value)] ?? yearPresets[0];
                setFilter({
                  from: yp.from ? String(yp.from) : undefined,
                  to: yp.to ? String(yp.to) : undefined,
                });
              }}
              className="md:hidden h-7 px-2 rounded-md bg-[var(--bg-card)] border border-white/10 text-xs text-white flex-shrink-0"
            >
              {yearPresets.map((yp, i) => (
                <option key={yp.label} value={i}>
                  {yp.label}
                </option>
              ))}
            </select>
            <div className="hidden md:contents">
              {yearPresets.map((yp) => {
                const isActive = yp.from
                  ? yearFrom === yp.from && yearTo === yp.to
                  : !yearFrom && !yearTo;
                return (
                  <button
                    key={yp.label}
                    type="button"
                    onClick={() =>
                      setFilter({
                        from: yp.from ? String(yp.from) : undefined,
                        to: yp.to ? String(yp.to) : undefined,
                      })
                    }
                    className={cn(
                      'px-2.5 py-0.5 rounded-full text-xs font-medium transition-colors flex-shrink-0 whitespace-nowrap',
                      isActive
                        ? 'bg-white/15 text-white'
                        : 'text-[var(--text-muted)] hover:text-white hover:bg-white/5'
                    )}
                  >
                    {yp.label}
                  </button>
                );
              })}
            </div>

            {hasActiveFilters && (
              <>
                <div className="w-px h-5 bg-white/10 flex-shrink-0" />
                <button
                  type="button"
                  onClick={() =>
                    setFilter({
                      genre: undefined,
                      from: undefined,
                      to: undefined,
                      sort: undefined,
                      rating: undefined,
                    })
                  }
                  className="flex items-center gap-1 px-2 py-0.5 rounded-full bg-white/5 text-xs text-[var(--text-secondary)] hover:text-white hover:bg-white/10 transition flex-shrink-0 whitespace-nowrap"
                >
                  <X size={10} />
                  Clear
                </button>
              </>
            )}
          </>
        )}
      </div>

      {/* Genre chips */}
      {tab === 'popular' && genres && genres.length > 0 && (
        <div className="flex flex-wrap gap-1.5 mt-2.5 mb-5">
          {genres.map((g) => (
            <button
              key={g.id}
              type="button"
              onClick={() => setFilter({ genre: genreId === g.id ? undefined : String(g.id) })}
              className={cn(
                'px-2.5 py-1 rounded-full text-xs font-medium transition-colors',
                genreId === g.id
                  ? 'bg-[var(--accent)] text-white'
                  : 'bg-white/5 text-[var(--text-secondary)] hover:bg-white/10 hover:text-white'
              )}
            >
              {g.name}
            </button>
          ))}
        </div>
      )}

      {/* Spacer when no genre row */}
      {(tab !== 'popular' || !genres?.length) && <div className="mb-5" />}

      {/* Grid */}
      {isLoading ? (
        <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8 gap-4">
          {Array.from({ length: 20 }, (_, i) => (
            <PosterCardSkeleton key={String(i)} />
          ))}
        </div>
      ) : (
        <>
          <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8 gap-4">
            {contentType === 'movies'
              ? movieResults.map((m) => (
                  <TmdbMovieCard
                    key={m.id}
                    id={m.id}
                    title={m.title}
                    releaseDate={m.release_date}
                    posterPath={m.poster_path}
                  />
                ))
              : showResults.map((s) => (
                  <TmdbShowCard
                    key={s.id}
                    id={s.id}
                    name={s.name}
                    firstAirDate={s.first_air_date}
                    posterPath={s.poster_path}
                  />
                ))}
          </div>

          {tab === 'popular' && (
            <div ref={loadMoreRef} className="py-8 flex justify-center">
              {activeInfinite.isFetchingNextPage && (
                <div className="flex gap-3">
                  {Array.from({ length: 4 }, (_, i) => (
                    <PosterCardSkeleton key={String(i)} />
                  ))}
                </div>
              )}
              {atEnd && hasResults && (
                <div className="flex items-center gap-2 text-xs text-[var(--text-muted)]">
                  <CheckCircle2 size={12} className="text-green-400/70" />
                  You&apos;ve reached the end.
                </div>
              )}
              {atEnd && !hasResults && (
                <p className="text-xs text-[var(--text-muted)]">
                  No results for the current filters.
                </p>
              )}
            </div>
          )}
        </>
      )}

      {/* Scroll-to-top floater */}
      {showTopButton && (
        <button
          type="button"
          onClick={() => window.scrollTo({ top: 0, behavior: 'smooth' })}
          title="Back to top"
          className="fixed bottom-6 right-6 z-40 w-10 h-10 flex items-center justify-center rounded-full bg-black/70 backdrop-blur text-white ring-1 ring-white/10 hover:bg-black/90 transition"
        >
          <ArrowUp size={16} />
        </button>
      )}
    </div>
  );
}
