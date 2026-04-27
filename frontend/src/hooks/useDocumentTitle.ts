/**
 * Set the browser document title while a route is mounted.
 *
 * Format: `"Page · kino"` for regular pages, plain `"kino"` on unmount or
 * when no title is provided (e.g. while data is loading).
 *
 * Use this per-route so the title can include dynamic data (movie name,
 * search query, tab) without needing a central pathname → title map that
 * drifts every time routes change.
 *
 * @example
 *   useDocumentTitle('Library');               // "Library · kino"
 *   useDocumentTitle(movie?.title);            // "The Matrix · kino"
 *   useDocumentTitle(null);                    // "kino" (loading state)
 */

import { useEffect } from 'react';

const APP_NAME = 'kino';

export function useDocumentTitle(title: string | null | undefined): void {
  useEffect(() => {
    const previous = document.title;
    document.title = title ? `${title} · ${APP_NAME}` : APP_NAME;
    return () => {
      document.title = previous;
    };
  }, [title]);
}
