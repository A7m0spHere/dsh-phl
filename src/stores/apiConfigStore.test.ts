import { beforeEach, expect, it, vi } from 'vitest'
import type { ApiConfig } from '@/types'

const mocks = vi.hoisted(() => ({
  root: 'old-root', loadApiConfig: vi.fn(), saveApiConfig: vi.fn(), importInstanceApi: vi.fn(), toast: vi.fn(),
}))
vi.mock('@/lib/desktop', () => ({ isDesktop: true, ...mocks }))
vi.mock('./settingsStore', () => ({ useSettingsStore: { getState: () => ({ root: mocks.root }) } }))
vi.mock('./instanceStore', () => ({ useInstanceStore: { getState: () => ({ instances: [] }) } }))
vi.mock('./uiStore', () => ({ useUIStore: { getState: () => ({ toast: mocks.toast }) } }))
import { useApiConfigStore } from './apiConfigStore'

const config: ApiConfig = { version: 1, updatedAt: '', providers: [] }
beforeEach(() => {
  vi.resetAllMocks()
  mocks.root = 'old-root'
  useApiConfigStore.setState({ config, loaded: true, saving: false, snapshots: {} })
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
