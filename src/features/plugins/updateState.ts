import type { LatestVersionState } from '@/stores/catalogStore'

/**
 * Pure interpretation of a plugin's update-check state into "what to show" (§M4,
 * building on §R5). Both the Updates-tab count and the per-row display derive
 * from this one function so they can never disagree — previously two copies of
 * the same switch risked drift (a `null` once meant different things to each).
 */
export interface ResolvedUpdate {
  /** The newest version to display, or `undefined` when we don't know one. */
  newest: string | undefined
  /** True ONLY when we KNOW a different, newer version exists. A `checking` /
   *  `error` / `unresolvable` state is never asserted updatable — and, paired
   *  with `newest === undefined`, never asserted "up to date" (§R5). */
  outdated: boolean
}

export function resolveUpdate(opts: {
  /** The plugin's update-check state, or `undefined` if never checked. */
  check?: LatestVersionState
  /** The catalog's static latest release, used only as a pre-check estimate. */
  seedLatest?: string
  /** The version currently installed in this instance. */
  installedVersion: string
  /** Linked plugins (local dev folders) are never updatable. */
  linked: boolean
}): ResolvedUpdate {
  const { check, seedLatest, installedVersion, linked } = opts
  if (check?.status === 'resolved') {
    return { newest: check.latest, outdated: !linked && check.latest !== installedVersion }
  }
  if (check === undefined) {
    // Not checked yet: fall back to the catalog's static latest as an estimate.
    const newest = seedLatest
    return { newest, outdated: !!newest && !linked && newest !== installedVersion }
  }
  // checking / error / unresolvable: we don't have a trustworthy answer.
  return { newest: undefined, outdated: false }
}
