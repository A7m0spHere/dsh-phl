import type { DshVersion } from '@/types'

/**
 * Pending-release tracking for the version catalog.
 *
 * DSH sometimes cuts a GitHub release before publishing it to npm (see
 * `list_dsh_versions`, which surfaces such versions with `pendingPublish`).
 * A user who sees "待发布" wants to know the moment it becomes installable —
 * without babysitting the sync button. This module turns each catalog fetch
 * into that check, purely: the store persists the set and raises the toast.
 *
 * The one thing the logic must not get wrong is the failure mode where the
 * GitHub source is simply unreachable: `list_dsh_versions` then returns
 * npm-only, every pending row vanishes, and a naive diff would scream
 * "published!" for versions that were never published at all. So a name only
 * counts as newly installable when it *explicitly* appears in the fresh list
 * as non-pending — its disappearance proves nothing.
 */

export type PendingSet = Record<string, string>

export interface PendingChange {
  /** Names recorded as pending that now appear installable — alert these. */
  published: string[]
  /** The next persisted set: every name currently pending, plus still-
   *  unconfirmed records whose rows vanished this fetch (GitHub hiccup). */
  next: PendingSet
}

export function detectPendingChanges(
  prev: PendingSet,
  list: DshVersion[],
): PendingChange {
  const byName = new Map(list.map((v) => [v.name, v]))
  const published: string[] = []

  // A recorded pending name only graduates when the fresh list proves it:
  // same version, now with an install source.
  for (const name of Object.keys(prev)) {
    const now = byName.get(name)
    if (now && !now.pendingPublish) published.push(name)
  }

  const next: PendingSet = {}
  for (const v of list) {
    if (v.pendingPublish) next[v.name] = v.releasedAt || prev[v.name] || ''
  }
  // Keep records for names that vanished entirely (e.g. GitHub fetch failed
  // this round): they are neither confirmed published nor confirmed gone.
  for (const [name, at] of Object.entries(prev)) {
    if (!(name in next) && !byName.has(name)) next[name] = at
  }

  return { published: published.sort(), next }
}

const KEY = 'phl.pendingReleases'

/** localStorage read, tolerant of a missing/corrupt entry. */
export function loadPendingSet(): PendingSet {
  try {
    const raw = localStorage.getItem(KEY)
    const parsed = raw ? (JSON.parse(raw) as PendingSet) : {}
    return typeof parsed === 'object' && parsed !== null ? parsed : {}
  } catch {
    return {}
  }
}

export function savePendingSet(set: PendingSet): void {
  localStorage.setItem(KEY, JSON.stringify(set))
}
