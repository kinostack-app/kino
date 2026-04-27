import { Link } from '@tanstack/react-router';
import { LayoutGrid, List, Search, X } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { PosterCardSkeleton } from '@/components/PosterCardSkeleton';
import { TmdbMovieCard } from '@/components/TmdbMovieCard';
import { TmdbShowCard } from '@/components/TmdbShowCard';
import { tmdbImage } from '@/lib/api';
import { cn } from '@/lib/utils';
import {
  type LibraryMovie,
  type LibraryShow,
  useLibraryMovies,
  useLibraryShows,
} from '@/state/library-cache';

type SortMode = 'title' | 'added' | 'year';
type StatusMode = 'all' | 'available' | 'unwatched' | 'watched' | 'wanted';
type ViewMode = 'grid' | 'list';

// Library view preferences persist per-device in localStorage — phone
// and desktop users reasonably want different defaults, and these
// aren't preferences users would miss syncing. See
// `docs/subsystems/18-ui-customisation.md` § Library persistence.
const LS_VIEW = 'kino.library.view';
const LS_SORT = 'kino.library.sort';
const LS_STATUS = 'kino.library.status';

function readView(): ViewMode {
  try {
    const raw = localStorage.getItem(LS_VIEW);
    if (raw === 'grid' || raw === 'list') return raw;
  } catch {
    // localStorage unavailable (private mode, quota, SSR); fall through
    // to defaults. Feature degrades silently by design.
  }
  return 'grid';
}
function readSort(): SortMode {
  try {
    const raw = localStorage.getItem(LS_SORT);
    if (raw === 'title' || raw === 'added' || raw === 'year') return raw;
  } catch {
    // see readView
  }
  return 'added';
}
function readStatus(): StatusMode {
  try {
    const raw = localStorage.getItem(LS_STATUS);
    if (
      raw === 'all' ||
      raw === 'available' ||
      raw === 'unwatched' ||
      raw === 'watched' ||
      raw === 'wanted'
    ) {
      return raw;
    }
  } catch {
    // see readView
  }
  return 'all';
}
function writeLs(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // private mode / quota exceeded — silent
  }
}

/**
 * Movies count as "watched" when play_count > 0 AND we're no longer
 * in the middle of a rewatch (playback_position_ticks === 0). This
 * isn't perfect — it treats a movie you just finished as watched —
 * but matches the bookkeeping the backend already does around
 * `watched_at`.
 */
function isMovieWatched(m: LibraryMovie): boolean {
  return m.play_count > 0 && m.playback_position_ticks === 0;
}

function isShowWatched(s: LibraryShow): boolean {
  // A show is "watched" when every monitored episode has been seen.
  // Unknown counts (older endpoints, pre-migration) fall through as
  // not-watched so they aren't hidden by accident.
  if (s.episode_count == null || s.watched_episode_count == null) return false;
  return s.episode_count > 0 && s.watched_episode_count >= s.episode_count;
}

function compareMovies(a: LibraryMovie, b: LibraryMovie, sort: SortMode): number {
  if (sort === 'title') return a.title.localeCompare(b.title);
  if (sort === 'year') return (b.year ?? 0) - (a.year ?? 0);
  // added
  return (b.added_at ?? '').localeCompare(a.added_at ?? '');
}

function compareShows(a: LibraryShow, b: LibraryShow, sort: SortMode): number {
  if (sort === 'title') return a.title.localeCompare(b.title);
  if (sort === 'year') return (b.year ?? 0) - (a.year ?? 0);
  return 0; // LibraryShow doesn't expose added_at yet — keep stable order
}

export function AllTab() {
  const { data: movieList, isLoading: moviesLoading } = useLibraryMovies();
  const { data: showList, isLoading: showsLoading } = useLibraryShows();

  const [search, setSearch] = useState('');
  // Restored from localStorage on mount; writes happen via the effects
  // below. Search stays ephemeral per spec.
  const [sort, setSort] = useState<SortMode>(readSort);
  const [statusMode, setStatusMode] = useState<StatusMode>(readStatus);
  const [view, setView] = useState<ViewMode>(readView);

  useEffect(() => writeLs(LS_VIEW, view), [view]);
  useEffect(() => writeLs(LS_SORT, sort), [sort]);
  useEffect(() => writeLs(LS_STATUS, statusMode), [statusMode]);

  const isLoading = moviesLoading || showsLoading;
  const isEmpty = (!movieList || movieList.length === 0) && (!showList || showList.length === 0);

  const filteredMovies = useMemo(() => {
    if (!movieList) return [];
    const q = search.trim().toLowerCase();
    let items = movieList;
    if (q) items = items.filter((m) => m.title.toLowerCase().includes(q));
    items = items.filter((m) => {
      switch (statusMode) {
        case 'available':
          return m.status === 'available';
        case 'unwatched':
          return m.status === 'available' && !isMovieWatched(m);
        case 'watched':
          return isMovieWatched(m);
        case 'wanted':
          return m.status === 'wanted';
        default:
          return true;
      }
    });
    return [...items].sort((a, b) => compareMovies(a, b, sort));
  }, [movieList, search, sort, statusMode]);

  const filteredShows = useMemo(() => {
    if (!showList) return [];
    const q = search.trim().toLowerCase();
    let items = showList;
    if (q) items = items.filter((s) => s.title.toLowerCase().includes(q));
    items = items.filter((s) => {
      switch (statusMode) {
        case 'available':
          // "Available" for shows: at least one monitored episode
          // exists (i.e. has any episodes at all).
          return (s.episode_count ?? 0) > 0;
        case 'unwatched':
          return (s.episode_count ?? 0) > 0 && !isShowWatched(s);
        case 'watched':
          return isShowWatched(s);
        case 'wanted':
          return (s.wanted_episode_count ?? 0) > 0;
        default:
          return true;
      }
    });
    return [...items].sort((a, b) => compareShows(a, b, sort));
  }, [showList, search, sort, statusMode]);

  if (isLoading) {
    return (
      <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8 gap-4">
        {Array.from({ length: 12 }, (_, i) => (
          <PosterCardSkeleton key={String(i)} />
        ))}
      </div>
    );
  }

  if (isEmpty) {
    return (
      <div className="flex flex-col items-center justify-center min-h-[40vh] text-[var(--text-muted)] gap-4">
        <p className="text-lg">Your library is empty</p>
        <p className="text-sm">Browse Discover or Search to add something.</p>
        <Link
          to="/discover"
          className="px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold transition"
        >
          Browse Discover
        </Link>
      </div>
    );
  }

  const noResults = filteredMovies.length === 0 && filteredShows.length === 0;

  return (
    <div className="space-y-5">
      {/* Controls */}
      <div className="flex items-center gap-2 flex-wrap">
        <div className="relative flex-1 min-w-[200px]">
          <Search
            size={14}
            className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-muted)]"
          />
          <input
            type="search"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Filter by title…"
            className="w-full h-8 pl-8 pr-8 rounded-lg bg-[var(--bg-card)] border border-white/10 text-sm text-white placeholder:text-[var(--text-muted)] focus:outline-none focus:border-white/25"
          />
          {search && (
            <button
              type="button"
              onClick={() => setSearch('')}
              className="absolute right-2 top-1/2 -translate-y-1/2 p-0.5 rounded hover:bg-white/10 text-[var(--text-muted)] hover:text-white"
              aria-label="Clear"
            >
              <X size={12} />
            </button>
          )}
        </div>

        <select
          value={statusMode}
          onChange={(e) => setStatusMode(e.target.value as StatusMode)}
          className="h-8 px-2 rounded-lg bg-[var(--bg-card)] border border-white/10 text-xs text-white flex-shrink-0"
        >
          <option value="all">All</option>
          <option value="available">Available</option>
          <option value="unwatched">Unwatched</option>
          <option value="watched">Watched</option>
          <option value="wanted">Wanted</option>
        </select>

        <select
          value={sort}
          onChange={(e) => setSort(e.target.value as SortMode)}
          className="h-8 px-2 rounded-lg bg-[var(--bg-card)] border border-white/10 text-xs text-white flex-shrink-0"
        >
          <option value="added">Recently Added</option>
          <option value="title">Title (A–Z)</option>
          <option value="year">Year (Newest)</option>
        </select>

        <div className="flex items-center bg-[var(--bg-card)] border border-white/10 rounded-lg p-0.5 flex-shrink-0">
          {(
            [
              { v: 'grid', icon: LayoutGrid, label: 'Grid' },
              { v: 'list', icon: List, label: 'List' },
            ] as const
          ).map(({ v, icon: Icon, label }) => (
            <button
              key={v}
              type="button"
              onClick={() => setView(v)}
              aria-label={label}
              title={label}
              className={cn(
                'h-7 w-7 flex items-center justify-center rounded-md transition',
                view === v ? 'bg-white/10 text-white' : 'text-[var(--text-muted)] hover:text-white'
              )}
            >
              <Icon size={14} />
            </button>
          ))}
        </div>
      </div>

      {noResults && (
        <p className="text-sm text-[var(--text-muted)] py-8 text-center">
          Nothing matches the current filter.
        </p>
      )}

      {filteredMovies.length > 0 && (
        <section>
          <h2 className="text-lg font-semibold mb-4">Movies ({filteredMovies.length})</h2>
          {view === 'grid' ? (
            <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8 gap-4">
              {filteredMovies.map((m) => (
                <TmdbMovieCard
                  key={m.tmdb_id}
                  id={m.tmdb_id}
                  title={m.title}
                  year={m.year ?? undefined}
                  posterPath={m.poster_path ?? undefined}
                  blurhash={m.blurhash_poster}
                />
              ))}
            </div>
          ) : (
            <div className="space-y-1">
              {filteredMovies.map((m) => (
                <MovieRow key={m.tmdb_id} movie={m} />
              ))}
            </div>
          )}
        </section>
      )}

      {filteredShows.length > 0 && (
        <section>
          <h2 className="text-lg font-semibold mb-4">Shows ({filteredShows.length})</h2>
          {view === 'grid' ? (
            <div className="grid grid-cols-3 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-8 gap-4">
              {filteredShows.map((s) => (
                <TmdbShowCard
                  key={s.tmdb_id}
                  id={s.tmdb_id}
                  name={s.title}
                  year={s.year ?? undefined}
                  posterPath={s.poster_path ?? undefined}
                  blurhash={s.blurhash_poster}
                />
              ))}
            </div>
          ) : (
            <div className="space-y-1">
              {filteredShows.map((s) => (
                <ShowRow key={s.tmdb_id} show={s} />
              ))}
            </div>
          )}
        </section>
      )}
    </div>
  );
}

function MovieRow({ movie }: { movie: LibraryMovie }) {
  return (
    <Link
      to="/movie/$tmdbId"
      params={{ tmdbId: String(movie.tmdb_id) }}
      className="flex items-center gap-3 px-3 py-2 rounded-lg bg-white/[0.02] hover:bg-white/[0.05] transition"
    >
      <div className="w-8 h-12 flex-shrink-0 rounded overflow-hidden bg-white/5">
        {movie.poster_path && (
          <img
            src={tmdbImage(movie.poster_path, 'w92')}
            alt=""
            className="w-full h-full object-cover"
          />
        )}
      </div>
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium truncate">{movie.title}</p>
        <p className="text-xs text-[var(--text-muted)]">
          {movie.year ?? '—'} · {movie.status}
        </p>
      </div>
      {isMovieWatched(movie) && (
        <span className="text-[10px] uppercase text-[var(--text-muted)] flex-shrink-0">
          Watched
        </span>
      )}
    </Link>
  );
}

function ShowRow({ show }: { show: LibraryShow }) {
  const watched = show.watched_episode_count ?? 0;
  const total = show.episode_count ?? 0;
  const wanted = show.wanted_episode_count ?? 0;
  return (
    <Link
      to="/show/$tmdbId"
      params={{ tmdbId: String(show.tmdb_id) }}
      className="flex items-center gap-3 px-3 py-2 rounded-lg bg-white/[0.02] hover:bg-white/[0.05] transition"
    >
      <div className="w-8 h-12 flex-shrink-0 rounded overflow-hidden bg-white/5">
        {show.poster_path && (
          <img
            src={tmdbImage(show.poster_path, 'w92')}
            alt=""
            className="w-full h-full object-cover"
          />
        )}
      </div>
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium truncate">{show.title}</p>
        <p className="text-xs text-[var(--text-muted)]">
          {show.year ?? '—'}
          {total > 0 && ` · ${watched}/${total} watched`}
          {wanted > 0 && ` · ${wanted} wanted`}
        </p>
      </div>
      {total > 0 && watched >= total && (
        <span className="text-[10px] uppercase text-[var(--text-muted)] flex-shrink-0">
          Watched
        </span>
      )}
    </Link>
  );
}
