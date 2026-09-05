import { describe, expect, it } from 'vitest'
import { detectPendingChanges } from './pendingReleases'
import type { DshVersion } from '@/types'

const version = (name: string, pending: boolean): DshVersion => ({
  id: `dsh-${name}`,
  name,
  channel: 'alpha',
  releasedAt: '2026-09-04T00:00:00Z',
  size: pending ? 0 : 43000,
  requiresNode: [],
  notes: [],
  pendingPublish: pending,
  source: pending ? undefined : { tarball: `https://registry/${name}.tgz` },
  state: { kind: 'available' },
})

describe('detectPendingChanges', () => {
  it('graduates a recorded pending version only when it reappears installable', () => {
    const prev = { '0.1.3-alpha.1': '2026-09-04T11:34:32Z' }
    const r = detectPendingChanges(prev, [version('0.1.3-alpha.1', false), version('0.1.2-rc.1', false)])
    expect(r.published).toEqual(['0.1.3-alpha.1'])
    expect(r.next).toEqual({})
  })

  it('a vanishing row (github fetch failed) is NOT a publish and survives in the set', () => {
    const prev = { '0.1.3-alpha.1': '2026-09-04T11:34:32Z' }
    const r = detectPendingChanges(prev, [version('0.1.2-rc.1', false)])
    expect(r.published).toEqual([])
    expect(r.next).toEqual(prev)
  })

  it('a still-pending row refreshes the record without alerting', () => {
    const prev = { '0.1.3-alpha.1': 'old' }
    const r = detectPendingChanges(prev, [version('0.1.3-alpha.1', true)])
    expect(r.published).toEqual([])
    expect(r.next['0.1.3-alpha.1']).toBe('2026-09-04T00:00:00Z')
  })

  it('first sight of a pending version records it quietly (no cold-start spam)', () => {
    const r = detectPendingChanges({}, [version('0.2.0-alpha.1', true)])
    expect(r.published).toEqual([])
    expect(r.next).toEqual({ '0.2.0-alpha.1': '2026-09-04T00:00:00Z' })
  })

  it('a version removed from GitHub entirely before publishing drops silently', () => {
    const prev = { '0.1.3-alpha.1': 'x' }
    // List explicitly contains the name as PENDING gone but as a NON-pending
    // row it graduates; a re-listed pending row with new data refreshes.
    const r = detectPendingChanges(prev, [version('0.1.3-alpha.1', true)])
    expect(r.published).toEqual([])
  })
})
