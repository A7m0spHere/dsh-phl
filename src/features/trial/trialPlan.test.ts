import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import type { DshVersion } from '@/types'
import type { TrialPreview } from '@/lib/desktop'
import {
  buildTrialCreateRequest,
  defaultTargetVersion,
  isPlanCurrent,
  planFromPreview,
  trialPlanKey,
  type TrialKnobs,
} from './trialPlan'

/**
 * R3-01 regression guard. The round-3 review reproduced a hard failure with
 * the payload the dialog actually sent: six fields, no plan, and the backend
 * refused the create outright ("缺少试升级计划"). Nothing in the UI test
 * suite caught it because no test tied the dialog's payload to the DTO the
 * Rust command validates.
 *
 * Two things are pinned here:
 *  - the payload the dialog builds carries the whole plan, and the KNOBS it
 *    was previewed for (the shared fixture is the single declaration of that
 *    key set, and the Rust side drives the real command with the same file);
 *  - a plan is only ever usable for the knob set it was issued for, so
 *    switching the target Runtime (which the preview effect used to omit from
 *    its dependencies) or the scope invalidates it instead of leaking a
 *    stale plan into the create payload.
 */

const FIXTURE = fileURLToPath(
  new URL('../../../src-tauri/src/instances/trial/fixtures/trial-create-request.json', import.meta.url),
)

/** The fixture's declared field set, minus its `_comment` header. */
function fixtureKeys(): string[] {
  const raw = JSON.parse(readFileSync(FIXTURE, 'utf8')) as Record<string, unknown>
  return Object.keys(raw)
    .filter((k) => !k.startsWith('_'))
    .sort()
}

function knobs(overrides: Partial<TrialKnobs> = {}): TrialKnobs {
  return {
    sourceId: 'daily',
    targetVersion: '0.1.7-rc.2',
    targetRuntimeId: 'node-system',
    scope: 'config',
    workspace: 'fresh',
    ...overrides,
  }
}

function preview(overrides: Partial<TrialPreview> = {}): TrialPreview {
  return {
    sourceId: 'daily',
    sourceName: '日常环境',
    sourceVersionId: 'dsh-0.1.5-rc.3',
    sourceRuntimeId: 'node-22.11.0',
    targetVersion: '0.1.7-rc.2',
    targetInstalled: true,
    targetRuntimeId: 'node-system',
    targetRuntimeInstalled: true,
    suggestedName: '日常环境 · 新版测试',
    scope: 'config',
    workspace: 'fresh',
    estimatedBytes: 1024,
    sessionCount: null,
    sessionCwds: [],
    pendingDownloads: [],
    conflicts: [],
    blocked: [],
    mismatchedPackages: [],
    planId: 'a'.repeat(64),
    targetId: 'trial-daily-1',
    sourceFingerprint: 'b'.repeat(64),
    allocatedPort: 8001,
    autoPort: false,
    ...overrides,
  }
}

describe('trial plan state (R3-01)', () => {
  it('builds the create payload the Rust command validates — same keys as the shared fixture', () => {
    const k = knobs()
    const plan = planFromPreview(trialPlanKey(k), preview())
    expect(plan).not.toBeNull()
    const req = buildTrialCreateRequest(k, 'review', plan!)
    expect(Object.keys(req).sort()).toEqual(fixtureKeys())
  })

  it('carries the preview-issued plan identity verbatim', () => {
    const k = knobs()
    const p = preview()
    const plan = planFromPreview(trialPlanKey(k), p)!
    const req = buildTrialCreateRequest(k, 'review', plan)
    expect(req.planId).toBe(p.planId)
    expect(req.targetId).toBe(p.targetId)
    expect(req.sourceFingerprint).toBe(p.sourceFingerprint)
    expect(req.targetRuntimeId).toBe(p.targetRuntimeId)
  })

  it('invalidates the plan when the target Runtime changes', () => {
    const before = trialPlanKey(knobs())
    const plan = planFromPreview(before, preview())!
    const after = trialPlanKey(knobs({ targetRuntimeId: 'node-22.11.0' }))
    expect(after).not.toBe(before)
    expect(isPlanCurrent(plan, after)).toBe(false)
  })

  it('invalidates the plan on every other plan-relevant knob', () => {
    const plan = planFromPreview(trialPlanKey(knobs()), preview())!
    for (const moved of [
      knobs({ targetVersion: '0.1.8' }),
      knobs({ sourceId: 'other' }),
      knobs({ scope: 'config+sessions' }),
      knobs({ workspace: 'shared' }),
    ]) {
      expect(isPlanCurrent(plan, trialPlanKey(moved))).toBe(false)
    }
  })

  it('keeps the plan current while the knobs stand still (retry after a lost response)', () => {
    const key = trialPlanKey(knobs())
    const plan = planFromPreview(key, preview())!
    expect(isPlanCurrent(plan, key)).toBe(true)
  })

  it('issues no plan when the backend returned no identity (mock / blocked preview)', () => {
    const key = trialPlanKey(knobs())
    expect(planFromPreview(key, preview({ planId: '' }))).toBeNull()
    expect(planFromPreview(key, preview({ targetId: '' }))).toBeNull()
    expect(planFromPreview(key, preview({ sourceFingerprint: '' }))).toBeNull()
    // A cleared plan is exactly what disables the create button.
    expect(isPlanCurrent(null, key)).toBe(false)
  })
})

/**
 * The default target version decides whether the flow can start at all: the
 * backend resolves bare names under `<root>/versions`, and the dialog's select
 * sends bare names. Preselecting the binding id (`dsh-x`) made the preview
 * look for a directory an install never creates, and the copy button stayed
 * disabled with 「未安装」 — caught by the real-window CDP lane on 2026-09-26.
 */
describe('trial target-version default (R3-01)', () => {
  const version = (name: string) =>
    ({
      id: `dsh-${name}`,
      name,
      channel: 'stable',
      releasedAt: '2026-09-01',
      size: 0,
      requiresNode: [],
      notes: [],
      state: { kind: 'installed', installedAt: '2026-09-01', installHealth: 'healthy', skippedDependencies: [] },
    }) as DshVersion

  it('preselects the newest installed version that is not the source binding — as a BARE name', () => {
    const options = [version('9.9.9-cdp'), version('9.9.8-cdp')]
    expect(defaultTargetVersion(options, 'dsh-9.9.8-cdp', null)).toBe('9.9.9-cdp')
    expect(defaultTargetVersion(options, 'dsh-9.9.9-cdp', null)).toBe('9.9.8-cdp')
  })

  it('never returns a binding id, and honours a preset verbatim', () => {
    const options = [version('9.9.9-cdp')]
    const picked = defaultTargetVersion(options, undefined, null)
    expect(picked).not.toMatch(/^dsh-/)
    expect(options.map((v) => v.name)).toContain(picked)
    expect(defaultTargetVersion(options, undefined, '9.9.7-cdp')).toBe('9.9.7-cdp')
  })

  it('stays empty when nothing is installed (create stays disabled)', () => {
    expect(defaultTargetVersion([], 'dsh-x', null)).toBe('')
  })
})

/**
 * Source-level wiring check, in the style of `InstanceCard.test.ts`: the
 * dialog must go through `buildTrialCreateRequest`, because that is what this
 * file and the Rust command test pin. An inline literal would silently
 * reintroduce the six-field payload from the review.
 */
describe('TrialDialog wiring (R3-01)', () => {
  const dialog = readFileSync(fileURLToPath(new URL('./TrialDialog.tsx', import.meta.url)), 'utf8')

  it('builds the create payload through the shared builder', () => {
    expect(dialog).toContain('buildTrialCreateRequest(')
  })

  it('gates create on a plan that matches the current knobs', () => {
    expect(dialog).toContain('isPlanCurrent(plan, planKey)')
    expect(dialog).toContain('!planCurrent')
  })

  it('re-previews when the target Runtime or the plan nonce moves', () => {
    expect(dialog).toMatch(/\[sourceId, targetVersion, resolvedRuntimeId, scope, workspace, replanNonce\]/)
  })

  it('refreshes the instance list after a create, so 打开副本 finds the copy', () => {
    expect(dialog).toContain('void reload()')
  })
})
