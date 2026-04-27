import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ArrowRight, TriangleAlert, X } from 'lucide-react';
import { useState } from 'react';
import { getStatus } from '@/api/generated/sdk.gen';
import { cn } from '@/lib/utils';

/**
 * A single warning row returned by /api/v1/status.
 * `route` points the Fix link at the exact settings section that
 * remediates this warning. Missing when no specific page fixes it,
 * in which case the banner falls back to /settings.
 */
interface StatusWarning {
  message: string;
  route?: string | null;
}

interface HealthBannerProps {
  /**
   * Optional — if omitted, the banner queries /status itself.
   * Provided by App.tsx when it already has the status loaded.
   */
  warnings?: StatusWarning[];
}

// These warnings indicate the system can't function — don't allow dismissal
const CRITICAL_PATTERNS = [
  'not running',
  'not found',
  'not configured',
  'not initialized',
  'stuck in',
];

function isCritical(warning: StatusWarning): boolean {
  const msg = warning.message.toLowerCase();
  return CRITICAL_PATTERNS.some((p) => msg.includes(p));
}

/**
 * Derive the human label of a target section from its route.
 * `/settings/playback` → `Playback`, `/downloads` → `Downloads`,
 * anything else falls back to `Settings` since that's where the
 * Fix link lands when the backend didn't supply a specific route.
 */
function sectionLabel(route?: string | null): string {
  if (!route) return 'Settings';
  const tail = route.split('/').filter(Boolean).pop() ?? '';
  if (!tail) return 'Settings';
  return tail.charAt(0).toUpperCase() + tail.slice(1);
}

export function HealthBanner({ warnings: warningsProp }: HealthBannerProps) {
  const [dismissed, setDismissed] = useState(false);

  // Fall back to our own status query when no warnings prop is supplied.
  const { data: status } = useQuery({
    queryKey: ['kino', 'status'],
    queryFn: async () => {
      const { data } = await getStatus();
      return data as { warnings: StatusWarning[] };
    },
    staleTime: 30_000,
    // No polling — meta-driven invalidation fires on every event
    // that can flip a warning on or off.
    enabled: warningsProp === undefined,
    meta: {
      invalidatedBy: [
        'health_warning',
        'health_recovered',
        'indexer_changed',
        'config_changed',
        'download_failed',
        // ffmpeg download flips config.ffmpeg_path which
        // feeds the banner's "FFmpeg out of date" / "libplacebo
        // missing" warning generator. Refetching on these two
        // makes the banner reflect the new state immediately
        // after a successful download or a failed one that
        // leaves the system ffmpeg in place.
        'ffmpeg_download_completed',
        'ffmpeg_download_failed',
        'ip_leak_detected',
      ],
    },
  });

  const warnings = warningsProp ?? status?.warnings ?? [];

  if (warnings.length === 0) return null;

  const hasCritical = warnings.some(isCritical);

  if (dismissed && !hasCritical) return null;

  // The banner shows only the first warning in full — if there are
  // several, we summarise with "N issues" and let the user resolve
  // them one at a time as each clears. The Fix link points at the
  // first warning's route; subsequent warnings get their own link
  // once the first is gone.
  const primary = warnings[0];
  const fixRoute = primary.route ?? '/settings';
  const fixLabel = sectionLabel(primary.route);

  return (
    <div
      className={cn(
        'border-b px-4 md:px-12 py-2 text-sm',
        hasCritical
          ? 'bg-red-500/[0.08] border-red-500/20 text-red-100'
          : 'bg-amber-500/[0.08] border-amber-500/20 text-amber-100'
      )}
    >
      <div className="flex items-center gap-3">
        <TriangleAlert
          size={15}
          className={cn('flex-shrink-0', hasCritical ? 'text-red-400' : 'text-amber-400')}
        />
        <div className="flex-1 min-w-0 text-[13px]">
          {warnings.length === 1 ? (
            <span className="truncate block">{primary.message}</span>
          ) : (
            <span className="truncate block">
              <span className="font-medium">
                {warnings.length} issue{warnings.length > 1 ? 's' : ''}
              </span>{' '}
              <span className="opacity-70">· {primary.message}</span>
            </span>
          )}
        </div>
        <Link
          to={fixRoute}
          title={`Open ${fixLabel} to fix this`}
          className={cn(
            'flex items-center gap-1.5 h-7 px-2.5 rounded-md text-[12px] font-medium transition flex-shrink-0',
            hasCritical
              ? 'bg-red-500/15 text-red-100 hover:bg-red-500/25'
              : 'bg-amber-500/15 text-amber-100 hover:bg-amber-500/25'
          )}
        >
          Fix in {fixLabel}
          <ArrowRight size={12} />
        </Link>
        {!hasCritical && (
          <button
            type="button"
            onClick={() => setDismissed(true)}
            aria-label="Dismiss"
            className="flex-shrink-0 p-1 -mr-1 rounded hover:bg-white/10 text-[var(--text-muted)] hover:text-white transition"
          >
            <X size={14} />
          </button>
        )}
      </div>
    </div>
  );
}
