/**
 * Fetches + parses a WebVTT trickplay cue file of the form:
 *
 *   WEBVTT
 *
 *   00:00:00.000 --> 00:00:10.000
 *   /api/v1/playback/5/trickplay/sprite_001.jpg#xywh=0,0,160,90
 *
 * Returns the cue list plus `coveredSec` — the upper bound of
 * coverage, max(cue.end). For a fully generated library file this
 * equals the runtime; for a streaming source it advances as new
 * chunks land.
 *
 * `cueAt(seconds)` is a cheap linear scan — the typical cue count
 * is a few hundred so this is faster than binary search overhead.
 * Returns undefined past `coveredSec` so callers can render a
 * "Generating…" hint instead of a stale thumbnail.
 */

import { useEffect, useMemo, useRef, useState } from 'react';

export interface TrickplayCue {
  start: number;
  end: number;
  src: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Trickplay {
  cues: TrickplayCue[];
  /** Upper bound of generated coverage in seconds. Zero when no
   *  cues have been produced yet (streaming source still waiting). */
  coveredSec: number;
  cueAt(seconds: number): TrickplayCue | undefined;
}

function parseTime(s: string): number {
  const m = /^(\d+):(\d+):(\d+)\.(\d+)$/.exec(s.trim());
  if (!m) return 0;
  return Number(m[1]) * 3600 + Number(m[2]) * 60 + Number(m[3]) + Number(m[4]) / 1000;
}

export function parseTrickplayVtt(text: string): TrickplayCue[] {
  const cues: TrickplayCue[] = [];
  const lines = text.split(/\r?\n/);

  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const arrow = line?.includes('-->') ? line : undefined;
    if (!arrow) {
      i++;
      continue;
    }

    const [a, b] = arrow.split('-->').map((x) => x.trim());
    const urlLine = lines[i + 1] ?? '';
    const hashIdx = urlLine.indexOf('#xywh=');
    if (hashIdx < 0) {
      i += 2;
      continue;
    }
    // Sprites are pulled by the browser as CSS background images —
    // no way to set Authorization headers. Cookie auto-attaches on
    // same-origin requests; cross-origin would need a signed URL
    // (wired through `mediaUrl` on the calling component).
    const src = urlLine.slice(0, hashIdx);
    const xywh = urlLine
      .slice(hashIdx + '#xywh='.length)
      .split(',')
      .map(Number);
    if (xywh.length !== 4 || xywh.some((n) => Number.isNaN(n))) {
      i += 2;
      continue;
    }

    cues.push({
      start: parseTime(a),
      end: parseTime(b),
      src,
      x: xywh[0],
      y: xywh[1],
      w: xywh[2],
      h: xywh[3],
    });
    i += 2;
  }
  return cues;
}

export interface UseTrickplayOpts {
  /**
   * Incrementing counter that forces a re-fetch of the VTT. Wire
   * this to a WS event (e.g. `trickplay_stream_updated`) so
   * streaming coverage grows in the UI without polling.
   */
  refreshSignal?: number;
}

export function useTrickplay(
  url: string | null | undefined,
  opts: UseTrickplayOpts = {}
): Trickplay | null {
  const { refreshSignal = 0 } = opts;
  const [cues, setCues] = useState<TrickplayCue[] | null>(null);
  // Tracks whether we've ever produced a successful parse for this
  // URL. Lets the effect keep existing cues on transient failures
  // (e.g. streaming VTT briefly 404's between regens) instead of
  // flashing back to the skeleton.
  const hasLoadedRef = useRef(false);

  // Reset the "have we loaded before" guard whenever the URL
  // changes — a different file's previous cues don't count.
  // biome-ignore lint/correctness/useExhaustiveDependencies: url is the trigger; we intentionally only act on it
  useEffect(() => {
    hasLoadedRef.current = false;
    setCues(null);
  }, [url]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshSignal is the external invalidation channel — mandatory for streaming refetch
  useEffect(() => {
    if (!url) return;
    let cancelled = false;
    (async () => {
      try {
        const resp = await fetch(url, {
          credentials: 'include',
        });
        if (!resp.ok) {
          if (!cancelled && !hasLoadedRef.current) setCues(null);
          return;
        }
        const text = await resp.text();
        if (!cancelled) {
          setCues(parseTrickplayVtt(text));
          hasLoadedRef.current = true;
        }
      } catch {
        if (!cancelled && !hasLoadedRef.current) setCues(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [url, refreshSignal]);

  return useMemo(() => {
    if (!url) return null;
    if (cues == null) {
      // URL exists but VTT hasn't been fetched yet — return an empty
      // trickplay so the UI can render the "Generating…" skeleton
      // instead of suppressing the hover preview altogether.
      return {
        cues: [],
        coveredSec: 0,
        cueAt: () => undefined,
      };
    }
    // Cues are produced by ffmpeg's `fps=1/N` filter, which emits
    // them in strictly increasing order. Max-end is then just the
    // last cue's end, and `cueAt` can binary-search.
    const coveredSec = cues.length > 0 ? cues[cues.length - 1].end : 0;
    return {
      cues,
      coveredSec,
      cueAt(seconds: number) {
        let lo = 0;
        let hi = cues.length - 1;
        while (lo <= hi) {
          const mid = (lo + hi) >> 1;
          const c = cues[mid];
          if (seconds < c.start) hi = mid - 1;
          else if (seconds >= c.end) lo = mid + 1;
          else return c;
        }
        return undefined;
      },
    };
  }, [url, cues]);
}
