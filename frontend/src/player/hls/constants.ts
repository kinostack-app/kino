/**
 * Two-tier retry budget for hls.js fatal errors.
 *
 * `NETWORK_ERROR`s get exponential backoff up to
 * `MAX_NETWORK_RETRIES`; `MEDIA_ERROR`s get one
 * `recoverMediaError()` pass. A successful manifest parse
 * or fragment load resets both budgets — so a transient
 * blip on an otherwise healthy session doesn't permanently
 * narrow the retry window for a later genuine failure.
 *
 * Exponential schedule: 1s, 2s, 4s. Anything past that is
 * unlikely to self-heal; we escalate to MSE recovery →
 * fatal. Modelled on the Shaka Player pattern described in
 * our `docs/subsystems/05-playback.md` retry matrix.
 */
export const MAX_NETWORK_RETRIES = 3;
export const MAX_MEDIA_RECOVERIES = 1;
export const BACKOFF_BASE_MS = 1000;
