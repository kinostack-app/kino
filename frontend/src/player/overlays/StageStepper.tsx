import { cn } from '@/lib/utils';

/**
 * Dot-and-line stepper showing the journey through load
 * stages. Past stages are filled, the current is accent-
 * coloured and gently pulsing, upcoming ones are dim.
 * Connecting lines mirror the filled/dim state so the
 * sense of travel is unambiguous.
 */
export function StageStepper({ stages, current }: { stages: string[]; current: number }) {
  return (
    <div
      role="progressbar"
      aria-valuemin={1}
      aria-valuemax={stages.length}
      aria-valuenow={current + 1}
      aria-valuetext={`${stages[current]} (${current + 1} of ${stages.length})`}
      className="flex items-center"
    >
      {stages.map((stage, i) => {
        const done = i < current;
        const active = i === current;
        // The line BEFORE dot i represents the journey
        // from dot i-1 into dot i — reads as "traversed"
        // whenever dot i-1 is complete (i <= current).
        const lineTraversed = i <= current;
        return (
          <div key={stage} className="flex items-center">
            {i > 0 && (
              <span
                className={cn(
                  'block h-px w-8 mx-1 transition-colors duration-500',
                  lineTraversed ? 'bg-[var(--accent)]/60' : 'bg-white/10'
                )}
              />
            )}
            <span
              className={cn(
                'relative block rounded-full transition-all duration-500',
                active
                  ? 'w-2.5 h-2.5 bg-[var(--accent)] shadow-[0_0_12px_var(--accent)]'
                  : done
                    ? 'w-2 h-2 bg-[var(--accent)]/70'
                    : 'w-1.5 h-1.5 bg-white/25'
              )}
            >
              {active && (
                <span className="absolute inset-0 rounded-full bg-[var(--accent)]/40 animate-ping" />
              )}
            </span>
          </div>
        );
      })}
    </div>
  );
}
