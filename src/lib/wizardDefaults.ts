import type { DshVersion, Runtime } from '@/types'
import { isRuntimeInstallable, isVersionInstallable } from '@/types'

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

/** Prefer an existing compatible Node, then a downloadable LTS build. */
export function pickDefaultRuntime(
  runtimes: readonly Runtime[],
  version: DshVersion,
): Runtime | undefined {
  return runtimes
    .filter((runtime) => !runtime.legacy && isRuntimeCompatible(runtime, version))
    .filter((runtime) => runtime.state.kind === 'installed' || isRuntimeInstallable(runtime))
    .sort((a, b) =>
      Number(b.state.kind === 'installed') - Number(a.state.kind === 'installed') ||
      Number(b.lts) - Number(a.lts) ||
      b.version.localeCompare(a.version, undefined, { numeric: true }),
    )[0]
}

export function isRuntimeCompatible(runtime: Runtime, version: DshVersion): boolean {
  return version.requiresNode.length === 0 || version.requiresNode.includes(runtime.major)
}

/** A first-run recommendation must have both a usable release and a Node. */
export function pickSetupVersion(
  versions: readonly DshVersion[],
  runtimes: readonly Runtime[],
): DshVersion | undefined {
  const candidates = versions
    .filter((version) => !version.pendingPublish && !version.legacy)
    .filter((version) => version.state.kind === 'installed' || isVersionInstallable(version))
    .filter((version) => !!pickDefaultRuntime(runtimes, version))
    .sort((a, b) => b.releasedAt.localeCompare(a.releasedAt))
  return candidates.find((version) => version.state.kind === 'installed') ??
    candidates.find((version) => version.channel === 'stable') ??
    candidates.find((version) => version.channel !== 'nightly') ?? candidates[0]
}
