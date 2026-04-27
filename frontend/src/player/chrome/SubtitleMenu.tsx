import { Captions } from 'lucide-react';
import { useCallback, useMemo, useState } from 'react';
import type { SubtitleTrack } from '@/api/generated/types.gen';
import { cn } from '@/lib/utils';
import type { VideoSource } from '../types';
import { TrackPickerDialog, type TrackPickerDialogItem } from './TrackPickerDialog';

/**
 * Subtitle picker.
 *
 * Clicking the button opens a centered modal dialog with
 * search and rich track badges instead of the old
 * popover-menu design. The dialog pauses the video on
 * open and resumes on close if the user was playing —
 * subtitles exist to be read, and reading while playing
 * means missing dialog.
 *
 * Handles both text subtitles (rendered client-side via
 * native `<track>` and the `subtitleStreamIndex` state
 * flip) and image-based subtitles (PGS/VOBSUB/DVB, which
 * must burn-in server-side via `onBurnInSubtitleChange`).
 * Both paths surface in the same list with a "burn-in"
 * badge on image entries so the user knows why that pick
 * restarts the transcode.
 *
 * `userHasPickedRef` is bumped on every interaction so the
 * auto-forced-subtitle effect stops overriding the user's
 * choice.
 */
export function SubtitleMenu({
  source,
  subtitleStreamIndex,
  setSubtitleStreamIndex,
  userHasPickedRef,
  onRequestPause,
  onRequestResume,
}: {
  source: VideoSource | null;
  subtitleStreamIndex: number | null;
  setSubtitleStreamIndex: (idx: number | null) => void;
  userHasPickedRef: React.RefObject<boolean>;
  /** Called when the dialog opens. Return `true` if the
   *  video was playing (so we know whether to resume on
   *  close). Host owns pause/resume side-effects. */
  onRequestPause?: () => boolean;
  onRequestResume?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [wasPlaying, setWasPlaying] = useState(false);

  const onOpen = useCallback(() => {
    setWasPlaying(onRequestPause?.() ?? false);
    setOpen(true);
  }, [onRequestPause]);

  const onClose = useCallback(() => {
    setOpen(false);
    if (wasPlaying) onRequestResume?.();
  }, [wasPlaying, onRequestResume]);

  const items = useMemo<TrackPickerDialogItem<SubtitleTrack>[]>(() => {
    if (!source?.subtitles) return [];
    const browserLangs = (navigator.languages ?? [navigator.language ?? '']).map((l) =>
      l.slice(0, 2).toLowerCase()
    );
    // Group membership: "Recommended" when the track's
    // language matches a browser-preferred language (rough
    // heuristic — good enough for "English speakers see
    // English subs first"). Empty group shows under no
    // header, next to the "Recommended" group.
    const recommended = (sub: SubtitleTrack) =>
      sub.language ? browserLangs.includes(sub.language.slice(0, 2).toLowerCase()) : false;

    // Sort: recommended first, then default, then by label.
    const sorted = [...source.subtitles].sort((a, b) => {
      const aR = recommended(a) ? 1 : 0;
      const bR = recommended(b) ? 1 : 0;
      if (aR !== bR) return bR - aR;
      if (a.is_default !== b.is_default) return a.is_default ? -1 : 1;
      return a.label.localeCompare(b.label);
    });
    return sorted.map((sub) => ({
      key: sub.stream_index,
      data: sub,
      group: recommended(sub) ? 'Recommended' : 'Other',
    }));
  }, [source?.subtitles]);

  if (!source?.subtitles || source.subtitles.length === 0) return null;

  const hasAny = source.subtitles.length > 0;
  const handleSelect = (key: number | string | null) => {
    userHasPickedRef.current = true;
    if (key === null) {
      setSubtitleStreamIndex(null);
      source.onBurnInSubtitleChange?.(null);
      return;
    }
    const idx = Number(key);
    const sub = source.subtitles?.find((s) => s.stream_index === idx);
    if (!sub) return;
    const burnIn = sub.vtt_url == null;
    setSubtitleStreamIndex(idx);
    source.onBurnInSubtitleChange?.(burnIn ? idx : null);
  };

  return (
    <>
      <button
        type="button"
        onClick={onOpen}
        aria-label="Subtitles"
        aria-haspopup="dialog"
        aria-expanded={open}
        disabled={!hasAny}
        className={cn(
          'w-9 h-9 grid place-items-center rounded-lg transition',
          subtitleStreamIndex != null
            ? 'bg-white/15 text-white'
            : 'hover:bg-white/10 text-white/70',
          !hasAny && 'opacity-40 cursor-not-allowed'
        )}
      >
        <Captions size={18} />
      </button>
      {open && (
        <TrackPickerDialog<SubtitleTrack>
          title="Subtitles"
          items={items}
          selectedKey={subtitleStreamIndex}
          searchText={(sub) =>
            [sub.label, sub.language, sub.title, sub.codec].filter(Boolean).join(' ')
          }
          render={(sub) => <SubtitleRow sub={sub} />}
          onSelect={handleSelect}
          offLabel="Off"
          onClose={onClose}
        />
      )}
    </>
  );
}

/**
 * Single-row layout for a subtitle. Language flag slot
 * on the left, label in the middle, badges on the right.
 * Keeps the row at a readable density (>= 44px tap
 * target) while still surfacing every useful piece of
 * metadata at a glance.
 */
function SubtitleRow({ sub }: { sub: SubtitleTrack }) {
  const lang = sub.language ?? null;
  const title = sub.title ?? null;
  // Split the canonical `label` into its leading language
  // component + remainder so we can render them with
  // different weights. Labels today look like
  // "English (forced)" or "Spanish SDH" — the opening
  // word is the part worth emphasising.
  const [head, ...tail] = sub.label.split(/[\s·]/);
  const primary = head || sub.label;
  const rest = tail.join(' ');

  return (
    <span className="flex items-center gap-2 w-full min-w-0">
      {lang && (
        <span className="shrink-0 w-8 text-center text-[10px] font-mono uppercase tracking-wider text-white/50 rounded bg-white/5 py-0.5">
          {lang}
        </span>
      )}
      <span className="flex-1 min-w-0 flex flex-col">
        <span className="font-medium truncate">{primary}</span>
        {(rest || title) && <span className="text-xs text-white/50 truncate">{rest || title}</span>}
      </span>
      <SubtitleBadges sub={sub} />
    </span>
  );
}

function SubtitleBadges({ sub }: { sub: SubtitleTrack }) {
  const badges: Array<{ label: string; tone: 'neutral' | 'amber' | 'blue' }> = [];
  if (sub.vtt_url == null) badges.push({ label: 'Burn-in', tone: 'amber' });
  if (sub.is_forced) badges.push({ label: 'Forced', tone: 'blue' });
  if (sub.is_hearing_impaired) badges.push({ label: 'SDH', tone: 'neutral' });
  if (sub.is_commentary) badges.push({ label: 'Commentary', tone: 'neutral' });
  if (sub.is_external) badges.push({ label: 'External', tone: 'neutral' });
  if (badges.length === 0) return null;
  return (
    <span className="flex items-center gap-1 shrink-0">
      {badges.map((b) => (
        <span
          key={b.label}
          className={cn(
            'text-[10px] font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded',
            b.tone === 'amber' && 'bg-amber-500/15 text-amber-300',
            b.tone === 'blue' && 'bg-blue-500/15 text-blue-300',
            b.tone === 'neutral' && 'bg-white/5 text-white/50'
          )}
        >
          {b.label}
        </span>
      ))}
    </span>
  );
}
