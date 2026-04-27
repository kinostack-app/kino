/**
 * Calendar — upcoming episodes + movie releases in Month, Week, or
 * Agenda view. Driven by the `/api/v1/calendar` endpoint which
 * returns episode/movie entries enriched with status + progress so
 * each cell can render actionable controls without a second fetch.
 *
 * View persistence and filter state live in localStorage so the page
 * remembers the user's preference across sessions.
 *
 * Live updates come from the WebSocket: the calendar query is
 * invalidated on library/download events (see state/websocket.ts).
 */

import * as Popover from '@radix-ui/react-popover';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import {
  CalendarDays,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Download,
  Film,
  Flag,
  Link2,
  List,
  Play,
  Sparkles,
  Square,
  Tv,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { calendarOptions } from '@/api/generated/@tanstack/react-query.gen';
import {
  markEpisodeWatched,
  markMovieWatched,
  unmarkEpisodeWatched,
  unmarkMovieWatched,
} from '@/api/generated/sdk.gen';
import type { CalendarEntry } from '@/api/generated/types.gen';
import { useDocumentTitle } from '@/hooks/useDocumentTitle';
import { tmdbImage } from '@/lib/api';
import { cn } from '@/lib/utils';
import type { InvalidationRule } from '@/state/invalidation';
import { queryMatchesId } from '@/state/query-utils';
import { useMutationWithToast } from '@/state/use-mutation-with-toast';

/** Calendar rolls up aired+upcoming episodes + movies + their
 *  download progress — every library / download lifecycle event
 *  shifts a row, plus new-episode discoveries. `show_monitor_changed`
 *  covers Manage-dialog saves that flip `monitor_new_items` or
 *  `monitor_specials` — those can drop/add episodes from the
 *  calendar's `in_scope = 1` filter. */
const CALENDAR_INVALIDATED_BY: InvalidationRule[] = [
  'movie_added',
  'show_added',
  'imported',
  'upgraded',
  'content_removed',
  'search_started',
  'release_grabbed',
  'download_started',
  'download_complete',
  'download_failed',
  'download_cancelled',
  'download_paused',
  'download_resumed',
  'new_episode',
  'watched',
  'unwatched',
  'show_monitor_changed',
];

// ── Constants / helpers ───────────────────────────────────────────

const DAYS_FULL = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const DAYS_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const MONTHS = [
  'January',
  'February',
  'March',
  'April',
  'May',
  'June',
  'July',
  'August',
  'September',
  'October',
  'November',
  'December',
];

type ViewMode = 'month' | 'week' | 'agenda';
type KindFilter = 'all' | 'shows' | 'movies';

const LS_VIEW = 'kino.calendar.view';
const LS_KIND = 'kino.calendar.kind';
const LS_HIDE_UNMONITORED = 'kino.calendar.hideUnmonitored';

/** YYYY-MM-DD in local time. Avoids UTC drift at the day boundary —
 *  a 2am local episode that's 10pm-prev-day UTC still lives in the
 *  right cell. */
function isoDay(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

function startOfWeek(date: Date): Date {
  // Monday-start. ISO 8601 convention; matches what most TV-schedule
  // users expect (MLB/NFL/most seasons start Sundays, but episodic
  // TV weeks tend to break Sunday-night → Monday-morning).
  const d = new Date(date);
  const day = d.getDay();
  const diff = (day + 6) % 7;
  d.setDate(d.getDate() - diff);
  d.setHours(0, 0, 0, 0);
  return d;
}

function addDays(date: Date, n: number): Date {
  const d = new Date(date);
  d.setDate(d.getDate() + n);
  return d;
}

function addMonths(date: Date, n: number): Date {
  const d = new Date(date);
  d.setMonth(d.getMonth() + n);
  return d;
}

function sameDay(a: Date, b: Date): boolean {
  return (
    a.getDate() === b.getDate() &&
    a.getMonth() === b.getMonth() &&
    a.getFullYear() === b.getFullYear()
  );
}

function isPast(date: Date, today: Date): boolean {
  // "Past" = before the start of today.
  const t = new Date(today);
  t.setHours(0, 0, 0, 0);
  const d = new Date(date);
  d.setHours(0, 0, 0, 0);
  return d.getTime() < t.getTime();
}

/** 42 cells covering the full month — leading days from the previous
 *  month, trailing days from the next, so the grid is always 6 rows. */
function getMonthGrid(monthStart: Date): Date[] {
  const first = new Date(monthStart.getFullYear(), monthStart.getMonth(), 1);
  const gridStart = startOfWeek(first);
  return Array.from({ length: 42 }, (_, i) => addDays(gridStart, i));
}

function getWeekDates(weekStart: Date): Date[] {
  return Array.from({ length: 7 }, (_, i) => addDays(weekStart, i));
}

function humanDate(d: Date, today: Date): string {
  if (sameDay(d, today)) return 'Today';
  if (sameDay(d, addDays(today, 1))) return 'Tomorrow';
  if (sameDay(d, addDays(today, -1))) return 'Yesterday';
  const thisYear = d.getFullYear() === today.getFullYear();
  const dayName = DAYS_FULL[d.getDay()];
  const monthName = MONTHS[d.getMonth()].slice(0, 3);
  return thisYear
    ? `${dayName}, ${monthName} ${d.getDate()}`
    : `${dayName}, ${monthName} ${d.getDate()}, ${d.getFullYear()}`;
}

/** Stable key for a calendar entry. Uses server ids where present so
 *  list updates don't cause React key churn. */
function entryKey(e: CalendarEntry): string {
  if (e.item_type === 'episode') {
    return `ep-${e.episode_id ?? `${e.show_id}-${e.season_number}-${e.episode_number}`}`;
  }
  return `mv-${e.movie_id ?? e.tmdb_id ?? e.date}-${e.title}`;
}

// ── Root component ────────────────────────────────────────────────

export function Calendar() {
  useDocumentTitle('Calendar');
  const today = useMemo(() => new Date(), []);
  // `cursor` points at the currently-focused date: anchor for week
  // (the week containing it) and month (the month containing it).
  // Agenda uses it as the start date.
  const [cursor, setCursor] = useState(today);
  const [view, setView] = useState<ViewMode>(() => readView());
  const [kind, setKind] = useState<KindFilter>(() => readKind());
  const [hideUnmonitored, setHideUnmonitored] = useState<boolean>(() => readHideUnmonitored());

  useEffect(() => writeLs(LS_VIEW, view), [view]);
  useEffect(() => writeLs(LS_KIND, kind), [kind]);
  useEffect(
    () => writeLs(LS_HIDE_UNMONITORED, hideUnmonitored ? 'true' : 'false'),
    [hideUnmonitored]
  );

  // Fetch window: always a little wider than what's on screen so
  // edge cells have their leading/trailing overflow days populated.
  const [fetchStart, fetchEnd] = useMemo(() => {
    switch (view) {
      case 'month': {
        const grid = getMonthGrid(cursor);
        return [grid[0], grid[grid.length - 1]] as const;
      }
      case 'week': {
        const ws = startOfWeek(cursor);
        return [ws, addDays(ws, 6)] as const;
      }
      case 'agenda':
        // 45-day forward window from cursor — feels long enough to
        // scroll but short enough to keep payload small.
        return [cursor, addDays(cursor, 45)] as const;
    }
  }, [view, cursor]);

  const { data, isLoading } = useQuery({
    ...calendarOptions({ query: { start: isoDay(fetchStart), end: isoDay(fetchEnd) } }),
    staleTime: 60_000,
    placeholderData: (prev) => prev,
    meta: { invalidatedBy: CALENDAR_INVALIDATED_BY },
  });

  const filtered = useMemo(() => {
    const list = data ?? [];
    return list.filter((e) => {
      if (kind === 'shows' && e.item_type !== 'episode') return false;
      if (kind === 'movies' && e.item_type !== 'movie') return false;
      if (hideUnmonitored && e.status === 'unmonitored') return false;
      return true;
    });
  }, [data, kind, hideUnmonitored]);

  const byDay = useMemo(() => groupByDay(filtered), [filtered]);

  // ── Keyboard nav ──
  //
  // Arrow keys: one unit per view (day in month, day in week, day in
  // agenda). PgUp/PgDn: one unit up (month → month, week → week,
  // agenda → week). `t` resets to today.
  const stepUnit = useCallback(
    (delta: number) => {
      const unit = view === 'month' ? 7 : 1;
      setCursor((c) => addDays(c, delta * unit));
    },
    [view]
  );
  const stepPage = useCallback(
    (delta: number) => {
      setCursor((c) => (view === 'month' ? addMonths(c, delta) : addDays(c, delta * 7)));
    },
    [view]
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement)?.matches?.('input, textarea, [contenteditable]')) return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      switch (e.key) {
        case 'ArrowLeft':
          e.preventDefault();
          stepUnit(-1);
          break;
        case 'ArrowRight':
          e.preventDefault();
          stepUnit(1);
          break;
        case 'ArrowUp':
          e.preventDefault();
          stepUnit(view === 'month' ? -7 : -7);
          break;
        case 'ArrowDown':
          e.preventDefault();
          stepUnit(view === 'month' ? 7 : 7);
          break;
        case 'PageUp':
          e.preventDefault();
          stepPage(-1);
          break;
        case 'PageDown':
          e.preventDefault();
          stepPage(1);
          break;
        case 't':
        case 'T':
          setCursor(new Date());
          break;
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [stepUnit, stepPage, view]);

  const hasAny = filtered.length > 0;

  return (
    <div className="px-4 md:px-12 py-6 pb-24 md:pb-8 max-w-6xl mx-auto">
      <Header view={view} cursor={cursor} setCursor={setCursor} setView={setView}>
        <FiltersBar
          kind={kind}
          setKind={setKind}
          hideUnmonitored={hideUnmonitored}
          setHideUnmonitored={setHideUnmonitored}
        />
      </Header>

      {view === 'month' && (
        <MonthView cursor={cursor} today={today} byDay={byDay} onDayClick={setCursor} />
      )}
      {view === 'week' && <WeekView cursor={cursor} today={today} byDay={byDay} />}
      {view === 'agenda' && <AgendaView today={today} entries={filtered} />}

      {isLoading && !data && (
        <div className="flex items-center justify-center mt-8">
          <div className="w-5 h-5 border-2 border-white/20 border-t-white rounded-full animate-spin" />
        </div>
      )}
      {!isLoading && !hasAny && (
        <div className="flex flex-col items-center justify-center mt-12 text-center gap-4">
          <div className="w-16 h-16 rounded-full bg-white/5 grid place-items-center">
            <CalendarDays size={28} className="text-[var(--text-muted)]" />
          </div>
          <div>
            <p className="text-lg font-medium">Nothing scheduled</p>
            <p className="text-sm text-[var(--text-muted)] mt-1">
              Follow shows or add movies with future release dates
            </p>
          </div>
        </div>
      )}
    </div>
  );
}

// ── Header (title, month/year jump, view switcher, ICS, nav) ──────

function Header({
  view,
  cursor,
  setCursor,
  setView,
  children,
}: {
  view: ViewMode;
  cursor: Date;
  setCursor: (d: Date) => void;
  setView: (v: ViewMode) => void;
  children: React.ReactNode;
}) {
  const label = (() => {
    if (view === 'month') return `${MONTHS[cursor.getMonth()]} ${cursor.getFullYear()}`;
    if (view === 'week') {
      const ws = startOfWeek(cursor);
      const we = addDays(ws, 6);
      const same = ws.getMonth() === we.getMonth();
      if (same) {
        return `${MONTHS[ws.getMonth()].slice(0, 3)} ${ws.getDate()} – ${we.getDate()}`;
      }
      return `${MONTHS[ws.getMonth()].slice(0, 3)} ${ws.getDate()} – ${MONTHS[we.getMonth()].slice(0, 3)} ${we.getDate()}`;
    }
    return `${MONTHS[cursor.getMonth()].slice(0, 3)} ${cursor.getDate()}, ${cursor.getFullYear()}`;
  })();

  const back = () =>
    setCursor(
      view === 'month' ? addMonths(cursor, -1) : addDays(cursor, view === 'week' ? -7 : -14)
    );
  const forward = () =>
    setCursor(view === 'month' ? addMonths(cursor, 1) : addDays(cursor, view === 'week' ? 7 : 14));

  const isOnToday = sameDay(cursor, new Date());
  return (
    <div className="space-y-4 mb-5">
      <div className="flex items-center justify-between gap-3 flex-wrap">
        <div className="flex items-center gap-1 min-w-0">
          {/* Prev / picker / next — chevrons flank the picker so one
              cluster does both paging and jump-to-month. A separate
              Today button only appears when we're not already on it,
              so it stops consuming space the 95% of the time the user
              is already there. */}
          <button
            type="button"
            onClick={back}
            className="w-8 h-8 grid place-items-center rounded-lg hover:bg-white/10 transition"
            aria-label="Previous"
          >
            <ChevronLeft size={18} />
          </button>
          <MonthJump cursor={cursor} setCursor={setCursor}>
            <button
              type="button"
              className="flex items-center gap-1.5 text-lg sm:text-xl font-semibold tracking-tight hover:bg-white/5 transition rounded-md px-2 py-0.5"
              aria-label="Jump to month"
            >
              <span className="truncate">{label}</span>
              <ChevronDown size={16} className="text-[var(--text-muted)] flex-shrink-0" />
            </button>
          </MonthJump>
          <button
            type="button"
            onClick={forward}
            className="w-8 h-8 grid place-items-center rounded-lg hover:bg-white/10 transition"
            aria-label="Next"
          >
            <ChevronRight size={18} />
          </button>
          {!isOnToday && (
            <button
              type="button"
              onClick={() => setCursor(new Date())}
              className="ml-1 px-2.5 h-8 text-xs rounded-lg bg-white/5 hover:bg-white/10 transition font-medium"
            >
              Today
            </button>
          )}
        </div>

        <div className="flex items-center gap-2">
          <ViewSwitcher view={view} setView={setView} />
          <SubscribeButton />
        </div>
      </div>
      {children}
    </div>
  );
}

function ViewSwitcher({ view, setView }: { view: ViewMode; setView: (v: ViewMode) => void }) {
  const opts: Array<{ k: ViewMode; label: string; icon: React.ReactNode }> = [
    { k: 'month', label: 'Month', icon: <CalendarDays size={14} /> },
    { k: 'week', label: 'Week', icon: <Square size={14} /> },
    { k: 'agenda', label: 'Agenda', icon: <List size={14} /> },
  ];
  return (
    <div className="flex items-center gap-0.5 p-0.5 rounded-lg bg-white/5 ring-1 ring-white/5">
      {opts.map((o) => (
        <button
          key={o.k}
          type="button"
          onClick={() => setView(o.k)}
          aria-pressed={view === o.k}
          className={cn(
            'flex items-center gap-1.5 px-2.5 py-1 rounded text-xs font-medium transition',
            view === o.k
              ? 'bg-white/10 text-white'
              : 'text-[var(--text-muted)] hover:text-white hover:bg-white/5'
          )}
        >
          {o.icon}
          <span className="hidden sm:inline">{o.label}</span>
        </button>
      ))}
    </div>
  );
}

function MonthJump({
  cursor,
  setCursor,
  children,
}: {
  cursor: Date;
  setCursor: (d: Date) => void;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [year, setYear] = useState(cursor.getFullYear());
  useEffect(() => setYear(cursor.getFullYear()), [cursor]);

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>{children}</Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="start"
          sideOffset={6}
          className="z-40 w-64 rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-xl p-3"
        >
          <div className="flex items-center justify-between mb-2">
            <button
              type="button"
              onClick={() => setYear((y) => y - 1)}
              className="w-7 h-7 grid place-items-center rounded hover:bg-white/10"
              aria-label="Previous year"
            >
              <ChevronLeft size={14} />
            </button>
            <span className="text-sm font-semibold tabular-nums">{year}</span>
            <button
              type="button"
              onClick={() => setYear((y) => y + 1)}
              className="w-7 h-7 grid place-items-center rounded hover:bg-white/10"
              aria-label="Next year"
            >
              <ChevronRight size={14} />
            </button>
          </div>
          <div className="grid grid-cols-3 gap-1">
            {MONTHS.map((m, i) => {
              const isCurrent = i === cursor.getMonth() && year === cursor.getFullYear();
              return (
                <button
                  key={m}
                  type="button"
                  onClick={() => {
                    setCursor(new Date(year, i, 1));
                    setOpen(false);
                  }}
                  className={cn(
                    'px-2 py-1.5 rounded text-xs transition',
                    isCurrent
                      ? 'bg-[var(--accent)] text-white'
                      : 'hover:bg-white/10 text-[var(--text-secondary)]'
                  )}
                >
                  {m.slice(0, 3)}
                </button>
              );
            })}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

function SubscribeButton() {
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  // External calendar apps can't carry our cookie — they need a
  // credential in the URL. The right long-term answer is a dedicated
  // "calendar feed" CLI token that the user generates from
  // Settings → Devices and pastes into their calendar app. For now
  // the URL is just the bare endpoint so the copy-out works visually;
  // calendar apps will get a 401 until we ship the dedicated token
  // generator on this page.
  const url = useMemo(() => {
    const base = typeof window === 'undefined' ? '' : window.location.origin;
    return `${base}/api/v1/calendar.ics`;
  }, []);
  const copy = () => {
    void navigator.clipboard.writeText(url).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button
          type="button"
          aria-label="Subscribe in external calendar"
          className="h-8 px-2.5 inline-flex items-center gap-1.5 rounded-lg text-xs text-[var(--text-muted)] hover:text-white hover:bg-white/10 transition"
        >
          <Link2 size={14} />
          <span className="hidden sm:inline">Subscribe</span>
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          className="z-40 w-80 rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-xl p-4"
        >
          <p className="text-sm font-medium mb-1">Subscribe to your schedule</p>
          <p className="text-xs text-[var(--text-muted)] mb-3">
            Paste this URL into Google Calendar → Add → From URL, or Apple Calendar → New
            Subscription.
          </p>
          <div className="flex items-center gap-1.5">
            <input
              readOnly
              value={url}
              onFocus={(e) => e.currentTarget.select()}
              className="flex-1 px-2 py-1.5 rounded-md bg-black/40 text-[11px] font-mono truncate"
              aria-label="Calendar subscription URL"
            />
            <button
              type="button"
              onClick={copy}
              className="px-2.5 py-1.5 rounded-md bg-white/10 hover:bg-white/15 text-xs transition"
            >
              {copied ? 'Copied' : 'Copy'}
            </button>
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

function FiltersBar({
  kind,
  setKind,
  hideUnmonitored,
  setHideUnmonitored,
}: {
  kind: KindFilter;
  setKind: (k: KindFilter) => void;
  hideUnmonitored: boolean;
  setHideUnmonitored: (v: boolean) => void;
}) {
  const kinds: Array<{ k: KindFilter; label: string; icon: React.ReactNode }> = [
    { k: 'all', label: 'All', icon: null },
    { k: 'shows', label: 'Shows', icon: <Tv size={12} /> },
    { k: 'movies', label: 'Movies', icon: <Film size={12} /> },
  ];
  return (
    <div className="flex items-center gap-2 flex-wrap text-xs">
      <div className="flex items-center gap-0.5 p-0.5 rounded-lg bg-white/[0.03] ring-1 ring-white/5">
        {kinds.map((o) => (
          <button
            key={o.k}
            type="button"
            onClick={() => setKind(o.k)}
            aria-pressed={kind === o.k}
            className={cn(
              'flex items-center gap-1 px-2 py-0.5 rounded font-medium transition',
              kind === o.k ? 'bg-white/10 text-white' : 'text-[var(--text-muted)] hover:text-white'
            )}
          >
            {o.icon}
            {o.label}
          </button>
        ))}
      </div>
      <label className="inline-flex items-center gap-1.5 cursor-pointer select-none text-[var(--text-muted)] hover:text-white transition">
        <input
          type="checkbox"
          checked={hideUnmonitored}
          onChange={(e) => setHideUnmonitored(e.target.checked)}
          className="sr-only"
        />
        <span
          className={cn(
            'w-4 h-4 rounded grid place-items-center transition',
            hideUnmonitored
              ? 'bg-[var(--accent)] text-white'
              : 'ring-1 ring-white/15 bg-transparent'
          )}
        >
          {hideUnmonitored && <Check size={10} strokeWidth={3} />}
        </span>
        Hide unmonitored
      </label>
    </div>
  );
}

// ── Month view ────────────────────────────────────────────────────

function MonthView({
  cursor,
  today,
  byDay,
  onDayClick,
}: {
  cursor: Date;
  today: Date;
  byDay: Map<string, CalendarEntry[]>;
  onDayClick: (d: Date) => void;
}) {
  const dates = useMemo(() => getMonthGrid(cursor), [cursor]);
  const currentMonth = cursor.getMonth();

  return (
    <div className="rounded-xl overflow-hidden ring-1 ring-white/5">
      {/* Weekday header row */}
      <div className="grid grid-cols-7 bg-white/5">
        {DAYS_SHORT.slice(1)
          .concat(DAYS_SHORT[0])
          .map((d) => (
            <div
              key={d}
              className="px-2 py-2 text-[10px] uppercase tracking-wider text-[var(--text-muted)] text-center"
            >
              {d}
            </div>
          ))}
      </div>
      <div className="grid grid-cols-7 gap-px bg-white/5">
        {dates.map((date) => {
          const key = isoDay(date);
          const entries = byDay.get(key) ?? [];
          const inMonth = date.getMonth() === currentMonth;
          return (
            <MonthCell
              key={key}
              date={date}
              today={today}
              entries={entries}
              inMonth={inMonth}
              onDayClick={onDayClick}
            />
          );
        })}
      </div>
    </div>
  );
}

function MonthCell({
  date,
  today,
  entries,
  inMonth,
  onDayClick,
}: {
  date: Date;
  today: Date;
  entries: CalendarEntry[];
  inMonth: boolean;
  onDayClick: (d: Date) => void;
}) {
  const grouped = useMemo(() => collapseSameDaySeries(entries), [entries]);
  const isTodayCell = sameDay(date, today);
  const visibleLimit = 3;
  const overflow = grouped.length - visibleLimit;

  return (
    <div
      className={cn(
        'min-h-[110px] sm:min-h-[130px] p-1.5 bg-[var(--bg-primary)] flex flex-col gap-1',
        !inMonth && 'opacity-55',
        isTodayCell && 'bg-white/[0.04] ring-1 ring-[var(--accent)]/40 ring-inset'
      )}
    >
      <div className="flex items-center justify-between">
        <button
          type="button"
          onClick={() => onDayClick(date)}
          className={cn(
            'inline-flex items-center justify-center text-xs font-semibold rounded-full transition w-6 h-6',
            isTodayCell
              ? 'bg-[var(--accent)] text-white'
              : inMonth
                ? // In-month: near-white for legibility, slightly
                  // toned down so today still clearly stands out.
                  'hover:bg-white/10 text-white/90'
                : // Out-of-month leading/trailing days: dimmed via
                  // cell opacity + muted tone so they read as overflow.
                  'hover:bg-white/10 text-[var(--text-muted)]'
          )}
        >
          {date.getDate()}
        </button>
      </div>
      <div className="space-y-0.5 min-w-0">
        {grouped.slice(0, visibleLimit).map((g) => (
          <CalendarChip key={g.key} group={g} today={today} compact />
        ))}
        {overflow > 0 && (
          <DayOverflow date={date} today={today} entries={grouped}>
            <button
              type="button"
              className="w-full text-left text-[10px] px-1.5 py-0.5 rounded text-[var(--text-muted)] hover:text-white hover:bg-white/5 transition"
            >
              +{overflow} more
            </button>
          </DayOverflow>
        )}
      </div>
    </div>
  );
}

function DayOverflow({
  date,
  today,
  entries,
  children,
}: {
  date: Date;
  today: Date;
  entries: EntryGroup[];
  children: React.ReactNode;
}) {
  return (
    <Popover.Root>
      <Popover.Trigger asChild>{children}</Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="start"
          sideOffset={6}
          className="z-40 w-80 max-h-[70vh] overflow-y-auto rounded-xl bg-[var(--bg-secondary)] ring-1 ring-white/10 shadow-xl p-3"
        >
          <p className="text-xs font-semibold mb-2 text-[var(--text-muted)]">
            {humanDate(date, today)}
          </p>
          <div className="space-y-1.5">
            {entries.map((g) => (
              <CalendarChip key={g.key} group={g} today={today} />
            ))}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

// ── Week view ─────────────────────────────────────────────────────

function WeekView({
  cursor,
  today,
  byDay,
}: {
  cursor: Date;
  today: Date;
  byDay: Map<string, CalendarEntry[]>;
}) {
  const dates = useMemo(() => getWeekDates(startOfWeek(cursor)), [cursor]);
  return (
    <div className="space-y-3 md:space-y-0 md:grid md:grid-cols-7 md:gap-px md:bg-white/5 md:rounded-xl md:overflow-hidden md:ring-1 md:ring-white/5">
      {dates.map((date, i) => {
        const key = isoDay(date);
        const entries = byDay.get(key) ?? [];
        const grouped = collapseSameDaySeries(entries);
        const isTodayCell = sameDay(date, today);
        return (
          <div
            key={key}
            className={cn(
              'md:min-h-[180px] md:p-3 md:bg-[var(--bg-primary)]',
              'rounded-lg ring-1 ring-white/5 p-3 bg-[var(--bg-primary)] md:ring-0',
              isTodayCell && 'md:bg-white/[0.04]'
            )}
          >
            <div className="flex items-center gap-2 mb-2">
              <span className="text-[10px] text-[var(--text-muted)] uppercase tracking-wider">
                {DAYS_SHORT[(i + 1) % 7]}
              </span>
              <span
                className={cn(
                  'text-sm font-semibold',
                  isTodayCell
                    ? 'w-6 h-6 rounded-full bg-[var(--accent)] text-white grid place-items-center text-xs'
                    : 'text-white/90'
                )}
              >
                {date.getDate()}
              </span>
            </div>
            <div className="space-y-1.5">
              {grouped.length === 0 ? (
                <p className="text-[11px] text-[var(--text-muted)] opacity-60">—</p>
              ) : (
                grouped.map((g) => <CalendarChip key={g.key} group={g} today={today} />)
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}

// ── Agenda view ───────────────────────────────────────────────────

function AgendaView({ today, entries }: { today: Date; entries: CalendarEntry[] }) {
  // Group by date (yyyy-mm-dd); already sorted by the backend.
  const days = useMemo(() => {
    const m = new Map<string, CalendarEntry[]>();
    for (const e of entries) {
      const k = e.date.slice(0, 10);
      const list = m.get(k);
      if (list) list.push(e);
      else m.set(k, [e]);
    }
    return [...m.entries()].map(([date, items]) => ({
      date,
      items: collapseSameDaySeries(items),
    }));
  }, [entries]);

  if (days.length === 0) return null;
  return (
    <div className="space-y-6">
      {days.map(({ date, items }) => {
        const d = parseIsoDay(date);
        const past = isPast(d, today);
        return (
          <section key={date} aria-labelledby={`agenda-${date}`}>
            <h2
              id={`agenda-${date}`}
              className={cn(
                'text-xs uppercase tracking-wider font-semibold mb-2',
                past ? 'text-[var(--text-muted)]' : 'text-white'
              )}
            >
              {humanDate(d, today)}
            </h2>
            <div className="space-y-1.5">
              {items.map((g) => (
                <CalendarChip key={g.key} group={g} today={today} expanded />
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}

function parseIsoDay(s: string): Date {
  const [y, m, d] = s.split('-').map(Number);
  return new Date(y, (m ?? 1) - 1, d ?? 1);
}

// ── Same-day grouping ─────────────────────────────────────────────

interface EntryGroup {
  key: string;
  /** Primary entry (first of the group). */
  head: CalendarEntry;
  /** Extra episodes when a show has multiple episodes the same day. */
  tail: CalendarEntry[];
}

function collapseSameDaySeries(entries: CalendarEntry[]): EntryGroup[] {
  const groups: EntryGroup[] = [];
  const showIdx = new Map<number, number>();
  for (const e of entries) {
    if (e.item_type === 'episode' && e.show_id != null) {
      const existing = showIdx.get(e.show_id);
      if (existing != null) {
        groups[existing].tail.push(e);
        continue;
      }
      showIdx.set(e.show_id, groups.length);
    }
    groups.push({ key: entryKey(e), head: e, tail: [] });
  }
  // Sort episodes within each group by episode_number for a stable range.
  for (const g of groups) {
    if (g.tail.length > 0) {
      const all = [g.head, ...g.tail].sort(
        (a, b) => (a.episode_number ?? 0) - (b.episode_number ?? 0)
      );
      g.head = all[0];
      g.tail = all.slice(1);
    }
  }
  return groups;
}

// ── Calendar chip (card) ──────────────────────────────────────────

function CalendarChip({
  group,
  today,
  compact,
  expanded,
}: {
  group: EntryGroup;
  today: Date;
  compact?: boolean;
  expanded?: boolean;
}) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { head, tail } = group;

  const onClick = () => {
    if (head.item_type === 'movie' && head.tmdb_id) {
      navigate({ to: '/movie/$tmdbId', params: { tmdbId: String(head.tmdb_id) } });
    } else if (head.item_type === 'episode' && head.tmdb_id) {
      navigate({ to: '/show/$tmdbId', params: { tmdbId: String(head.tmdb_id) } });
    }
  };

  const status = head.status ?? 'wanted';
  const date = parseIsoDay(head.date);
  // `past` is currently unused here — earlier we dimmed past-and-
  // unwatched chips, but for a view that's mostly "this month," a
  // past-date episode is more often an awareness signal (catch up
  // on this!) than something to hide. Keep the variable in case we
  // want per-view logic later.
  void isPast(date, today);
  const isEpisode = head.item_type === 'episode';
  const isWatched = status === 'watched';

  const tint = tintForStatus(status);

  const label = isEpisode
    ? tail.length > 0
      ? `S${two(head.season_number)}E${two(head.episode_number)}–E${two(
          tail.at(-1)?.episode_number ?? head.episode_number ?? 0
        )}`
      : `S${two(head.season_number)}E${two(head.episode_number)}`
    : 'Release';

  const markWatched = useMutationWithToast({
    verb: 'mark watched',
    mutationFn: async () => {
      if (isEpisode && head.episode_id != null) {
        await markEpisodeWatched({ path: { id: head.episode_id } });
      } else if (!isEpisode && head.movie_id != null) {
        await markMovieWatched({ path: { id: head.movie_id } });
      }
    },
    onSuccess: () => qc.invalidateQueries({ predicate: (q) => queryMatchesId(q, 'calendar') }),
  });
  const unmarkWatched = useMutationWithToast({
    verb: 'unmark watched',
    mutationFn: async () => {
      if (isEpisode && head.episode_id != null) {
        await unmarkEpisodeWatched({ path: { id: head.episode_id } });
      } else if (!isEpisode && head.movie_id != null) {
        await unmarkMovieWatched({ path: { id: head.movie_id } });
      }
    },
    onSuccess: () => qc.invalidateQueries({ predicate: (q) => queryMatchesId(q, 'calendar') }),
  });

  const canPlay = status === 'available' && head.media_id != null;
  const downloadPercent = head.download_percent ?? 0;

  return (
    // biome-ignore lint/a11y/useSemanticElements: the chip wraps absolutely-positioned children (poster, watch progress bar, hover-action overlay). A real <button> would inherit form-button defaults that fight this layout. role+tabIndex+onKeyDown gives the same a11y surface
    <div
      className={cn(
        'relative group/chip w-full flex items-center gap-2 rounded border text-left transition hover:brightness-110 cursor-pointer',
        tint,
        compact ? 'px-1.5 py-1' : 'px-2 py-1.5',
        expanded && 'py-2.5'
      )}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onClick();
        }
      }}
      role="button"
      tabIndex={0}
    >
      {/* Poster — rendered at render size (w92 source ≈ 46x69 display). */}
      {head.poster_path ? (
        <img
          src={tmdbImage(head.poster_path, 'w92') ?? undefined}
          alt=""
          className={cn(
            'rounded-sm object-cover flex-shrink-0',
            compact ? 'w-5 h-7' : expanded ? 'w-11 h-16' : 'w-7 h-10'
          )}
          loading="lazy"
        />
      ) : (
        <div
          className={cn(
            'grid place-items-center text-[var(--text-muted)] flex-shrink-0 rounded-sm bg-white/5',
            compact ? 'w-5 h-7' : expanded ? 'w-11 h-16' : 'w-7 h-10'
          )}
        >
          {isEpisode ? <Tv size={12} /> : <Film size={12} />}
        </div>
      )}

      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5 min-w-0">
          <p
            className={cn(
              'font-medium truncate leading-tight',
              compact ? 'text-[11px]' : 'text-xs',
              expanded && 'text-sm'
            )}
          >
            {isEpisode ? head.show_title : head.title}
          </p>
          {head.is_premiere && (
            <Sparkles
              size={compact ? 10 : 11}
              className="flex-shrink-0 text-amber-400"
              aria-label="Premiere"
            />
          )}
          {head.is_finale && !head.is_premiere && (
            <Flag
              size={compact ? 10 : 11}
              className="flex-shrink-0 text-rose-400"
              aria-label="Finale"
            />
          )}
        </div>
        <p
          className={cn(
            'text-[var(--text-muted)] truncate leading-tight',
            compact ? 'text-[10px]' : 'text-[11px]'
          )}
        >
          {label}
          {expanded && isEpisode && head.episode_title ? ` · ${head.episode_title}` : ''}
        </p>
        {/* Thin progress bar for in-flight downloads — hugs the bottom
            edge of the chip, reusing the downloaded_percent we already
            ship on the entry. */}
        {status === 'downloading' && (
          <div className="mt-1 h-0.5 rounded-full bg-white/10 overflow-hidden">
            <div
              className="h-full bg-blue-400 rounded-full transition-all duration-500"
              style={{ width: `${Math.max(2, downloadPercent)}%` }}
            />
          </div>
        )}
      </div>

      {/* Action column — only when there's real estate. Play on
          hover when available; mark-watched chevron always present. */}
      {!compact && (canPlay || (isEpisode && head.episode_id != null) || head.movie_id != null) && (
        <div className="flex items-center gap-0.5 flex-shrink-0 opacity-0 group-hover/chip:opacity-100 focus-within:opacity-100 transition-opacity">
          {canPlay && (
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                if (head.episode_id != null) {
                  navigate({
                    to: '/play/$kind/$entityId',
                    params: { kind: 'episode', entityId: String(head.episode_id) },
                    search: {},
                  });
                } else if (head.movie_id != null) {
                  navigate({
                    to: '/play/$kind/$entityId',
                    params: { kind: 'movie', entityId: String(head.movie_id) },
                    search: {},
                  });
                }
              }}
              aria-label="Play"
              className="w-6 h-6 grid place-items-center rounded-full bg-white/10 hover:bg-white/20"
            >
              <Play size={10} fill="currentColor" className="ml-0.5" />
            </button>
          )}
          {status === 'downloading' && (
            // biome-ignore lint/a11y/useSemanticElements: role="status" on a span is the documented a11y pattern for live-region announcements; <output> isn't a one-to-one swap on a span used as inline content
            <span
              role="status"
              aria-label={`Downloading ${downloadPercent}%`}
              className="h-6 px-1.5 grid place-items-center rounded-full bg-white/10 text-[10px] tabular-nums inline-flex items-center gap-1"
            >
              <Download size={10} />
              {downloadPercent}%
            </span>
          )}
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              if (isWatched) unmarkWatched.mutate();
              else markWatched.mutate();
            }}
            aria-label={isWatched ? 'Mark unwatched' : 'Mark watched'}
            className={cn(
              'w-6 h-6 grid place-items-center rounded-full',
              isWatched
                ? 'bg-emerald-500/30 text-emerald-300 hover:bg-emerald-500/50'
                : 'bg-white/10 hover:bg-white/20'
            )}
          >
            <Check size={10} strokeWidth={3} />
          </button>
        </div>
      )}
    </div>
  );
}

function two(n: number | null | undefined): string {
  return String(n ?? 0).padStart(2, '0');
}

function tintForStatus(status: string): string {
  switch (status) {
    case 'watched':
      // Watched chips fade back a touch — the user has already
      // consumed this item, so they shouldn't compete visually with
      // unwatched ones.
      return 'border-emerald-500/25 bg-emerald-500/10 opacity-75';
    case 'available':
      return 'border-emerald-500/50 bg-emerald-500/15';
    case 'downloading':
      return 'border-blue-500/50 bg-blue-500/15';
    case 'unmonitored':
      return 'border-white/5 bg-white/[0.03] opacity-60';
    default:
      // Wanted — scheduler will pick it up but nothing's happened
      // yet. Subtle but clearly present.
      return 'border-white/15 bg-white/10';
  }
}

// ── localStorage helpers ──────────────────────────────────────────

function readView(): ViewMode {
  try {
    const raw = localStorage.getItem(LS_VIEW);
    if (raw === 'month' || raw === 'week' || raw === 'agenda') return raw;
  } catch {
    // ignore
  }
  return 'month';
}
function readKind(): KindFilter {
  try {
    const raw = localStorage.getItem(LS_KIND);
    if (raw === 'all' || raw === 'shows' || raw === 'movies') return raw;
  } catch {
    // ignore
  }
  return 'all';
}
function readHideUnmonitored(): boolean {
  try {
    return localStorage.getItem(LS_HIDE_UNMONITORED) === 'true';
  } catch {
    return false;
  }
}
function writeLs(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // private mode — silent
  }
}

function groupByDay(entries: CalendarEntry[]): Map<string, CalendarEntry[]> {
  const m = new Map<string, CalendarEntry[]>();
  for (const e of entries) {
    const key = e.date.slice(0, 10);
    const list = m.get(key);
    if (list) list.push(e);
    else m.set(key, [e]);
  }
  return m;
}
