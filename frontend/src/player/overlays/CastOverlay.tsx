/**
 * "Now casting on $device" take-over card.
 *
 * Shown while a Cast session is active — local playback is
 * paused and the receiver owns the screen. Tapping our
 * custom Cast button in the control bar ends the session
 * and resumes here.
 */
export function CastOverlay({ deviceName }: { deviceName: string | null }) {
  return (
    <div className="absolute inset-0 z-30 flex items-center justify-center bg-black/80">
      <div className="text-center space-y-3 max-w-sm px-4">
        <p className="text-[10px] uppercase tracking-wider text-[var(--accent)] font-semibold">
          Casting
        </p>
        <p className="text-xl font-medium text-white">{deviceName ?? 'Chromecast'}</p>
        <p className="text-sm text-white/60">
          Playback continues on your TV. Tap the Cast button to stop and resume here.
        </p>
      </div>
    </div>
  );
}
