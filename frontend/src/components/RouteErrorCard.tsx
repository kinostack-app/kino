/**
 * Route-level error fallback.
 *
 * Used as the `errorComponent` on detail routes (MovieDetail,
 * ShowDetail, ListDetail, player) so a crash inside the route's
 * component OR a thrown `validateSearch` error lands on a small card
 * with a Back / Home affordance instead of blanking the page.
 *
 * Deliberately simpler than `AppErrorBoundary`: those are the root
 * and per-render catch-alls. Route-level errors are usually bad URLs
 * or transient API failures — the user just wants out, not
 * diagnostics.
 */

import { useRouter } from '@tanstack/react-router';
import { AlertTriangle, ArrowLeft, Home } from 'lucide-react';

export function RouteErrorCard({ error }: { error: Error }) {
  const router = useRouter();
  return (
    <div className="min-h-[60vh] flex flex-col items-center justify-center gap-3 text-center px-6">
      <AlertTriangle size={32} className="text-amber-400" aria-hidden="true" />
      <h1 className="text-xl font-semibold text-white">This page couldn&apos;t load</h1>
      <p className="text-sm text-[var(--text-secondary)] max-w-sm">
        {error.message || 'The URL may be malformed, or the item no longer exists.'}
      </p>
      <div className="flex gap-2 mt-2">
        <button
          type="button"
          onClick={() => router.history.back()}
          className="inline-flex items-center gap-1.5 px-4 py-2 rounded-lg bg-white/5 hover:bg-white/10 text-sm text-white"
        >
          <ArrowLeft size={14} aria-hidden="true" />
          Back
        </button>
        <a
          href="/"
          className="inline-flex items-center gap-1.5 px-4 py-2 rounded-lg bg-[var(--accent)] hover:bg-[var(--accent-hover)] text-white text-sm font-semibold"
        >
          <Home size={14} aria-hidden="true" />
          Go home
        </a>
      </div>
    </div>
  );
}
