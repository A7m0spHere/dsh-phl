import type { DshVersion } from '@/types'

/**
 * The manifest's version binding is the canonical `dsh-<ver>` id — the same
 * shape the version catalog mints (`versions/catalog.rs`) and the Rust
 * installer validates (`bound_id_for_version`). Adoption and pack install
 * historically stored the bare detected version ("0.9.9-test") instead, which
 * matched no catalog row and no installed directory: the instance displayed a
 * phantom version, the edit dropdown rendered blank (its controlled value was
 * not among the options), and launch could never resolve. New writes are
 * normalized backend-side; these helpers cover the read side for instances
 * adopted before the fix, so such a binding resolves (and is rewritten to the
 * canonical id on the next save) instead of being stuck.
 */

/** Map any historical/raw version shape onto the canonical binding id. */
export function toBoundVersionId(raw: string | null | undefined): string {
  const v = (raw ?? '').trim()
  if (!v || v.startsWith('dsh-')) return v
  return `dsh-${v}`
}

/**
 * Find the catalog entry an instance is bound to. Exact ids win; a legacy
 * bare-version binding is retried in canonical form. `null` means "no real
 * binding" (unbound, or a version PHL has never heard of) — callers must show
 * an actionable state, not the raw string as if it were a version name.
 */
export function resolveBoundVersion(
  versions: readonly DshVersion[],
  versionId: string | null | undefined,
): DshVersion | null {
  const raw = (versionId ?? '').trim()
  if (!raw) return null
  const exact = versions.find((v) => v.id === raw)
  if (exact) return exact
  if (!raw.startsWith('dsh-')) {
    return versions.find((v) => v.id === `dsh-${raw}`) ?? null
  }
  return null
}
