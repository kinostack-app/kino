import * as Dialog from '@radix-ui/react-dialog';
import { Check, Sparkles, Tv, X } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type { MonitorNewItems, SeasonAcquireState } from '@/api/generated/types.gen';
import { cn } from '@/lib/utils';
import type { AddContentOptions } from '@/state/use-content-state';

/**
 * Follow Show dialog — shown when the user clicks Add on a TV show.
 *
 * Two independent decisions:
 *
 *   1. Monitor future episodes? A single checkbox. When checked,
 *      `show.monitor_new_items = 'future'` — new episodes get auto-
 *      grabbed as they air. Unchecked → `'none'`, the user grabs
 *      episodes manually via the card's + button.
 *
 *   2. What should it download right now? Drives the per-episode
 *      `acquire` flag — "all seasons", "latest season" (default),
 *      "specific seasons" with a checklist, "no past episodes".
 *
 * Both captured on submit as `AddContentOptions` that the caller
 * passes through to `state.add(options)`.
 */

interface Season {
  season_number: number;
  name: string | null;
  episode_count: number | null;
  air_date: string | null;
}

interface FollowShowDialogProps {
  /** Controlled-mount flag — Radix handles the portal + focus trap
   *  + Escape via onOpenChange; the caller just flips this boolean. */
  open: boolean;
  showTitle: string;
  seasons: Season[];
  /** Re-open case: after a show is already followed, the dialog
   *  doubles as a "Manage downloads" surface. Switches the title
   *  and CTA language from "Follow" to "Update." */
  isAlreadyFollowed?: boolean;
  /** Current monitor-new-items setting; pre-fills the top radio on
   *  re-open so the user sees their existing preference rather than
   *  a default that might differ. */
  currentMonitorNewItems?: MonitorNewItems;
  /** Per-season acquire/library state from the backend's
   *  `/shows/{id}/monitored-seasons` endpoint. Drives the tri-state
   *  checkboxes: a season with all episodes `acquire = 1` is pre-
   *  checked; a season that has *some* acquired or imported (e.g.
   *  from a Play-auto-follow stream) renders indeterminate so the
   *  user can see "this isn't starting from scratch." Pass undefined
   *  on first-time Follow. */
  seasonStates?: SeasonAcquireState[];
  /** Whether the show has at least one Season 0 ("Specials")
   *  episode on TMDB. Controls visibility of the Include-Specials
   *  checkbox — no point showing it for a show with no specials. */
  hasSpecials: boolean;
  /** Current `show.monitor_specials` value on a re-open (Manage).
   *  Undefined on a fresh follow (defaults to off). */
  currentMonitorSpecials?: boolean;
  onConfirm: (options: AddContentOptions) => void;
  onCancel: () => void;
  isLoading?: boolean;
}

type DownloadMode = 'all' | 'latest' | 'specific' | 'none';

/** Map the stored enum to the dialog's single checkbox. Checked →
 *  `'future'` (auto-grab new episodes). Unchecked → `'none'` (track
 *  only). Defaults to ON when the show isn't in the library yet —
 *  most users want monitoring when they follow. */
function monitorFutureFromEnum(v: MonitorNewItems | undefined): boolean {
  if (v == null) return true;
  return v !== 'none';
}
function monitorEnumFromFuture(on: boolean): MonitorNewItems {
  return on ? 'future' : 'none';
}

function partialLabel(stat: SeasonAcquireState): string {
  const incoming = Math.max(0, stat.acquiring - stat.in_library);
  if (stat.in_library > 0 && incoming > 0) {
    return `${stat.in_library} in library · ${incoming} incoming`;
  }
  if (stat.in_library > 0) return `${stat.in_library}/${stat.total} in library`;
  if (incoming > 0) return `${incoming}/${stat.total} incoming`;
  return '';
}

export function FollowShowDialog({
  open,
  showTitle,
  seasons,
  isAlreadyFollowed,
  currentMonitorNewItems,
  seasonStates,
  hasSpecials,
  currentMonitorSpecials,
  onConfirm,
  onCancel,
  isLoading,
}: FollowShowDialogProps) {
  const latestSeason = useMemo(() => {
    // Prefer the season with the newest air_date; fall back to the
    // highest season number when air dates aren't available. Specials
    // (season 0) are excluded — "Latest" means the most recent *real*
    // season; specials have their own axis.
    const sorted = [...seasons]
      .filter((s) => s.season_number > 0)
      .sort((a, b) => {
        const at = a.air_date ? Date.parse(a.air_date) : 0;
        const bt = b.air_date ? Date.parse(b.air_date) : 0;
        if (at !== bt) return bt - at;
        return b.season_number - a.season_number;
      });
    return sorted[0]?.season_number;
  }, [seasons]);

  const [monitorFuture, setMonitorFuture] = useState<boolean>(
    monitorFutureFromEnum(currentMonitorNewItems)
  );
  const [monitorSpecials, setMonitorSpecials] = useState<boolean>(currentMonitorSpecials ?? false);
  // Past-specials axis — orthogonal to `monitorSpecials` (which is
  // forward-only). Checked → Season 0 is in the effective
  // seasons_to_monitor list, past-aired specials get grabbed like
  // any listed season. Defaults to whatever Season 0 currently looks
  // like on re-open (a season with any acquiring episodes is "on"),
  // else off.
  const seasonZeroAcquiring = (seasonStates ?? []).find(
    (s) => s.season_number === 0 && s.acquiring > 0
  );
  const [pastSpecials, setPastSpecials] = useState<boolean>(
    Boolean(isAlreadyFollowed && seasonZeroAcquiring)
  );
  // On re-open (Manage downloads) the dialog doubles as "edit your
  // existing selection"; on a brand-new follow it's a clean sheet.
  // Fall back to 'specific' when there's no real season to be "latest"
  // of (specials-only shows) — the 'latest' radio is hidden in that
  // case and would otherwise leave the dialog with no selection.
  const [download, setDownload] = useState<DownloadMode>(
    isAlreadyFollowed ? 'specific' : latestSeason != null ? 'latest' : 'specific'
  );
  // Seasons fully monitored right now, derived from the stats payload.
  // "Fully" = every episode in the season has acquire = 1; seasons in
  // a partial state don't count as "checked" (they render
  // indeterminate below). On new follow there's no state yet so the
  // Set is empty and the user's clicks are additive from scratch.
  const fullyMonitored = useMemo(
    () =>
      new Set(
        (seasonStates ?? [])
          .filter((s) => s.total > 0 && s.acquiring >= s.total)
          .map((s) => s.season_number)
      ),
    [seasonStates]
  );
  const [specific, setSpecific] = useState<Set<number>>(
    () => new Set(isAlreadyFollowed ? fullyMonitored : [])
  );

  // Re-seed when the stats lands asynchronously — Manage opens before
  // the fetch for monitored-seasons has resolved, so seeded state
  // needs to catch up when data arrives.
  useEffect(() => {
    if (isAlreadyFollowed && seasonStates != null) {
      setSpecific(new Set(fullyMonitored));
      // Mirror for past-specials: detect Season 0 with any acquiring
      // episodes and seed the checkbox on.
      setPastSpecials(Boolean(seasonStates.find((s) => s.season_number === 0 && s.acquiring > 0)));
    }
  }, [isAlreadyFollowed, seasonStates, fullyMonitored]);

  // Quick lookup from season_number → state for per-row rendering.
  const stateByNumber = useMemo(() => {
    const m = new Map<number, SeasonAcquireState>();
    for (const s of seasonStates ?? []) m.set(s.season_number, s);
    return m;
  }, [seasonStates]);
  // Monitor (checkbox) and download (radio) are now genuinely
  // independent decisions — no need to suggest one from the other.
  // The old 3-way monitor radio made `all` and `future` visually
  // distinct but behaviourally identical for 99% of shows, which
  // required a `suggestMonitor(downloadMode)` helper to auto-sync
  // them and a `monitorTouched` flag to preserve manual picks. All
  // gone: checkbox state is the monitor state.

  const toggleSpecific = (num: number) => {
    setSpecific((prev) => {
      const next = new Set(prev);
      if (next.has(num)) next.delete(num);
      else next.add(num);
      return next;
    });
  };

  const handleConfirm = () => {
    // Build the effective seasons list. Season 0 is injected when
    // the user's ticked the past-specials affordance (either the
    // "All + specials" sub-checkbox or the "Specials" row in the
    // "Specific" checklist). Latest-only and "no past" stay as they
    // were — specials don't fit their semantics.
    const nonZeroSeasons = seasons
      .map((s) => s.season_number)
      .filter((n) => n > 0)
      .sort((a, b) => a - b);
    const specifics = [...specific].filter((n) => n !== 0).sort((a, b) => a - b);
    const withSpecials = (list: number[]) => (pastSpecials && hasSpecials ? [...list, 0] : list);
    const seasonsToMonitor: number[] =
      download === 'all'
        ? withSpecials(nonZeroSeasons)
        : download === 'latest' && latestSeason != null
          ? [latestSeason]
          : download === 'specific'
            ? withSpecials(specifics)
            : [];
    onConfirm({
      monitorNewItems: monitorEnumFromFuture(monitorFuture),
      seasonsToMonitor,
      monitorSpecials: hasSpecials ? monitorSpecials : undefined,
    });
  };

  const specialsSuffix = pastSpecials && hasSpecials ? ' + specials' : '';
  const downloadSummary =
    download === 'all'
      ? `${seasons.length} season${seasons.length !== 1 ? 's' : ''}${specialsSuffix}`
      : download === 'latest'
        ? latestSeason != null
          ? `Season ${latestSeason}`
          : 'none'
        : download === 'specific'
          ? specific.size === 0 && pastSpecials
            ? 'specials only'
            : `${specific.size} season${specific.size !== 1 ? 's' : ''}${specialsSuffix}`
          : 'nothing';

  // "Specific" with zero seasons is normally a nonsense selection —
  // nothing to save. Exceptions: the future-monitor specials flag
  // is on (specials-only future monitoring), or the past-specials
  // checkbox is on (specials-only download). Only block when every
  // axis is empty.
  const confirmDisabled =
    isLoading ||
    (download === 'specific' && specific.size === 0 && !monitorSpecials && !pastSpecials);

  const title = isAlreadyFollowed ? 'Manage tracking' : 'Follow Show';
  const confirmLabel = isAlreadyFollowed ? 'Update' : 'Follow Show';
  const confirmPending = isAlreadyFollowed ? 'Updating\u2026' : 'Following\u2026';

  return (
    // Radix Dialog: focus trap, Escape-to-close, portal, backdrop
    // click handling, aria-labelledby/-describedby wiring are all
    // free. Earlier hand-rolled version had none of this — tab could
    // escape behind the modal, Escape didn't dismiss, and the muted
    // backdrop button duplicated the X button's role for screen
    // readers. Radix handles all of that.
    <Dialog.Root open={open} onOpenChange={(next) => (next ? null : onCancel())}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/70 backdrop-blur-sm data-[state=open]:animate-in data-[state=open]:fade-in-0" />
        <Dialog.Content className="fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2 w-[min(28rem,calc(100vw-2rem))] max-h-[min(calc(100vh-4rem),40rem)] flex flex-col bg-[var(--bg-secondary)] rounded-xl ring-1 ring-white/10 shadow-2xl overflow-hidden data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95">
          <div className="flex items-center justify-between px-5 py-4 border-b border-white/5">
            <div className="min-w-0">
              <Dialog.Title className="text-lg font-semibold">{title}</Dialog.Title>
              <Dialog.Description className="text-sm text-[var(--text-muted)] mt-0.5 truncate">
                {showTitle}
              </Dialog.Description>
            </div>
            <Dialog.Close asChild>
              <button
                type="button"
                className="p-1.5 rounded-lg hover:bg-white/10 text-[var(--text-muted)] hover:text-white transition"
                aria-label="Close"
              >
                <X size={18} />
              </button>
            </Dialog.Close>
          </div>

          <div className="px-5 py-5 space-y-6 overflow-y-auto flex-1">
            <Section
              icon={<Sparkles size={13} className="text-[var(--accent)]" />}
              title="Monitor future episodes"
              description="Auto-grab new episodes as they air, matching your quality profile."
            >
              <label className="flex items-start gap-3 cursor-pointer group">
                <input
                  type="checkbox"
                  checked={monitorFuture}
                  onChange={(e) => setMonitorFuture(e.target.checked)}
                  className="mt-0.5 w-4 h-4 accent-[var(--accent)]"
                />
                <div className="text-sm">
                  <div className="text-white">Monitor future episodes</div>
                  <div className="text-xs text-[var(--text-muted)]">
                    {monitorFuture
                      ? 'New episodes will download automatically as they air.'
                      : "New episodes won't auto-download. Grab them manually from the episode list."}
                  </div>
                </div>
              </label>
              {/* Specials are a separate axis — shows like The Boys
                  drop weekly shorts that'd otherwise clog Next Up and
                  the calendar. Off by default. Only rendered when the
                  show actually has a Season 0 on TMDB (no point
                  showing the toggle otherwise). */}
              {hasSpecials && (
                <label className="mt-3 flex items-start gap-3 cursor-pointer group">
                  <input
                    type="checkbox"
                    checked={monitorSpecials}
                    onChange={(e) => setMonitorSpecials(e.target.checked)}
                    className="mt-0.5 w-4 h-4 accent-[var(--accent)]"
                  />
                  <div className="text-sm">
                    <div className="text-white">Include specials</div>
                    <div className="text-xs text-[var(--text-muted)]">
                      {monitorSpecials
                        ? 'Season 0 episodes will be tracked and surface in Next Up / calendar.'
                        : 'Season 0 (specials) stays hidden from progress and Next Up. Grab individual specials manually.'}
                    </div>
                  </div>
                </label>
              )}
            </Section>

            <Section
              icon={<Tv size={13} className="text-[var(--accent)]" />}
              title="Download existing episodes"
              description="What to fetch now for seasons that have already aired."
            >
              <Radio
                label={`All seasons (${seasons.length})`}
                checked={download === 'all'}
                onChange={() => setDownload('all')}
              />
              {/* "All" + specials — sub-checkbox so users can opt
                  past specials in without picking "Specific." Off by
                  default; only shown when the show has a Season 0
                  and "All" is the active radio. */}
              {download === 'all' && hasSpecials && (
                <label className="mt-1 ml-7 flex items-center gap-2 cursor-pointer group">
                  <input
                    type="checkbox"
                    checked={pastSpecials}
                    onChange={(e) => setPastSpecials(e.target.checked)}
                    className="w-4 h-4 accent-[var(--accent)]"
                  />
                  <span className="text-sm text-[var(--text-secondary)] group-hover:text-white transition-colors">
                    Include specials (Season 0)
                  </span>
                </label>
              )}
              {latestSeason != null && (
                <Radio
                  label={`Latest season only (Season ${latestSeason})`}
                  checked={download === 'latest'}
                  onChange={() => setDownload('latest')}
                />
              )}
              <Radio
                label="Specific seasons"
                checked={download === 'specific'}
                onChange={() => setDownload('specific')}
              />
              {download === 'specific' && (
                <div className="mt-2 border-l-2 border-[var(--accent)]/30 pl-4 space-y-1 max-h-60 overflow-y-auto">
                  {seasons.map((s) => {
                    const checked = specific.has(s.season_number);
                    const stat = stateByNumber.get(s.season_number);
                    // Indeterminate = "not fully monitored but has
                    // some state." Shows up when the user streamed an
                    // episode via Play, or did a specific-seasons
                    // pick that covers only some episodes of this
                    // season. Clicking flips to fully checked — the
                    // dash is purely visual.
                    const partial =
                      !checked && stat != null && (stat.acquiring > 0 || stat.in_library > 0);
                    const detail = partial ? partialLabel(stat) : undefined;
                    return (
                      <label
                        key={s.season_number}
                        className="flex items-center gap-3 py-1.5 cursor-pointer"
                      >
                        <span
                          className={cn(
                            'w-4 h-4 rounded grid place-items-center flex-shrink-0 transition',
                            checked
                              ? 'bg-[var(--accent)] text-white'
                              : partial
                                ? 'bg-[var(--accent)]/20 text-[var(--accent)] ring-1 ring-[var(--accent)]/40'
                                : 'ring-1 ring-white/20 text-transparent'
                          )}
                        >
                          {checked ? (
                            <Check size={10} strokeWidth={3} />
                          ) : partial ? (
                            <span className="w-1.5 h-1.5 rounded-full bg-[var(--accent)]" />
                          ) : (
                            <Check size={10} strokeWidth={3} />
                          )}
                        </span>
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() => toggleSpecific(s.season_number)}
                          className="sr-only"
                        />
                        <span className="flex-1 text-sm">
                          {s.name ?? `Season ${s.season_number}`}
                          {detail && (
                            <span className="ml-2 text-[10px] text-[var(--accent)]/80">
                              {detail}
                            </span>
                          )}
                        </span>
                        {s.episode_count != null && (
                          <span className="text-xs text-[var(--text-muted)]">
                            {s.episode_count} ep{s.episode_count !== 1 ? 's' : ''}
                          </span>
                        )}
                      </label>
                    );
                  })}
                  {/* Specials row — rendered at the bottom of the
                      checklist (after real seasons). Binds to the
                      same `pastSpecials` state the "All + specials"
                      sub-checkbox uses; handleConfirm injects 0
                      into seasons_to_monitor when it's on. */}
                  {hasSpecials && (
                    <label className="flex items-center gap-3 py-1.5 cursor-pointer border-t border-white/5 mt-1 pt-2">
                      <span
                        className={cn(
                          'w-4 h-4 rounded grid place-items-center flex-shrink-0 transition',
                          pastSpecials
                            ? 'bg-[var(--accent)] text-white'
                            : 'ring-1 ring-white/20 text-transparent'
                        )}
                      >
                        <Check size={10} strokeWidth={3} />
                      </span>
                      <input
                        type="checkbox"
                        checked={pastSpecials}
                        onChange={(e) => setPastSpecials(e.target.checked)}
                        className="sr-only"
                      />
                      <span className="flex-1 text-sm">Specials (Season 0)</span>
                    </label>
                  )}
                </div>
              )}
              <Radio
                label="No past episodes"
                hint="Skip the backfill. If Monitor future is on, new episodes still get grabbed as they air."
                checked={download === 'none'}
                onChange={() => setDownload('none')}
              />
            </Section>
          </div>

          <div className="flex items-center justify-between gap-3 px-5 py-4 border-t border-white/5 bg-white/[0.02]">
            <p className="text-[11px] text-[var(--text-muted)]">
              Downloading: <span className="text-white">{downloadSummary}</span>
            </p>
            <div className="flex gap-2">
              <button
                type="button"
                onClick={onCancel}
                className="px-4 py-2 rounded-lg text-sm font-medium text-[var(--text-secondary)] hover:text-white hover:bg-white/10 transition"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={handleConfirm}
                disabled={confirmDisabled}
                className="px-5 py-2 rounded-lg text-sm font-semibold bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white disabled:opacity-50 transition"
              >
                {isLoading ? confirmPending : confirmLabel}
              </button>
            </div>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function Section({
  icon,
  title,
  description,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  description: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="flex items-center gap-2 mb-1">
        {icon}
        <h3 className="text-sm font-semibold text-white">{title}</h3>
      </div>
      <p className="text-xs text-[var(--text-muted)] mb-3">{description}</p>
      <div className="space-y-1">{children}</div>
    </div>
  );
}

function Radio({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint?: string;
  checked: boolean;
  onChange: () => void;
}) {
  return (
    <label className="flex items-start gap-3 py-1.5 cursor-pointer group">
      <span
        className={cn(
          'mt-0.5 w-4 h-4 rounded-full ring-2 grid place-items-center flex-shrink-0 transition',
          checked ? 'ring-[var(--accent)]' : 'ring-white/20 group-hover:ring-white/40'
        )}
      >
        {checked && <span className="w-2 h-2 rounded-full bg-[var(--accent)]" />}
      </span>
      <input type="radio" checked={checked} onChange={onChange} className="sr-only" />
      <span className="flex-1 min-w-0">
        <span className="text-sm text-white block">{label}</span>
        {hint && <span className="text-[11px] text-[var(--text-muted)] block mt-0.5">{hint}</span>}
      </span>
    </label>
  );
}
