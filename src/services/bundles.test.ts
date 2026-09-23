import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  readInstanceBundle: vi.fn(),
  importInstanceBundle: vi.fn(),
}))
vi.mock('@/lib/desktop', () => ({
  readInstanceBundle: mocks.readInstanceBundle,
  importInstanceBundle: mocks.importInstanceBundle,
}))
// Keep the real `newInstanceId` (its slug rules are part of the contract),
// stub only the record→Instance conversion, which needs a full wire record.
vi.mock('./tauriInstances', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./tauriInstances')>()),
  instanceFromRecord: (record: { id: string }) => ({ id: record.id }),
}))

import { importBundleAsInstance, readBundle } from './bundles'

const preview = {
  name: 'Bundled',
  versionId: 'dsh-1.2.0',
  runtimeId: 'node-22',
  port: 3999,
  pluginCount: 2,
  exportedAt: '2026-09-01',
  credentials: ['OPENAI_API_KEY'],
  machineOnly: ['PATH'],
}

beforeEach(() => vi.resetAllMocks())

describe('bundle import service', () => {
  it('reads through to the bridge', async () => {
    mocks.readInstanceBundle.mockResolvedValue(preview)
    await expect(readBundle('C:/x/dsh-bundle.json')).resolves.toBe(preview)
    expect(mocks.readInstanceBundle).toHaveBeenCalledWith('C:/x/dsh-bundle.json')
  })

  it('imports with the instance conventions the launch pipeline assumes', async () => {
    const record = { id: 'rec-1', name: 'Bundled' }
    mocks.importInstanceBundle.mockResolvedValue({ record, credentials: preview.credentials })
    const out = await importBundleAsInstance('C:/x/b.json', preview, 3180)
    const [path, manifest] = mocks.importInstanceBundle.mock.calls[0]
    expect(path).toBe('C:/x/b.json')
    // `dsh web` boots profiles/web and is the only entry parsing --port:
    // an import that drifted from these fields yields an instance that
    // cannot launch — the same promise the create wizard makes.
    expect(manifest.profile).toBe('web')
    expect(manifest.api).toEqual({ inheritance: 'default', providerIds: [] })
    expect(manifest.port).toBe(3180)
    expect(manifest.autoPort).toBe(true)
    expect(manifest.versionId).toBe(preview.versionId)
    expect(manifest.runtimeId).toBe(preview.runtimeId)
    expect(out.credentials).toEqual(preview.credentials)
    expect(out.instance).toBeTruthy()
  })

  it('gives the imported instance a fresh id, never the one carried by the bundle', async () => {
    mocks.importInstanceBundle.mockResolvedValue({ record: { id: 'r' }, credentials: [] })
    await importBundleAsInstance('p', preview, 3180)
    const manifest = mocks.importInstanceBundle.mock.calls[0][1]
    expect(manifest.id).not.toBe('')
    expect(manifest.id.startsWith('bundled')).toBe(true)
  })
})
