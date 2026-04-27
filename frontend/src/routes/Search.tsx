import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { useNavigate, useSearch } from '@tanstack/react-router';
import { Library as LibraryIcon, Search as SearchIcon } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { librarySearchOptions, searchOptions } from '@/api/generated/@tanstack/react-query.gen';
import type { LibraryHit } from '@/api/generated/types.gen';
import { PosterCardSkeleton } from '@/components/PosterCardSkeleton';
import { TmdbMovieCard } from '@/components/TmdbMovieCard';
import { TmdbShowCard } from '@/components/TmdbShowCard';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { tmdbImage } from '@/lib/api';

export function Search() {
  const initial = (useSearch({ strict: false }) as { q?: string }).q ?? '';
  const [query, setQuery] = useState(initial);
  const [debouncedQuery, setDebouncedQuery] = useState(initial);
  const inputRef = useRef<HTMLInputElement>(null);

  useDocumentTitle(debouncedQuery.length >= 2 ? `Search: ${debouncedQuery}` : 'Search');

  // Keep input in sync when the URL ?q= changes (e.g. navigated from TopNav)
  useEffect(() => {
    setQuery(initial);
  }, [initial]);

  // Debounce the value that actually drives the queries — prevents the
  // result grid from rebuilding on every keystroke.
  useEffect(() => {
    const handle = setTimeout(() => setDebouncedQuery(query), 250);
    return () => clearTimeout(handle);
  }, [query]);

  // Auto-focus the input on mount so users can start typing immediately.
  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  const searchEnabled = debouncedQuery.length >= 2;

  const { data, isPending } = useQuery({
    ...searchOptions({ query: { q: debouncedQuery } }),
    enabled: searchEnabled,
    placeholderData: keepPreviousData,
  });

  // Federated library search — surfaces items already in the user's
  // library ahead of the TMDB results.
  const { data: libraryHits } = useQuery({
    ...librarySearchOptions({ query: { q: debouncedQuery, limit: 20 } }),
    enabled: searchEnabled,
    staleTime: 30_000,
    placeholderData: keepPreviousData,
  });

  const results = (data as { results?: Array<Record<string, unknown>> })?.results ?? [];

  const movies = results.filter((r) => r.media_type === 'movie');
  const shows = results.filter((r) => r.media_type === 'tv');

  // Show the skeleton only on the initial fetch. Subsequent keystrokes
  // keep prior results on screen via keepPreviousData so the grid
  // doesn't flicker back to empty while TMDB responds.
  const hasData = data !== undefined;
  const showSkeleton = searchEnabled && isPending && !hasData;

  return (
    <div className="px-4 md:px-12 py-6 pb-24 md:pb-8">
      {/* Search input */}
      <div className="relative max-w-xl mx-auto mb-8">
        <SearchIcon
          size={20}
          className="absolute left-4 top-1/2 -translate-y-1/2 text-[var(--text-muted)]"
        />
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search movies and TV shows..."
          className="w-full h-12 pl-12 pr-4 rounded-xl bg-[var(--bg-card)] border border-white/10 text-lg text-white placeholder:text-[var(--text-muted)] focus:outline-none focus:ring-2 focus:ring-[var(--accent)] transition"
        />
      </div>

      {/* Loading (first fetch only — subsequent keystrokes reuse previous results) */}
      {showSkeleton && (
        <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-6 lg:grid-cols-8 gap-4">
          {Array.from({ length: 8 }, (_, i) => (
            <PosterCardSkeleton key={String(i)} />
          ))}
        </div>
      )}

      {/* Library matches (shown above TMDB results) */}
      {searchEnabled && (libraryHits?.length ?? 0) > 0 && (
        <section className="mb-8">
          <h2 className="text-lg font-semibold mb-4 flex items-center gap-2">
            <LibraryIcon size={16} className="text-[var(--text-muted)]" />
            In Your Library
            <span className="ml-1 text-sm text-[var(--text-muted)] font-normal">
              {libraryHits?.length} match{libraryHits?.length === 1 ? '' : 'es'}
            </span>
          </h2>
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3">
            {(libraryHits ?? []).map((h) => (
              <LibraryHitCard key={`${h.item_type}-${h.id}`} hit={h} />
            ))}
          </div>
        </section>
      )}

      {/* Results */}
      {searchEnabled && hasData && (
        <>
          {movies.length > 0 && (
            <section className="mb-8">
              <h2 className="text-lg font-semibold mb-4">
                Movies
                <span className="ml-2 text-sm text-[var(--text-muted)] font-normal">
                  {movies.length} results
                </span>
              </h2>
              <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-6 lg:grid-cols-8 gap-4">
                {movies.map((m) => (
                  <TmdbMovieCard
                    key={Number(m.id)}
                    id={Number(m.id)}
                    title={String(m.title ?? m.name ?? '')}
                    releaseDate={m.release_date as string | undefined}
                    posterPath={m.poster_path as string | undefined}
                  />
                ))}
              </div>
            </section>
          )}

          {shows.length > 0 && (
            <section className="mb-8">
              <h2 className="text-lg font-semibold mb-4">
                TV Shows
                <span className="ml-2 text-sm text-[var(--text-muted)] font-normal">
                  {shows.length} results
                </span>
              </h2>
              <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-6 lg:grid-cols-8 gap-4">
                {shows.map((s) => (
                  <TmdbShowCard
                    key={Number(s.id)}
                    id={Number(s.id)}
                    name={String(s.name ?? s.title ?? '')}
                    firstAirDate={s.first_air_date as string | undefined}
                    posterPath={s.poster_path as string | undefined}
                  />
                ))}
              </div>
            </section>
          )}

          {movies.length === 0 && shows.length === 0 && (
            <p className="text-center text-[var(--text-muted)] mt-12">
              No results for &ldquo;{debouncedQuery}&rdquo;
            </p>
          )}
        </>
      )}
    </div>
  );
}

function LibraryHitCard({ hit }: { hit: LibraryHit }) {
  const navigate = useNavigate();
  const go = () => {
    if (hit.item_type === 'movie') {
      navigate({ to: '/movie/$tmdbId', params: { tmdbId: String(hit.tmdb_id) } });
    } else {
      navigate({ to: '/show/$tmdbId', params: { tmdbId: String(hit.tmdb_id) } });
    }
  };

  const statusBadge = hit.status
    ? hit.status === 'available' || hit.status === 'watched'
      ? { label: hit.status, tint: 'bg-emerald-500/15 text-emerald-300' }
      : hit.status === 'downloading'
        ? { label: hit.status, tint: 'bg-blue-500/15 text-blue-300' }
        : { label: hit.status, tint: 'bg-white/10 text-[var(--text-muted)]' }
    : null;

  return (
    <button
      type="button"
      onClick={go}
      className="flex items-center gap-3 p-2 rounded-lg bg-white/[0.04] hover:bg-white/[0.08] transition text-left"
    >
      <div className="w-10 h-14 flex-shrink-0 rounded overflow-hidden bg-white/5">
        {hit.poster_path ? (
          <img
            src={tmdbImage(hit.poster_path, 'w92') ?? undefined}
            alt=""
            className="w-full h-full object-cover"
            loading="lazy"
          />
        ) : null}
      </div>
      <div className="flex-1 min-w-0">
        <p className="font-medium truncate">{hit.title}</p>
        <p className="text-xs text-[var(--text-muted)]">
          {hit.item_type === 'movie' ? 'Movie' : 'Show'}
          {hit.year ? ` · ${hit.year}` : ''}
        </p>
      </div>
      {statusBadge && (
        <span
          className={`px-1.5 py-0.5 rounded text-[10px] uppercase font-medium tracking-wide ${statusBadge.tint}`}
        >
          {statusBadge.label}
        </span>
      )}
    </button>
  );
}
