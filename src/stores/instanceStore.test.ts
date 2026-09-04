import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Instance, InstanceRuntimeState } from '@/types'

const mocks = vi.hoisted(() => ({
  stop: vi.fn(), launch: vi.fn(), saveInstance: vi.fn(), toast: vi.fn(), listInstances: vi.fn(),
  onExit: null as null | ((event: { instanceId: string; pid: number; code: number | null }) => void),
}))
vi.mock('@/services', async () => ({
  ...await import('@/services/repository'),
  repository: mocks,
}))
vi.mock('@/lib/desktop', () => ({
  isDesktop: true, openExternal: vi.fn(),
  onInstanceExited: async (handler: typeof mocks.onExit) => { mocks.onExit = handler; return () => {} },
}))
vi.mock('./catalogStore', () => ({ useCatalogStore: { getState: () => ({
  pluginTransfers: {}, versionById: vi.fn(), runtimeById: vi.fn(),
}) } }))
vi.mock('./uiStore', () => ({ useUIStore: { getState: () => ({ toast: mocks.toast }) } }))

import { useInstanceStore } from './instanceStore'

const instance: Instance = {
  id: 'test', name: 'Test', kind: 'sandbox', hue: 0, versionId: 'dsh-1', runtimeId: 'node-22',
  port: 3080, autoPort: true, dshHome: '', workspace: '', profile: 'web', plugins: [],
  createdAt: '', totalRuntime: 100, diskUsage: 0, env: {}, args: [], snapshots: [],
}
const running: InstanceRuntimeState = {
  status: 'running', pid: 123, startedAt: Date.now() - 10_000, webUrl: 'http://localhost:3080/?token=test',
}

beforeEach(async () => {
  await useInstanceStore.getState().flushWrites()
  vi.clearAllMocks()
  mocks.saveInstance.mockResolvedValue(undefined)
  mocks.listInstances.mockResolvedValue([instance])
  await useInstanceStore.getState().reload()
  useInstanceStore.setState({ instances: [{ ...instance }], states: { test: { ...running } } })
})

describe('instance lifecycle', () => {
  it('keeps a failed stop tracked with its URL, port and original start time', async () => {
    mocks.stop.mockRejectedValue(new Error('access denied'))
    expect(await useInstanceStore.getState().stop('test')).toBe(false)
    expect(useInstanceStore.getState().stateOf('test')).toEqual(running)
    expect(useInstanceStore.getState().portsInUse().has(3080)).toBe(true)
    expect(mocks.saveInstance).not.toHaveBeenCalled()
    expect(mocks.toast).toHaveBeenCalledWith(expect.objectContaining({ kind: 'error' }))
  })

  it('banks runtime once when the exit event races a successful stop', async () => {
    mocks.stop.mockImplementation(async () => {
      mocks.onExit!({ instanceId: 'test', pid: 123, code: 0 })
      expect(useInstanceStore.getState().stateOf('test').status).toBe('stopping')
    })
    expect(await useInstanceStore.getState().stop('test')).toBe(true)
    await useInstanceStore.getState().flushWrites()
    expect(useInstanceStore.getState().stateOf('test').status).toBe('stopped')
    expect(useInstanceStore.getState().byId('test')!.totalRuntime).toBeGreaterThanOrEqual(110)
    expect(mocks.saveInstance).toHaveBeenCalledTimes(1)
  })

  it('accepts confirmed process exit even if taskkill then reports failure', async () => {
    mocks.stop.mockImplementation(async () => {
      mocks.onExit!({ instanceId: 'test', pid: 123, code: 0 })
      throw new Error('already exited')
    })
    expect(await useInstanceStore.getState().stop('test')).toBe(true)
    expect(useInstanceStore.getState().stateOf('test').status).toBe('stopped')
  })

  it('does not write the manifest for derived state updates', () => {
    useInstanceStore.getState().updateInstance('test', { diskUsage: 42, plugins: [], snapshots: [] })
    expect(useInstanceStore.getState().byId('test')!.diskUsage).toBe(42)
    expect(mocks.saveInstance).not.toHaveBeenCalled()
  })

  it('ignores a delayed exit event from an earlier process', () => {
    mocks.onExit!({ instanceId: 'test', pid: 122, code: 1 })
    expect(useInstanceStore.getState().stateOf('test')).toEqual(running)
  })

  it('does not report running when the exit event precedes the launch result', async () => {
    useInstanceStore.setState({ states: {} })
    mocks.launch.mockImplementation(async () => {
      mocks.onExit!({ instanceId: 'test', pid: 124, code: 1 })
      return { pid: 124, port: 3080 }
    })
    await useInstanceStore.getState().launch('test')
    expect(useInstanceStore.getState().stateOf('test').status).toBe('error')
    expect(mocks.toast).not.toHaveBeenCalledWith(expect.objectContaining({ kind: 'success' }))
  })
})
