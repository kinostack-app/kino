/**
 * Server-side Chromecast session state.
 *
 * The browser doesn't run the Cast SDK any more — kino's backend
 * speaks the Cast protocol directly via `rust_cast` (subsystem 32).
 * This store is a reactive mirror over kino's HTTP + WebSocket
 * surface:
 *
 *   - `bootstrap()` fetches the device list once on mount.
 *   - `selectDevice(id)` POSTs `/api/v1/cast/sessions` to launch the
 *     receiver app and load media.
 *   - Worker-thread MEDIA_STATUS frames flow back as
 *     `cast_status` AppEvents on the WebSocket and patch this store.
 *   - Transport controls (`play` / `pause` / `seekTo`) POST to
 *     `/api/v1/cast/sessions/{id}/{play|pause|seek}`.
 *
 * Result: same Zustand-driven mini-controller / overlay UI works
 * unchanged in Firefox + Safari, which never had `chrome.cast.*`.
 */

import { create } from 'zustand';
import {
  addDevice as apiAddDevice,
  deleteDevice as apiDeleteDevice,
  listDevices as apiListDevices,
  pause as apiPause,
  play as apiPlay,
  seek as apiSeek,
  startSession as apiStartSession,
  stopSession as apiStopSession,
} from '@/api/generated/sdk.gen';
import type { CastDevice } from '@/api/generated/types.gen';

/** UI state derived from session + device list. */
export type CastState = 'idle' | 'no_devices' | 'connecting' | 'connected' | 'ending';

interface CastMedia {
  mediaId: number;
  title: string;
}

interface ParsedStatus {
  isPaused: boolean;
  positionSec: number;
}

export interface CastStore {
  /** Initialised flag — `false` until first bootstrap completes. */
  ready: boolean;

  state: CastState;
  devices: CastDevice[];
  /** Currently-selected device when state ≠ idle. */
  deviceId: string | null;
  deviceName: string | null;

  /**
   * BBC iPlayer-style pre-connect: the user picks a target device
   * from the header popover BEFORE opening any media. When set, the
   * next playback that lands on a media row with `castMediaId` is
   * routed to this device automatically — no second click on the
   * in-player Cast button. Persists across page reloads via
   * localStorage so a Cast-first user doesn't have to reselect on
   * every navigation. Cleared when the user explicitly disconnects
   * from the popover, or when the device disappears from the LAN
   * for long enough to drop out of `devices`.
   */
  preselectedDeviceId: string | null;

  /** Active session id (server-issued UUID). Null when idle. */
  sessionId: string | null;
  media: CastMedia | null;

  /** Mirrored from MEDIA_STATUS events. */
  isPaused: boolean;
  currentTimeSec: number;

  /**
   * Position (in receiver time, seconds) captured at the moment a
   * Cast session ends. PlayerRoot consumes this on the
   * connected → idle transition and seeks local playback to it so
   * the viewer picks up where the TV left off. Cleared by
   * `consumePendingResume()`.
   */
  pendingResumeSec: number | null;

  bootstrap: () => Promise<void>;
  refreshDevices: () => Promise<void>;
  addDevice: (input: { ip: string; name?: string }) => Promise<CastDevice | null>;
  forgetDevice: (id: string) => Promise<void>;
  selectDevice: (deviceId: string, media: SelectMediaInput) => Promise<void>;
  /** Pre-connect to a device. Pass `null` to clear the preselection. */
  preselectDevice: (deviceId: string | null) => void;
  endSession: () => Promise<void>;

  play: () => Promise<void>;
  pause: () => Promise<void>;
  playOrPause: () => Promise<void>;
  seekTo: (sec: number) => Promise<void>;

  /**
   * Atomic read-and-clear of `pendingResumeSec`. Returns `null`
   * if nothing's pending. PlayerRoot calls this on the
   * connected → idle transition so the resume seek doesn't
   * fight a subsequent user seek.
   */
  consumePendingResume: () => number | null;

  // ─── Internal: WebSocket dispatcher hooks. ───
  /** Apply a `cast_status` AppEvent to the store. */
  _applyStatus: (sessionId: string, positionMs: number | null, statusJson: string) => void;
  /** Apply a `cast_session_ended` AppEvent. */
  _applyEnded: (sessionId: string) => void;
}

export interface SelectMediaInput {
  mediaId: number;
  title: string;
  startTimeSec?: number;
}

function deviceById(devices: CastDevice[], id: string | null): CastDevice | null {
  if (id == null) return null;
  return devices.find((d) => d.id === id) ?? null;
}

function parseStatusJson(raw: string): ParsedStatus | null {
  try {
    const parsed = JSON.parse(raw) as { player_state?: string; current_time_sec?: number };
    const ps = (parsed.player_state ?? '').toUpperCase();
    return {
      isPaused: ps === 'PAUSED' || ps === 'IDLE',
      positionSec: parsed.current_time_sec ?? 0,
    };
  } catch {
    return null;
  }
}

const PRESELECT_STORAGE_KEY = 'kino:cast:preselectedDeviceId';

function loadPreselect(): string | null {
  if (typeof window === 'undefined') return null;
  try {
    return window.localStorage.getItem(PRESELECT_STORAGE_KEY);
  } catch {
    return null;
  }
}

function savePreselect(id: string | null) {
  if (typeof window === 'undefined') return;
  try {
    if (id == null) window.localStorage.removeItem(PRESELECT_STORAGE_KEY);
    else window.localStorage.setItem(PRESELECT_STORAGE_KEY, id);
  } catch {
    // localStorage can throw in private mode / quota errors — non-fatal.
  }
}

export const useCastStore = create<CastStore>((set, get) => ({
  ready: false,
  state: 'idle',
  devices: [],
  deviceId: null,
  deviceName: null,
  preselectedDeviceId: loadPreselect(),
  sessionId: null,
  media: null,
  isPaused: true,
  currentTimeSec: 0,
  pendingResumeSec: null,

  bootstrap: async () => {
    if (get().ready) return;
    await get().refreshDevices();
    set({ ready: true });
  },

  refreshDevices: async () => {
    try {
      const { data } = await apiListDevices();
      const devices = data ?? [];
      set((s) => ({
        devices,
        // If the active device disappeared, pretend it's still
        // listed so the UI shows "Casting · X" instead of
        // collapsing mid-session. Backend events drive the actual
        // disconnect.
        state: s.devices.length === 0 && devices.length === 0 ? 'no_devices' : s.state,
      }));
    } catch (err) {
      console.warn('[cast] listDevices failed:', err);
    }
  },

  addDevice: async ({ ip, name }) => {
    try {
      const { data } = await apiAddDevice({
        body: { ip, name: name ?? null, port: null },
      });
      if (!data) return null;
      // Optimistic insert ahead of the next refreshDevices.
      set((s) => ({ devices: [data, ...s.devices.filter((d) => d.id !== data.id)] }));
      return data;
    } catch (err) {
      console.warn('[cast] addDevice failed:', err);
      return null;
    }
  },

  forgetDevice: async (id) => {
    try {
      await apiDeleteDevice({ path: { id } });
      set((s) => ({ devices: s.devices.filter((d) => d.id !== id) }));
    } catch (err) {
      console.warn('[cast] deleteDevice failed:', err);
    }
  },

  selectDevice: async (deviceId, media) => {
    const device = deviceById(get().devices, deviceId);
    if (!device) return;
    set({
      state: 'connecting',
      deviceId,
      deviceName: device.name,
      media: { mediaId: media.mediaId, title: media.title },
      currentTimeSec: media.startTimeSec ?? 0,
      isPaused: false,
    });
    try {
      const { data } = await apiStartSession({
        body: {
          device_id: deviceId,
          media_id: media.mediaId,
          start_position_ms:
            media.startTimeSec && media.startTimeSec > 0
              ? Math.round(media.startTimeSec * 1000)
              : null,
        },
      });
      if (!data) {
        set({ state: 'idle', sessionId: null, deviceId: null, deviceName: null, media: null });
        return;
      }
      set({
        sessionId: data.id,
        state: data.status === 'errored' ? 'idle' : 'connected',
      });
    } catch (err) {
      console.warn('[cast] startSession failed:', err);
      set({ state: 'idle', sessionId: null, deviceId: null, deviceName: null, media: null });
    }
  },

  preselectDevice: (deviceId) => {
    savePreselect(deviceId);
    set({ preselectedDeviceId: deviceId });
  },

  endSession: async () => {
    const id = get().sessionId;
    if (!id) return;
    set({ state: 'ending' });
    try {
      await apiStopSession({ path: { id } });
    } catch (err) {
      console.warn('[cast] stopSession failed:', err);
    }
    // Hard-reset locally — the cast_session_ended WS event will
    // arrive eventually and is idempotent w.r.t. the cleared state.
    const { currentTimeSec } = get();
    set({
      state: 'idle',
      sessionId: null,
      deviceId: null,
      deviceName: null,
      media: null,
      isPaused: true,
      currentTimeSec: 0,
      pendingResumeSec: currentTimeSec > 0 ? currentTimeSec : null,
    });
  },

  play: async () => {
    const id = get().sessionId;
    if (!id) return;
    try {
      await apiPlay({ path: { id } });
      set({ isPaused: false });
    } catch (err) {
      console.warn('[cast] play failed:', err);
    }
  },

  pause: async () => {
    const id = get().sessionId;
    if (!id) return;
    try {
      await apiPause({ path: { id } });
      set({ isPaused: true });
    } catch (err) {
      console.warn('[cast] pause failed:', err);
    }
  },

  playOrPause: async () => {
    if (get().isPaused) await get().play();
    else await get().pause();
  },

  seekTo: async (sec) => {
    const id = get().sessionId;
    if (!id) return;
    try {
      await apiSeek({ path: { id }, body: { position_ms: Math.round(sec * 1000) } });
      set({ currentTimeSec: sec });
    } catch (err) {
      console.warn('[cast] seek failed:', err);
    }
  },

  consumePendingResume: () => {
    const pending = get().pendingResumeSec;
    if (pending == null) return null;
    set({ pendingResumeSec: null });
    return pending;
  },

  _applyStatus: (sessionId, positionMs, statusJson) => {
    const current = get().sessionId;
    if (current !== sessionId) return; // Stale event from a previous session.
    const parsed = parseStatusJson(statusJson);
    set({
      currentTimeSec: positionMs != null ? positionMs / 1000 : (parsed?.positionSec ?? 0),
      isPaused: parsed?.isPaused ?? get().isPaused,
      // First Status frame after launch confirms we're live.
      state: 'connected',
    });
  },

  _applyEnded: (sessionId) => {
    if (get().sessionId !== sessionId) return;
    const { currentTimeSec } = get();
    set({
      state: 'idle',
      sessionId: null,
      deviceId: null,
      deviceName: null,
      media: null,
      isPaused: true,
      currentTimeSec: 0,
      pendingResumeSec: currentTimeSec > 0 ? currentTimeSec : null,
    });
  },
}));
