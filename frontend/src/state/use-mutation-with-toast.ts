/**
 * `useMutation` with a default `onError` toast.
 *
 * The default is "silent failure": a mutation whose request 500s
 * rolls back its optimistic patch (if any) and the user sees nothing.
 * For settings saves and download-lifecycle buttons this is actively
 * dangerous — the user believes their click worked and walks away.
 *
 * This wrapper adds:
 *   - `onError: kinoToast.error('Couldn't {verb}', ...)` by default
 *   - The caller's own `onError` still runs (after the toast) if
 *     provided — opt out entirely via `silentError: true`
 *
 * Use for every mutation unless you've got a domain-specific error
 * surface (inline banner, stall overlay). Check the existing Explore
 * audit for the ~25 silent sites to convert.
 */

import { type DefaultError, type UseMutationOptions, useMutation } from '@tanstack/react-query';
import { kinoToast } from '@/components/kino-toast';

interface ToastOptions {
  /** Short verb phrase: "save settings", "pause download", "follow show".
   *  Rendered as "Couldn't {verb}." Keep it lowercase — the UI prepends
   *  "Couldn't ". Default: "complete action". */
  verb?: string;
  /** Skip the toast entirely. For mutations with richer error UX
   *  (inline banners, dialog-internal errors) that would be
   *  duplicated by a toast. */
  silentError?: boolean;
}

/** Defaults match TanStack's own `useMutation` generics so call sites
 *  with `mutationFn: () => ...` (no args) get `TVars = void` — which
 *  is what makes `mutate()` callable without parameters. Without these
 *  defaults, a caller that doesn't explicitly specify TVars gets
 *  `unknown`, and `mutate()` rejects as "Expected 1-2 arguments." */
export function useMutationWithToast<
  TData = unknown,
  TError = DefaultError,
  TVars = void,
  TContext = unknown,
>(options: UseMutationOptions<TData, TError, TVars, TContext> & ToastOptions) {
  const { verb, silentError, onError, ...rest } = options;
  return useMutation<TData, TError, TVars, TContext>({
    ...rest,
    onError: (error, vars, ctx, mctx) => {
      if (!silentError) {
        const msg = error instanceof Error ? error.message : String(error);
        kinoToast.error(`Couldn't ${verb ?? 'complete action'}`, {
          description: msg || undefined,
        });
      }
      onError?.(error, vars, ctx, mctx);
    },
  });
}
