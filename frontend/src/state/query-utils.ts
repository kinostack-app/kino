/**
 * Query-key predicate helpers.
 *
 * hey-api's generated queries key themselves as
 * `[{_id: 'name', baseUrl, path, ...}]`, while hand-rolled queries
 * key themselves as `['name', ...]`. A naive predicate like
 * `q.queryKey[0] === 'name'` silently never matches the generated
 * shape, which manifests as "invalidate call did nothing" — usually
 * spotted as "UI doesn't update until refresh" bugs on surfaces
 * whose freshness depends on a generated query (`showWatchState`,
 * `continueWatching`, `calendar`, …).
 *
 * `queryMatchesId` handles both shapes so call sites don't need to
 * care which scheme the target query uses.
 */

export function queryMatchesId(q: { queryKey: readonly unknown[] }, id: string): boolean {
  const first = q.queryKey[0];
  if (first === id) return true;
  if (typeof first === 'object' && first !== null) {
    return (first as { _id?: string })._id === id;
  }
  return false;
}
