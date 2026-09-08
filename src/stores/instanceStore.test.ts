import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Instance, InstanceRuntimeState } from '@/types'

const mocks = vi.hoisted(() => ({
  stop: vi.fn(), launch: vi.fn(), saveInstance: vi.fn(), toast: vi.fn(), listInstances: vi.fn(),
  openDshWebUi: vi.fn(),
  cloneInstance: vi.fn(),
  onExit: null as null | ((event: { instanceId: string; pid: number; code: number | null }) => void),
}))
vi.mock('@/services', async () => ({
  ...await import('@/services/repository'),
  repository: mocks,
}))
vi.mock('@/lib/desktop', () => ({
  isDesktop: true,
  openDshWebUi: mocks.openDshWebUi,
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
  mocks.openDshWebUi.mockResolvedValue({ ok: true, how: 'window' })
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

  it('keeps a durable abnormal-exit mark so a crash outlives its toast', () => {
    // Found in the 2026-09-08 desktop alpha pass: a process that reaches
    // ready and then dies folded the state back to stopped with only a
    // transient toast — the card claimed a healthy 已停止. The runtime state
    // must retain the exit facts until the next launch clears them.
    mocks.onExit!({ instanceId: 'test', pid: 123, code: 1 })
    const after = useInstanceStore.getState().stateOf('test')
    expect(after.status).toBe('stopped')
    expect(after.lastExit).toEqual(
      expect.objectContaining({ code: 1, ranFor: expect.any(Number) }),
    )
  })

  it('a clean exit leaves no abnormal mark', () => {
    mocks.onExit!({ instanceId: 'test', pid: 123, code: 0 })
    expect(useInstanceStore.getState().stateOf('test').lastExit).toBeUndefined()
  })

  it('a failed clone raises an error toast instead of swallowing the rejection', async () => {
    // Found in the 2026-09-08 desktop alpha pass: the backend refuses to
    // clone a tree containing symlinks, the store let the rejection escape
    // unhandled, and the UI silently kept showing the old count.
    mocks.cloneInstance?.mockRejectedValue(new Error('复制源包含符号链接'))
    const clone = await useInstanceStore.getState().cloneInstance('test', 'Test 2')
    expect(clone).toBeNull()
    expect(mocks.toast).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'error', message: expect.stringContaining('符号链接') }),
    )
    expect(mocks.toast).not.toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'success' }),
    )
  })

  it('surfaces a WebUI that could not be opened at all', async () => {
    // Both halves of the contract failed — the embedded window was refused and
    // the system-browser fallback threw. The old code let the second rejection
    // vanish into a `void` call site: a button that did nothing, with no way
    // for the user to know why.
    mocks.openDshWebUi.mockResolvedValue({
      ok: false,
      reason: '内嵌窗口失败：拒绝访问；改用系统浏览器也失败：找不到处理程序',
    })
    await useInstanceStore.getState().openWebUi('test')
    expect(mocks.toast).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: 'error',
        title: expect.stringContaining('无法打开'),
        message: expect.stringContaining('系统浏览器'),
      }),
    )
  })

  it('stays quiet when the WebUI opened in a window', async () => {
    mocks.openDshWebUi.mockResolvedValue({ ok: true, how: 'window' })
    await useInstanceStore.getState().openWebUi('test')
    expect(mocks.toast).not.toHaveBeenCalled()
  })
})
