import { describe, expect, it } from 'vitest'
import { resolveUpdate } from './updateState'

describe('resolveUpdate (§M4 / §R5 interpretation contract)', () => {
  it('resolved newer → updatable with that version shown', () => {
    expect(
      resolveUpdate({ check: { status: 'resolved', latest: '2.0.0' }, installedVersion: '1.0.0', linked: false }),
    ).toEqual({ newest: '2.0.0', outdated: true })
  })
  it('resolved equal → not updatable', () => {
    expect(
      resolveUpdate({ check: { status: 'resolved', latest: '1.0.0' }, installedVersion: '1.0.0', linked: false }),
    ).toEqual({ newest: '1.0.0', outdated: false })
  })
  it('never-checked → catalog seed is an estimate', () => {
    expect(
      resolveUpdate({ check: undefined, seedLatest: '3.1.0', installedVersion: '3.0.0', linked: false }),
    ).toEqual({ newest: '3.1.0', outdated: true })
    expect(
      resolveUpdate({ check: undefined, seedLatest: undefined, installedVersion: '3.0.0', linked: false }),
    ).toEqual({ newest: undefined, outdated: false })
  })
  it('checking / error / unresolvable are NEVER asserted up to date (§R5)', () => {
    for (const check of [
      { status: 'checking' } as const,
      { status: 'error', message: 'x' } as const,
      { status: 'unresolvable' } as const,
    ]) {
      // Even with a seed that WOULD look newer, a non-resolved state shows no
      // newest and never claims outdated.
      expect(
        resolveUpdate({ check, seedLatest: '9.9.9', installedVersion: '1.0.0', linked: false }),
      ).toEqual({ newest: undefined, outdated: false })
    }
  })
  it('linked plugins are never updatable, but still surface a resolved newest', () => {
    expect(
      resolveUpdate({ check: { status: 'resolved', latest: '2.0.0' }, installedVersion: '1.0.0', linked: true }),
    ).toEqual({ newest: '2.0.0', outdated: false })
  })
})
