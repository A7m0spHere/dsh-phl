import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  listNodeRuntimeCatalog: vi.fn(),
  listInstalledRuntimes: vi.fn(),
  runtimesDiskUsage: vi.fn(),
  systemNodeVersion: vi.fn(),
}))
vi.mock('@/lib/desktop', () => ({
  NODE_DIST_MIRROR: 'https://cdn.npmmirror.com/binaries/node',
  NODE_DIST_OFFICIAL: 'https://nodejs.org/dist',
  listNodeRuntimeCatalog: mocks.listNodeRuntimeCatalog,
  listInstalledRuntimes: mocks.listInstalledRuntimes,
  runtimesDiskUsage: mocks.runtimesDiskUsage,
  systemNodeVersion: mocks.systemNodeVersion,
}))
vi.mock('@/stores/settingsStore', () => ({
  useSettingsStore: { getState: () => ({ source: 'official', customSource: '', keepArchives: false }) },
}))

import { InstalledScanError } from './repository'
import { tauriRuntimeOverrides } from './tauriRuntimes'

const catalogMeta = (major: number) => ({
  id: `node-${major}`,
  major,
  version: `${major}.0.0`,
  lts: true,
})

beforeEach(() => {
  vi.resetAllMocks()
  mocks.listNodeRuntimeCatalog.mockResolvedValue([catalogMeta(22), catalogMeta(20)])
  mocks.listInstalledRuntimes.mockResolvedValue([
    { name: 'node-22', installedAt: '2026-09-02', version: '22.5.1' },
  ])
  mocks.runtimesDiskUsage.mockResolvedValue({ 'node-22': 1234 })
  mocks.systemNodeVersion.mockResolvedValue(null)
})

describe('listRuntimes failure paths', () => {
  it('still lists installed runtimes when the catalog is unavailable', async () => {
    mocks.listNodeRuntimeCatalog.mockRejectedValue(new Error('网络不可达'))
    const runtimes = await tauriRuntimeOverrides.listRuntimes()
    expect(runtimes.map((r) => [r.id, r.state.kind])).toEqual([['node-22', 'installed']])
  })

  it('fails loudly on an installed-scan error instead of offering installed items as installable', async () => {
    mocks.listInstalledRuntimes.mockRejectedValue(new Error('[permission] 数据目录不可读'))
    await expect(tauriRuntimeOverrides.listRuntimes()).rejects.toBeInstanceOf(InstalledScanError)
  })

  it('degrades disk usage to zero without failing the list (size is display-only)', async () => {
    mocks.runtimesDiskUsage.mockRejectedValue(new Error('volume busy'))
    const runtimes = await tauriRuntimeOverrides.listRuntimes()
    expect(runtimes.find((r) => r.id === 'node-22')?.size).toBe(0)
    expect(runtimes.find((r) => r.id === 'node-22')?.state.kind).toBe('installed')
  })

  it('treats an empty local scan as genuinely no installations', async () => {
    mocks.listInstalledRuntimes.mockResolvedValue([])
    const runtimes = await tauriRuntimeOverrides.listRuntimes()
    expect(runtimes.map((r) => [r.id, r.state.kind])).toEqual([
      ['node-22', 'available'],
      ['node-20', 'available'],
    ])
  })
})
