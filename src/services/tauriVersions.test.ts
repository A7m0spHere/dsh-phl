import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  listDshVersions: vi.fn(),
  listInstalledVersions: vi.fn(),
  enrichModelMetadata: vi.fn(),
}))
vi.mock('@/lib/desktop', () => ({
  listDshVersions: mocks.listDshVersions,
  listInstalledVersions: mocks.listInstalledVersions,
  enrichModelMetadata: mocks.enrichModelMetadata,
}))
vi.mock('@/stores/settingsStore', () => ({
  useSettingsStore: { getState: () => ({ source: 'official', customSource: '' }) },
}))

import { InstalledScanError } from './repository'
import { tauriRepository } from './tauriRepository'

const remoteMeta = (name: string) => ({
  id: `dsh-${name}`,
  name,
  channel: 'stable',
  releasedAt: '2026-09-01',
  size: 100,
  requiresNode: [22],
  notes: [],
  source: { tarball: `https://registry/${name}.tgz`, integrity: 'sha512-x' },
})

beforeEach(() => {
  vi.resetAllMocks()
  mocks.listDshVersions.mockResolvedValue([remoteMeta('1.0.0'), remoteMeta('2.0.0')])
  mocks.listInstalledVersions.mockResolvedValue([
    { name: '1.0.0', installedAt: '2026-09-02', installHealth: 'healthy', skippedDependencies: [] },
  ])
})

describe('listVersions failure paths', () => {
  it('reports installed versions as installed while the remote catalog is unavailable', async () => {
    mocks.listDshVersions.mockRejectedValue(new Error('网络不可达'))
    const versions = await tauriRepository.listVersions()
    expect(versions.map((v) => [v.name, v.state.kind])).toEqual([['1.0.0', 'installed']])
  })

  it('fails loudly on an installed-scan error instead of offering installed items as installable', async () => {
    mocks.listInstalledVersions.mockRejectedValue(new Error('[permission] 数据目录不可读'))
    await expect(tauriRepository.listVersions()).rejects.toBeInstanceOf(InstalledScanError)
  })

  it('keeps the underlying error message for user-facing rendering', async () => {
    mocks.listInstalledVersions.mockRejectedValue(new Error('[permission] 数据目录不可读'))
    await expect(tauriRepository.listVersions()).rejects.toThrow('数据目录不可读')
  })

  it('treats an empty local scan as genuinely no installations', async () => {
    mocks.listInstalledVersions.mockResolvedValue([])
    const versions = await tauriRepository.listVersions()
    expect(versions.map((v) => [v.name, v.state.kind])).toEqual([
      ['1.0.0', 'available'],
      ['2.0.0', 'available'],
    ])
  })
})
