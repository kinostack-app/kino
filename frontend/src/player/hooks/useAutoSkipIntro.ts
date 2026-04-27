/**
 * Smart-mode auto-skip for TV intros (subsystem #15).
 *
 * Fires when the playhead enters an intro range and the current
 * `auto_skip_intros` config says to skip. A silent seek to
 * `intro_end_ms` is followed by a 3-second `Intro skipped · Undo`
 * toast (spec §8). Two undos in one playback session disable
 * auto-skip for the remainder of that session — if the user has
 * gone out of their way twice to watch the intro, we stop fighting
 * them.
 *
 * Modes:
 *   - `off`   → hook is a no-op (button still renders via SkipButton)
 *   - `on`    → always auto-skip on intro entry
 *   - `smart` → auto-skip when the user has seen this season's intro
 *               before — either historically (`seasonAnyWatched`) or
 *               earlier in the current session (tracked locally).
 */

import { useEffect, useRef } from 'react';
import { kinoToast } from '@/components/kino-toast';
import type { VideoSource } from '@/player/types';

/** Per-session memory of which `show:season` keys have had their
 *  intro watched through. Mutating a module-level Set is deliberate:
 *  React state would force re-renders we don't want here. */
const sessionSeenSeasons = new Set<string>();
let sessionUndoCount = 0;

function seasonKey(showId?: number | null, seasonNumber?: number | null): string | null {
  if (showId == null || seasonNumber == null) return null;
  return `${showId}:${seasonNumber}`;
}

function shouldAutoSkip(source: VideoSource): boolean {
  if (sessionUndoCount >= 2) return false;
  const mode = source.autoSkipIntros;
  if (mode === 'on') return true;
  if (mode !== 'smart') return false;
  if (source.seasonAnyWatched) return true;
  const key = seasonKey(source.showId, source.seasonNumber);
  return key ? sessionSeenSeasons.has(key) : false;
}

export function useAutoSkipIntro({
  currentTime,
  source,
  onSeek,
}: {
  currentTime: number;
  source: VideoSource | null;
  onSeek: (seconds: number) => void;
}) {
  /** Last-seen time so we can detect the rising-edge crossing into
   *  the intro range without firing on every timeupdate. */
  const lastTimeRef = useRef(0);
  /** Episode-local suppression — set when the user hits Undo so we
   *  don't immediately auto-skip again when the playhead re-enters
   *  the range. Cleared on source change. */
  const suppressedForEpisodeRef = useRef(false);
  /** Tracks whether we have (re-)entered the range this episode
   *  so we can detect "played through without skipping" → mark the
   *  season as seen for smart mode. */
  const insideIntroRef = useRef(false);

  // Reset episode-scoped state when the source (episode) changes.
  const sourceKey = source ? `${source.url}` : null;
  // biome-ignore lint/correctness/useExhaustiveDependencies: refs are stable handles; deps would defeat reset semantics
  useEffect(() => {
    suppressedForEpisodeRef.current = false;
    insideIntroRef.current = false;
    lastTimeRef.current = 0;
  }, [sourceKey]);

  useEffect(() => {
    if (!source) return;
    const startSec = source.introStartMs != null ? source.introStartMs / 1000 : null;
    const endSec = source.introEndMs != null ? source.introEndMs / 1000 : null;
    if (startSec == null || endSec == null || endSec <= startSec) {
      lastTimeRef.current = currentTime;
      return;
    }
    if (source.skipEnabledForShow === false) {
      lastTimeRef.current = currentTime;
      return;
    }

    const prev = lastTimeRef.current;
    const now = currentTime;

    // Rising-edge cross into the range — this is the auto-skip
    // trigger. Guard against seek-backwards + re-entry via the
    // episode-local suppression flag.
    const enteredRange = prev < startSec && now >= startSec && now < endSec;
    if (enteredRange) {
      insideIntroRef.current = true;
      if (!suppressedForEpisodeRef.current && shouldAutoSkip(source)) {
        const preSkipSec = prev;
        onSeek(endSec);
        const undoToastId = kinoToast.info('Intro skipped', {
          duration: 3000,
          action: {
            label: 'Undo',
            onClick: () => {
              kinoToast.dismiss(undoToastId);
              suppressedForEpisodeRef.current = true;
              sessionUndoCount += 1;
              onSeek(Math.max(0, preSkipSec));
            },
          },
        });
        // Mark season as seen — we auto-skipped it, which is our
        // strongest signal that the user knows the intro.
        const key = seasonKey(source.showId, source.seasonNumber);
        if (key) sessionSeenSeasons.add(key);
      }
    }

    // Falling-edge leave of the range while we never skipped = the
    // intro played through. Record the season so future episodes
    // this session auto-skip under `smart`.
    const leftRangeForward = insideIntroRef.current && prev < endSec && now >= endSec;
    if (leftRangeForward) {
      insideIntroRef.current = false;
      const key = seasonKey(source.showId, source.seasonNumber);
      if (key) sessionSeenSeasons.add(key);
    }

    lastTimeRef.current = now;
  }, [currentTime, source, onSeek]);
}
