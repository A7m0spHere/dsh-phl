import { describe, expect, it } from 'vitest'
import { pickDefaultVersion } from './wizardDefaults'
import type { DshVersion } from '@/types'

const v = (name: string, patch: Partial<DshVersion> = {}): DshVersion =>
  ({
    id: `dsh-${name}`,
    name,
    releasedAt: '2026-09-17T00:00:00.000Z',
    state: { kind: 'available' },
    ...patch,
  }) as DshVersion

/** Newest first — the order `list_dsh_versions` returns. */
const catalog = [
  v('0.1.6-alpha.2', { pendingPublish: true }),
  v('0.1.6-alpha.1'),
  v('0.1.5-rc.2'),
]

describe('pickDefaultVersion', () => {
  it('skips a GitHub-only row instead of opening the wizard on a blocked choice', () => {
    expect(pickDefaultVersion(catalog)?.name).toBe('0.1.6-alpha.1')
  })

  it('prefers an installed version over a newer uninstalled one', () => {
    const versions = [...catalog, v('0.1.5-rc.1', { state: { kind: 'installed', installedAt: 'x' } })]
    expect(pickDefaultVersion(versions)?.name).toBe('0.1.5-rc.1')
  })

  it('does not let a legacy maintenance-line version take the installed seat', () => {
    const versions = [
      v('0.1.0-rc.7', { legacy: true, state: { kind: 'installed', installedAt: 'x' } }),
      v('0.1.5-rc.2', { state: { kind: 'installed', installedAt: 'y' } }),
    ]
    expect(pickDefaultVersion(versions)?.name).toBe('0.1.5-rc.2')
    // With nothing else on offer it is still a better default than none.
    expect(pickDefaultVersion([versions[0]])?.name).toBe('0.1.0-rc.7')
  })

  it('honours the clone source even when that version is GitHub-only', () => {
    expect(pickDefaultVersion(catalog, 'dsh-0.1.6-alpha.2')?.name).toBe('0.1.6-alpha.2')
    expect(pickDefaultVersion(catalog, null)?.name).toBe('0.1.6-alpha.1')
  })

  it('reports nothing when only GitHub-only releases exist', () => {
    expect(pickDefaultVersion([v('0.2.0-alpha.1', { pendingPublish: true })])).toBeUndefined()
    expect(pickDefaultVersion([])).toBeUndefined()
  })
})
