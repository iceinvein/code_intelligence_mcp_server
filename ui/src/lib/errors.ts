/**
 * Turn a raw API/query error into plain, actionable copy for the operator.
 *
 * The daemon returns terse, sometimes-nested messages ("failed to load repo:
 * Failed to initialize repository state"). Leading with our own "X failed:"
 * prefix on top of that produces a wall of "failed". Instead we map the known
 * causes to a clear sentence + next action, and fall back to a single clean
 * lead plus the raw detail. Copy is lowercase-leading to match the surface.
 */
export function describeError(err: unknown, fallback: string): string {
  const raw = (err instanceof Error ? err.message : String(err ?? "")).trim();
  const low = raw.toLowerCase();

  // The daemon couldn't load a repo's on-disk index – common on a repo that is
  // registered but never indexed (or whose index is stale/missing).
  if (low.includes("repository state") || low.includes("not indexed") || low.includes("no index")) {
    return "this repository isn't indexed yet, or its index couldn't load. reindex it from the repositories tab, then try again.";
  }

  // A repo-scoped view with nothing selected/bound.
  if (low.includes("no repo") || low.includes("not bound") || low.includes("missing repo")) {
    return "no repository is selected. choose one above to continue.";
  }

  // Network / daemon down – fetch rejects with TypeError "Failed to fetch".
  if (
    low.includes("failed to fetch") ||
    low.includes("networkerror") ||
    low.includes("load failed") ||
    low.includes("network request failed")
  ) {
    return "can't reach the daemon at 127.0.0.1:17802. check that it's running, then retry.";
  }

  // Otherwise: one clear lead plus the raw detail (no stacked "failed:" prefixes).
  return raw ? `${fallback}: ${raw}` : fallback;
}
