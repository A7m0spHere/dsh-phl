import type { DshVersion } from '@/types'
import type { TrialPreview, TrialRequest } from '@/lib/desktop'

/**
 * The target version the dialog preselects: the newest installed version that
 * is not the source's own binding, or the caller's preset.
 *
 * Bare names only. A `DshVersion.id` is the instance-binding spelling
 * (`dsh-0.1.7-rc.2`), and the select's option values — like every other
 * version the backend takes — are bare names. Preselecting the id made the
 * preview ask for a version directory that can never exist (`versions/dsh-x`
 * is not where an install lands), so the copy button stayed disabled with
 * 「未安装」 for the rest of the session (found by the CDP lane, 2026-09-26).
 */
export function defaultTargetVersion(
  options: DshVersion[],
  currentId: string | undefined,
  preset: string | null,
): string {
  if (preset) return preset
  const different = options.find((v) => v.id !== currentId)
  return (different ?? options[0])?.name ?? ''
}

/**
 * The plan a preview issued, bound to the exact knob set it was built for.
 *
 * The backend refuses a create that does not carry the plan it previewed
 * (`planId`, `targetId`, `sourceFingerprint`), and the plan covers every knob
 * plus the source fingerprint. Keeping the plan in one small object — keyed
 * by the knobs — is what makes "current options + old plan" impossible to
 * assemble by accident (R3-01): a knob change moves the key, and a plan whose
 * key no longer matches is not usable, so the dialog waits for the new
 * preview instead of submitting with a stale one.
 */

export interface TrialKnobs {
  sourceId: string
  targetVersion: string
  targetRuntimeId: string
  scope: TrialRequest['scope']
  workspace: TrialRequest['workspace']
}

export interface TrialPlanRef {
  /** `trialPlanKey` of the knobs this plan was previewed for. */
  key: string
  planId: string
  targetId: string
  sourceFingerprint: string
}

/**
 * Identity of the preview a plan belongs to. Every field the backend's plan
 * id covers is in here, so changing any of them requires a fresh preview —
 * including the target Runtime, which the preview effect used to omit from
 * its dependencies.
 */
export function trialPlanKey(knobs: TrialKnobs): string {
  return [
    knobs.sourceId,
    knobs.targetVersion,
    knobs.targetRuntimeId,
    knobs.scope,
    knobs.workspace,
  ].join('|')
}

/**
 * The executable plan a preview carried, or `null` when the backend did not
 * issue one (an older backend, or a preview that only documents a blocked
 * state). A `null` plan disables the create button rather than guessing.
 */
export function planFromPreview(key: string, preview: TrialPreview): TrialPlanRef | null {
  if (!preview.planId || !preview.targetId || !preview.sourceFingerprint) return null
  return {
    key,
    planId: preview.planId,
    targetId: preview.targetId,
    sourceFingerprint: preview.sourceFingerprint,
  }
}

/** Whether this plan is still the one the current knobs were previewed for. */
export function isPlanCurrent(plan: TrialPlanRef | null, key: string): boolean {
  return plan !== null && plan.key === key
}

/**
 * The exact payload `create_trial` receives. Built in one place so a plan can
 * never travel without the knobs it was previewed for, and so the DTO the
 * backend validates is the same one the tests pin
 * (`src-tauri/src/instances/trial/fixtures/trial-create-request.json`).
 */
export function buildTrialCreateRequest(
  knobs: TrialKnobs,
  name: string,
  plan: TrialPlanRef,
): TrialRequest {
  return {
    sourceId: knobs.sourceId,
    targetVersion: knobs.targetVersion,
    targetRuntimeId: knobs.targetRuntimeId,
    name,
    scope: knobs.scope,
    workspace: knobs.workspace,
    planId: plan.planId,
    targetId: plan.targetId,
    sourceFingerprint: plan.sourceFingerprint,
  }
}
