/**
 * Context-specific media error overlay.
 *
 * We deliberately don't use Media Chrome's built-in
 * `<media-error-dialog>` — its messages are generic
 * ("This video format isn't supported") while ours are
 * tailored to the kino failure modes ("This file's
 * container isn't playable in the browser. It'll play
 * from the library once import completes."). The HLS
 * retry exhaustion path also surfaces through here.
 *
 * Shown above the controller; dismiss via the Back button.
 */
export function MediaErrorOverlay({ message, onBack }: { message: string; onBack: () => void }) {
  return (
    <div className="absolute inset-0 flex items-center justify-center bg-black/70 backdrop-blur-sm p-6 z-40">
      <div className="max-w-md text-center space-y-4">
        <p className="text-lg font-medium">Can&apos;t play this file in the browser</p>
        <p className="text-sm text-white/70">{message}</p>
        <button
          type="button"
          onClick={onBack}
          className="px-4 py-2 rounded-lg bg-white/10 hover:bg-white/20 text-sm"
        >
          Back
        </button>
      </div>
    </div>
  );
}
