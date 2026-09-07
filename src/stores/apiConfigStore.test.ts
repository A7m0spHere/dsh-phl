import { beforeEach, expect, it, vi } from 'vitest'
import type { ApiConfig } from '@/types'

const mocks = vi.hoisted(() => ({
  root: 'old-root', loadApiConfig: vi.fn(), saveApiConfig: vi.fn(), importInstanceApi: vi.fn(), toast: vi.fn(), enrichModelMetadata: vi.fn(),
}))
vi.mock('@/lib/desktop', () => ({ isDesktop: true, ...mocks }))
vi.mock('@/services', () => ({ repository: { enrichModelMetadata: mocks.enrichModelMetadata } }))
vi.mock('./settingsStore', () => ({ useSettingsStore: { getState: () => ({ root: mocks.root }) } }))
vi.mock('./instanceStore', () => ({ useInstanceStore: { getState: () => ({ instances: [] }) } }))
vi.mock('./uiStore', () => ({ useUIStore: { getState: () => ({ toast: mocks.toast }) } }))
import { useApiConfigStore } from './apiConfigStore'

const config: ApiConfig = { version: 1, updatedAt: '', providers: [] }
beforeEach(() => {
  vi.resetAllMocks()
  mocks.root = 'old-root'
  useApiConfigStore.setState({ config, loaded: true, saving: false, enriching: false, snapshots: {} })
})

it('surfaces desktop read failures instead of replacing the library with browser data', async () => {
  mocks.loadApiConfig.mockRejectedValue(new Error('read denied'))
  await expect(useApiConfigStore.getState().load()).rejects.toThrow('read denied')
  expect(useApiConfigStore.getState().loaded).toBe(false)
  expect(useApiConfigStore.getState().config).toBeNull()
})

it('ignores a load response from a root that is no longer selected', async () => {
  let resolve!: (value: ApiConfig) => void
  mocks.loadApiConfig.mockReturnValue(new Promise<ApiConfig>((ok) => { resolve = ok }))
  const pending = useApiConfigStore.getState().load()
  mocks.root = 'new-root'
  resolve(config)
  await pending
  expect(useApiConfigStore.getState().config).toBeNull()
})

it('reports an import as failed if persisting it fails', async () => {
  mocks.importInstanceApi.mockResolvedValue(config)
  mocks.saveApiConfig.mockRejectedValue(new Error('disk full'))
  expect(await useApiConfigStore.getState().importFromInstance('test')).toBeNull()
})

it('does not start a second API write while the current one is pending', async () => {
  useApiConfigStore.setState({ saving: true })
  expect(await useApiConfigStore.getState().save(config)).toBeNull()
  expect(mocks.saveApiConfig).not.toHaveBeenCalled()
})

const library: ApiConfig = { version: 1, updatedAt: '', providers: [{ id: 'p', name: 'openai', kind: 'custom', enabled: true, apiKeyEnv: 'KEY', models: [{ id: 'gpt', contextWindow: 123 }] }] }
const batch = { catalogStatus: 'fresh', results: [{ model: { id: 'gpt', contextWindow: 123, maxTokens: 4096, metadataSources: { maxTokens: 'models.dev' } }, matched: true, changed: true, ambiguous: false }] }

it('global enrichment uses provider names and saves the enriched library without syncing instances', async () => {
  useApiConfigStore.setState({ config: library })
  mocks.enrichModelMetadata.mockResolvedValue(batch)
  mocks.saveApiConfig.mockImplementation(async (value) => value)
  await useApiConfigStore.getState().enrichMissingModels()
  expect(mocks.enrichModelMetadata).toHaveBeenCalledWith({ provider: 'openai', models: library.providers[0].models })
  expect(useApiConfigStore.getState().config?.providers[0].models[0]).toEqual(batch.results[0].model)
  expect(mocks.toast).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringContaining('补全 1 个') }))
})

it.each(['edit', 'root'])('global enrichment discards stale responses after %s', async (change) => {
  useApiConfigStore.setState({ config: library })
  let resolve!: (value: typeof batch) => void
  mocks.enrichModelMetadata.mockReturnValue(new Promise((ok) => { resolve = ok }))
  const pending = useApiConfigStore.getState().enrichMissingModels()
  if (change === 'root') mocks.root = 'new-root'
  else useApiConfigStore.setState({ config: { ...library, providers: [] } })
  resolve(batch)
  await pending
  expect(mocks.saveApiConfig).not.toHaveBeenCalled()
  expect(useApiConfigStore.getState().enriching).toBe(false)
})

it('failed enrichment never writes a partial library', async () => {
  useApiConfigStore.setState({ config: library })
  mocks.enrichModelMetadata.mockRejectedValue(new Error('IPC unavailable'))
  await useApiConfigStore.getState().enrichMissingModels()
  expect(mocks.saveApiConfig).not.toHaveBeenCalled()
  expect(useApiConfigStore.getState().config).toBe(library)
  expect(useApiConfigStore.getState().enriching).toBe(false)
})
