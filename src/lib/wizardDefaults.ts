import type { DshVersion } from '@/types'

/**
 * The wizard's default DSH version, seeded once the catalog is available.
 *
 * Order: the instance being cloned (that binding is the whole point of cloning,
 * even when the version is GitHub-only — the clone then surfaces the same
 * blocking notice the source instance has), then the newest *installed*
 * version, then the newest installable one.
 *
 * A GitHub-only row is skipped in that last step on purpose: npm carries no
 * package for it, so preselecting it would open the wizard on a choice that
 * blocks submission (2026-09-10 review #17). `undefined` means there is nothing
 * to preselect — the version section then reports an empty choice instead of
 * pretending one was made.
 */
export function pickDefaultVersion(
  versions: readonly DshVersion[],
  sourceVersionId?: string | null,
): DshVersion | undefined {
  return (
    versions.find((v) => !!sourceVersionId && v.id === sourceVersionId) ??
    versions.find((v) => v.state.kind === 'installed' && !v.legacy) ??
    versions.find((v) => !v.pendingPublish)
  )
}
