/**
 * Canonical `/play/$kind/$entityId` route component. Thin wrapper
 * that validates the kind param and hands off to PlayerRoot.
 */

import { useNavigate, useParams, useSearch } from '@tanstack/react-router';
import type { PlayKind } from '@/api/generated/types.gen';
import { PlayerRoot } from '@/player/PlayerRoot';

export function UnifiedPlayerRoute() {
  const { kind, entityId } = useParams({ from: '/play/$kind/$entityId' });
  const { resume_at } = useSearch({ from: '/play/$kind/$entityId' });
  const navigate = useNavigate();

  // Validate kind — the route matcher accepts any string; this
  // catches typos like `/play/movies/42` (note the 's') that would
  // otherwise reach the backend as a 404.
  if (kind !== 'movie' && kind !== 'episode') {
    void navigate({ to: '/', replace: true });
    return null;
  }

  return (
    <PlayerRoot kind={kind as PlayKind} entityId={Number(entityId)} initialResumeAt={resume_at} />
  );
}
