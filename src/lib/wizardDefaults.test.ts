import { describe, expect, it } from 'vitest'
import { pickDefaultRuntime, pickDefaultVersion, pickSetupVersion } from './wizardDefaults'
import type { DshVersion, Runtime } from '@/types'

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

const runtime = (major: number, patch: Partial<Runtime> = {}): Runtime => ({
  id: `node-${major}.1.0`, name: `Node ${major}`, version: `${major}.1.0`,
  major, lts: true, size: 1, state: { kind: 'available' }, ...patch,
})

describe('first-run recommendations', () => {
  const release = (name: string, patch: Partial<DshVersion> = {}) => v(name, {
    requiresNode: [22, 24], channel: 'stable', ...patch,
  })

  it('chooses a compatible downloadable Node on an empty machine', () => {
    expect(pickDefaultRuntime([runtime(26), runtime(24), runtime(22)], release('1'))?.major).toBe(24)
  })

  it('reuses an installed compatible Node, including a detected system Node', () => {
    const system = runtime(22, { id: 'node-system', system: true, state: { kind: 'installed', installedAt: '' } })
    expect(pickDefaultRuntime([runtime(24), system], release('1'))).toBe(system)
  })

  it('keeps unknown Node requirements usable and skips legacy or busy runtimes', () => {
    expect(pickDefaultRuntime([
      runtime(24, { legacy: true }), runtime(22, { state: { kind: 'removing' } }), runtime(26),
    ], release('1', { requiresNode: [] }))?.major).toBe(26)
  })

  it('selects a stable release with a matching runtime ahead of a newer preview', () => {
    const stable = release('1')
    expect(pickSetupVersion([
      release('3', { pendingPublish: true }), release('2', { channel: 'alpha' }), stable,
    ], [runtime(24)])).toBe(stable)
  })

  it('excludes releases that have no matching runtime and reuses installed releases', () => {
    const installed = release('1', { state: { kind: 'installed', installedAt: '' } })
    expect(pickSetupVersion([
      release('3', { requiresNode: [30] }), release('2'), installed,
    ], [runtime(24)])).toBe(installed)
    expect(pickSetupVersion([release('3', { requiresNode: [30] })], [runtime(24)])).toBeUndefined()
  })
})
