import { useNavigate } from '@tanstack/react-router';
import type { ContinueItem } from '@/api/generated/types.gen';
import { PosterCard } from '@/components/PosterCard';
import { usePlayMedia } from '@/hooks/usePlayMedia';
import { tmdbImage } from '@/lib/api';

interface ContinueCardProps {
  item: ContinueItem;
}

/**
 * A Continue Watching / Up Next tile. Unifies the movie and episode
 * cases — clicking the poster navigates to the right detail page for
 * the kind, and the Play button resolves via usePlayMedia so it hits
 * /play/{mediaId} with the right resolver per kind.
 *
 * Episode tiles show the show's poster with an `S01E03 · Title`
 * overlay instead of building a separate episode-still card, so every
 * tile in the Continue row has the same 2:3 aspect ratio.
 */
export function ContinueCard({ item }: ContinueCardProps) {
  const navigate = useNavigate();
  const { playMovie, playEpisode } = usePlayMedia();

  const isEpisode = item.kind === 'episode';
  const posterUrl = tmdbImage(item.poster_path ?? undefined);
  const epCode =
    item.season != null && item.episode != null
      ? `S${String(item.season).padStart(2, '0')}E${String(item.episode).padStart(2, '0')}`
      : undefined;
  const subtitle = isEpisode
    ? item.episode_title
      ? `${epCode} · ${item.episode_title}`
      : epCode
    : undefined;

  const onClick = () => {
    if (isEpisode) {
      navigate({ to: '/show/$tmdbId', params: { tmdbId: String(item.tmdb_id) } });
    } else {
      navigate({ to: '/movie/$tmdbId', params: { tmdbId: String(item.tmdb_id) } });
    }
  };

  const onPlay = () => {
    if (isEpisode) {
      void playEpisode(item.library_id);
    } else {
      void playMovie(item.library_id);
    }
  };

  return (
    <div className="flex flex-col gap-1">
      <PosterCard
        title={item.title}
        posterUrl={posterUrl}
        blurhash={item.blurhash_poster}
        phase="available"
        watchProgress={item.progress_percent}
        onClick={onClick}
        onPlay={onPlay}
      />
      {subtitle && (
        <p className="px-1 text-[11px] text-[var(--text-muted)] truncate" title={subtitle}>
          {subtitle}
        </p>
      )}
    </div>
  );
}
