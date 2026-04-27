import { useEffect, useRef } from 'react';
import type { VideoSource } from '../types';

/**
 * When the active audio-language changes, auto-enable any
 * `is_forced` text-based subtitle in the new language — the
 * convention for foreign-dialogue translations ("signs &
 * songs" on anime, alien-language overlays on sci-fi).
 *
 * Only fires when the user hasn't made a subtitle selection
 * yet, so we never override an explicit pick. Returns a ref
 * the picker bumps to lock auto-selection out.
 */
export function useAutoForcedSubtitles(
  source: VideoSource | null,
  setSubtitleStreamIndex: (idx: number | null) => void
) {
  const userHasPickedRef = useRef(false);

  const activeAudioLang = source?.audioTracks?.find(
    (t) => t.stream_index === source.audioStreamIndex
  )?.language;

  useEffect(() => {
    if (userHasPickedRef.current) return;
    if (!activeAudioLang) return;
    const forced = source?.subtitles?.find(
      (s) => s.is_forced && s.vtt_url != null && s.language === activeAudioLang
    );
    if (forced) {
      setSubtitleStreamIndex(forced.stream_index);
    }
  }, [activeAudioLang, source?.subtitles, setSubtitleStreamIndex]);

  return userHasPickedRef;
}
