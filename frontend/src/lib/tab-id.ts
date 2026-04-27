/**
 * Per-tab nonce used to segregate backend transcode sessions. Two
 * browser tabs playing the same download/media would otherwise share
 * `session_id = stream-{id}` (or `transcode-{id}`) on the backend and
 * stop each other's ffmpeg on every master.m3u8 refetch. Threading a
 * tab-local nonce through the URL fixes that.
 *
 * `sessionStorage` is per-tab by design — survives F5 within the same
 * tab (so the resumed session keeps its ffmpeg), clears on tab close
 * (so the next tab starts fresh).
 */

const KEY = 'kino-tab-id';

export function getTabId(): string {
  try {
    let id = sessionStorage.getItem(KEY);
    if (!id) {
      // 8 chars of base36 — enough to avoid collisions across open
      // tabs on the same machine; short enough to keep URLs readable.
      id = Math.random().toString(36).slice(2, 10);
      sessionStorage.setItem(KEY, id);
    }
    return id;
  } catch {
    // sessionStorage can throw in private / locked-down contexts.
    // An in-memory fallback still gives this mount a stable id for
    // its lifetime; cross-tab collision would return, but so would
    // basic app functionality, so ok.
    return Math.random().toString(36).slice(2, 10);
  }
}
