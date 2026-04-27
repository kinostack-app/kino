import { useMutation } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { CalendarClock, Loader2, RefreshCw, Search, Tv } from 'lucide-react';
import { runTask } from '@/api/generated/sdk.gen';
import { kinoToast } from '@/components/kino-toast';
import { tmdbImage } from '@/lib/api';
import {
  type LibraryMovie,
  type LibraryShow,
  useLibraryMovies,
  useLibraryShows,
} from '@/state/library-cache';

export function WantedTab() {
  const navigate = useNavigate();
  const { data: movies, isLoading: moviesLoading } = useLibraryMovies();
  const { data: shows, isLoading: showsLoading } = useLibraryShows();

  const wantedMovies = (movies ?? []).filter((m) => m.status === 'wanted');
  // A show counts as "wanted" here when at least one of its monitored
  // episodes has already aired and is waiting on a release. Shows
  // with only upcoming (unaired) monitored episodes go into their own
  // "Upcoming" group below — those aren't being searched, they're
  // just scheduled, and putting them under "Searching…" was the
  // long-standing UX gripe.
  const wantedShows = (shows ?? []).filter((s) => (s.wanted_episode_count ?? 0) > 0);
  const upcomingShows = (shows ?? []).filter(
    (s) => (s.wanted_episode_count ?? 0) === 0 && (s.upcoming_episode_count ?? 0) > 0
  );

  const total = wantedMovies.length + wantedShows.length;
  const upcomingTotal = upcomingShows.length;
  const isLoading = moviesLoading || showsLoading;

  const searchMutation = useMutation({
    mutationFn: () => runTask({ path: { name: 'wanted_search' } }),
    onSuccess: () => {
      kinoToast.success('Search started', {
        description: 'kino is checking indexers for new releases.',
      });
    },
    onError: (err) => {
      kinoToast.error('Couldn\u2019t start search', {
        description: err instanceof Error ? err.message : undefined,
      });
    },
  });

  if (isLoading) {
    return (
      <div className="flex items-center justify-center min-h-[40vh]">
        <div className="w-6 h-6 border-2 border-white/20 border-t-white rounded-full animate-spin" />
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <p className="text-sm text-[var(--text-muted)]">
          {total} {total === 1 ? 'item' : 'items'} waiting for releases
        </p>
        <button
          type="button"
          disabled={searchMutation.isPending || total === 0}
          onClick={() => searchMutation.mutate()}
          className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-white/5 hover:bg-white/10 disabled:opacity-40 text-sm font-medium transition"
        >
          {searchMutation.isPending ? (
            <Loader2 size={14} className="animate-spin" />
          ) : (
            <RefreshCw size={14} />
          )}
          Search All
        </button>
      </div>

      {total === 0 && upcomingTotal === 0 ? (
        <div className="flex flex-col items-center justify-center min-h-[30vh] text-center gap-4">
          <div className="w-16 h-16 rounded-full bg-white/5 grid place-items-center">
            <Search size={28} className="text-[var(--text-muted)]" />
          </div>
          <div>
            <p className="text-lg font-medium">Nothing wanted</p>
            <p className="text-sm text-[var(--text-muted)] mt-1">
              Added movies or shows will appear here while searching for releases.
            </p>
          </div>
          <Link
            to="/discover"
            className="px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold transition"
          >
            Browse Discover
          </Link>
        </div>
      ) : (
        <div className="space-y-6">
          {total > 0 && (
            <div className="space-y-2">
              {wantedMovies.map((movie) => (
                <WantedMovieRow
                  key={`m-${movie.id}`}
                  movie={movie}
                  onClick={() =>
                    navigate({ to: '/movie/$tmdbId', params: { tmdbId: String(movie.tmdb_id) } })
                  }
                />
              ))}
              {wantedShows.map((show) => (
                <WantedShowRow
                  key={`s-${show.id}`}
                  show={show}
                  onClick={() =>
                    navigate({ to: '/show/$tmdbId', params: { tmdbId: String(show.tmdb_id) } })
                  }
                />
              ))}
            </div>
          )}

          {upcomingTotal > 0 && (
            <section>
              <h3 className="text-[11px] uppercase tracking-wider font-semibold text-[var(--text-muted)] mb-2 flex items-center gap-1.5">
                <CalendarClock size={12} />
                Upcoming · {upcomingTotal}
              </h3>
              <div className="space-y-2">
                {upcomingShows.map((show) => (
                  <UpcomingShowRow
                    key={`u-${show.id}`}
                    show={show}
                    onClick={() =>
                      navigate({ to: '/show/$tmdbId', params: { tmdbId: String(show.tmdb_id) } })
                    }
                  />
                ))}
              </div>
            </section>
          )}
        </div>
      )}
    </div>
  );
}

function WantedMovieRow({ movie, onClick }: { movie: LibraryMovie; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="w-full flex items-center gap-4 p-3 rounded-lg bg-white/[0.03] hover:bg-white/[0.06] transition text-left"
    >
      <div className="w-12 h-[72px] flex-shrink-0 rounded overflow-hidden bg-white/5">
        {movie.poster_path ? (
          <img
            src={tmdbImage(movie.poster_path, 'w92')}
            alt=""
            className="w-full h-full object-cover"
          />
        ) : (
          <div className="w-full h-full grid place-items-center text-[var(--text-muted)]">
            <Search size={16} />
          </div>
        )}
      </div>

      <div className="flex-1 min-w-0">
        <p className="font-medium truncate">{movie.title}</p>
        <p className="text-sm text-[var(--text-muted)]">{movie.year ?? 'Unknown year'}</p>
      </div>

      <SearchingDots />
    </button>
  );
}

function WantedShowRow({ show, onClick }: { show: LibraryShow; onClick: () => void }) {
  const wanted = show.wanted_episode_count ?? 0;
  return (
    <button
      type="button"
      onClick={onClick}
      className="w-full flex items-center gap-4 p-3 rounded-lg bg-white/[0.03] hover:bg-white/[0.06] transition text-left"
    >
      <div className="w-12 h-[72px] flex-shrink-0 rounded overflow-hidden bg-white/5">
        {show.poster_path ? (
          <img
            src={tmdbImage(show.poster_path, 'w92')}
            alt=""
            className="w-full h-full object-cover"
          />
        ) : (
          <div className="w-full h-full grid place-items-center text-[var(--text-muted)]">
            <Tv size={16} />
          </div>
        )}
      </div>

      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <p className="font-medium truncate">{show.title}</p>
          <span className="text-[10px] uppercase px-1.5 py-0.5 rounded bg-white/5 text-[var(--text-muted)]">
            TV
          </span>
        </div>
        <p className="text-sm text-[var(--text-muted)]">
          {wanted} {wanted === 1 ? 'episode' : 'episodes'} wanted
        </p>
      </div>

      <SearchingDots />
    </button>
  );
}

function UpcomingShowRow({ show, onClick }: { show: LibraryShow; onClick: () => void }) {
  const upcoming = show.upcoming_episode_count ?? 0;
  return (
    <button
      type="button"
      onClick={onClick}
      className="w-full flex items-center gap-4 p-3 rounded-lg bg-white/[0.02] hover:bg-white/[0.05] transition text-left opacity-80"
    >
      <div className="w-12 h-[72px] flex-shrink-0 rounded overflow-hidden bg-white/5">
        {show.poster_path ? (
          <img
            src={tmdbImage(show.poster_path, 'w92')}
            alt=""
            className="w-full h-full object-cover"
          />
        ) : (
          <div className="w-full h-full grid place-items-center text-[var(--text-muted)]">
            <Tv size={16} />
          </div>
        )}
      </div>

      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <p className="font-medium truncate">{show.title}</p>
          <span className="text-[10px] uppercase px-1.5 py-0.5 rounded bg-white/5 text-[var(--text-muted)]">
            TV
          </span>
        </div>
        <p className="text-sm text-[var(--text-muted)]">
          {upcoming} {upcoming === 1 ? 'episode' : 'episodes'} waiting to air
        </p>
      </div>

      <div className="flex items-center gap-2 flex-shrink-0 text-[var(--text-muted)]">
        <CalendarClock size={14} />
        <span className="text-xs hidden sm:inline">Scheduled</span>
      </div>
    </button>
  );
}

function SearchingDots() {
  return (
    <div className="flex items-center gap-2 flex-shrink-0">
      <div className="flex gap-[3px] dot-animation">
        <span className="block w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
        <span className="block w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
        <span className="block w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
      </div>
      <span className="text-xs text-[var(--text-muted)] hidden sm:inline">Searching</span>
    </div>
  );
}
