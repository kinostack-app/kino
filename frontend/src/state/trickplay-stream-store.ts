/**
 * Per-download refresh counters for streaming trickplay.
 *
 * The websocket forwarder bumps the counter whenever the backend
 * emits `trickplay_stream_updated`; `useTrickplay` reads it as its
 * `refreshSignal` so the VTT is re-fetched and the new cues show up
 * on hover. Keyed by `download_id` so multiple concurrent streams
 * don't invalidate each other.
 */

import { create } from 'zustand';

interface State {
  ticks: Record<number, number>;
  bump: (downloadId: number) => void;
}

export const useTrickplayStreamStore = create<State>((set) => ({
  ticks: {},
  bump: (downloadId) =>
    set((s) => ({ ticks: { ...s.ticks, [downloadId]: (s.ticks[downloadId] ?? 0) + 1 } })),
}));

export function useTrickplayTick(downloadId: number | null | undefined): number {
  return useTrickplayStreamStore((s) => (downloadId == null ? 0 : (s.ticks[downloadId] ?? 0)));
}
