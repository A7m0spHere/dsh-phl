import { describe, it, expect, vi, beforeEach } from 'vitest'

import { useUIStore } from './uiStore'
import { useSettingsStore } from './settingsStore'
import { isInstanceLiveForSnapshot } from '@/types/instance'

/**
 * Stabilization regressions for the state-semantic convergence (Phase 5):
 * the in-app "动画：关闭" must reach the CSS layer, the snapshot guard's live
 * status set is one shared predicate, and the WebUI auto-open is a defaulted-on
 * user setting. These lock the behaviour so the same class of drift cannot
 * silently return.
 */

describe('uiStore.setMotion', () => {
  let setAttribute: ReturnType<typeof vi.fn>
  beforeEach(() => {
    setAttribute = vi.fn()
    // setMotion reads `document.documentElement` at call time; stub it so the
    // assertion does not depend on a DOM test environment.
    ;(globalThis as unknown as { document: unknown }).document = {
      documentElement: {
        setAttribute,
        style: {},
        classList: { toggle: vi.fn(), add: vi.fn(), remove: vi.fn() },
      },
    }
  })

  it('mirrors the level onto the root data-motion attribute', () => {
    useUIStore.getState().setMotion('off')
    expect(setAttribute).toHaveBeenCalledWith('data-motion', 'off')
    useUIStore.getState().setMotion('full')
    expect(setAttribute).toHaveBeenLastCalledWith('data-motion', 'full')
    expect(useUIStore.getState().motion).toBe('full')
  })
})

describe('isInstanceLiveForSnapshot', () => {
  it('blocks the live and mid-transition statuses', () => {
    for (const s of ['running', 'starting', 'stopping'] as const) {
      expect(isInstanceLiveForSnapshot(s)).toBe(true)
    }
  })
  it('allows resting statuses (including error) to snapshot', () => {
    for (const s of ['stopped', 'error'] as const) {
      expect(isInstanceLiveForSnapshot(s)).toBe(false)
    }
  })
})

describe('settings.autoOpenWebUi', () => {
  it('defaults to on, matching the long-standing auto-open behaviour', () => {
    expect(useSettingsStore.getState().autoOpenWebUi).toBe(true)
  })
  it('is settable via the generic setter', () => {
    useSettingsStore.getState().set('autoOpenWebUi', false)
    expect(useSettingsStore.getState().autoOpenWebUi).toBe(false)
    useSettingsStore.getState().set('autoOpenWebUi', true)
  })
})
