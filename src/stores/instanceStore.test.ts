import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Instance, InstanceRuntimeState } from '@/types'

const mocks = vi.hoisted(() => ({
  stop: vi.fn(), launch: vi.fn(), saveInstance: vi.fn(), toast: vi.fn(), listInstances: vi.fn(),
  openDshWebUi: vi.fn(),
  cloneInstance: vi.fn(),
  createSnapshot: vi.fn(), restoreSnapshot: vi.fn(), deleteSnapshot: vi.fn(),
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

  it('shows a surviving process as running and stoppable after an aborted launch', async () => {
    // R3: the backend could not confirm termination after a cancel, so it
    // kept the registration. The UI must present a *running, stoppable*
    // instance — not a failed start with no stop entry, which is how a live
    // DSH used to silently escape PHL's management.
    useInstanceStore.setState({ states: {} })
    const { KeptRunningError } = await import('@/services/repository')
    mocks.launch.mockRejectedValue(
      new KeptRunningError(4321, 3180, '启动已取消，但进程 4321 在终止后仍然存活'),
    )
    await useInstanceStore.getState().launch('test')
    const s = useInstanceStore.getState().stateOf('test')
    expect(s.status).toBe('running')
    expect(s.pid).toBe(4321)
    expect(useInstanceStore.getState().byId('test')!.port).toBe(3180)
    expect(mocks.toast).toHaveBeenCalledWith(expect.objectContaining({ kind: 'warn' }))
    // The stop entry is live: stop reaches the backend and clears the card.
    mocks.stop.mockResolvedValue(undefined)
    expect(await useInstanceStore.getState().stop('test')).toBe(true)
    expect(useInstanceStore.getState().stateOf('test').status).toBe('stopped')
  })

  it('does not resurrect a kept process whose exit preceded the launch error', async () => {
    useInstanceStore.setState({ states: {} })
    const { KeptRunningError } = await import('@/services/repository')
    mocks.launch.mockImplementation(async () => {
      mocks.onExit!({ instanceId: 'test', pid: 4321, code: 0 })
      throw new KeptRunningError(4321, 3180, 'termination unconfirmed')
    })
    await useInstanceStore.getState().launch('test')
    expect(useInstanceStore.getState().stateOf('test').status).toBe('stopped')
    expect(useInstanceStore.getState().portsInUse().has(3180)).toBe(false)
    expect(mocks.toast).not.toHaveBeenCalledWith(expect.objectContaining({ kind: 'warn' }))
  })

  it('observes a kept process exiting after the launch error', async () => {
    useInstanceStore.setState({ states: {} })
    const { KeptRunningError } = await import('@/services/repository')
    mocks.launch.mockRejectedValue(new KeptRunningError(4321, 3180, 'termination unconfirmed'))
    await useInstanceStore.getState().launch('test')
    mocks.onExit!({ instanceId: 'test', pid: 4321, code: 1 })
    expect(useInstanceStore.getState().stateOf('test').status).toBe('stopped')
    expect(useInstanceStore.getState().portsInUse().has(3180)).toBe(false)
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

  it('marks the source cloning, absorbs a second click, and clears when done', async () => {
    // Clone copies the whole dsh-home (minutes on a heavy instance). Before
    // the pending fix the source row sat unchanged and repeat clicks stacked
    // backend busy-lock toasts.
    let release!: () => void
    mocks.cloneInstance.mockImplementation(
      () =>
        new Promise((resolve) => {
          release = () =>
            resolve({ ...instance, id: 'clone-1', name: 'Test Copy', port: 3099 })
        }),
    )
    const first = useInstanceStore.getState().cloneInstance('test', 'Test Copy')
    expect(useInstanceStore.getState().cloning['test']).toBe(true)
    const second = await useInstanceStore.getState().cloneInstance('test', 'Test Copy 2')
    expect(second).toBeNull()
    expect(mocks.cloneInstance).toHaveBeenCalledTimes(1)
    release()
    await first
    expect(useInstanceStore.getState().cloning['test']).toBeUndefined()
    expect(useInstanceStore.getState().byId('clone-1')).toBeTruthy()
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

describe('snapshot copy operations', () => {
  const stopped = () => useInstanceStore.setState({ states: { test: { status: 'stopped' } } })

  it('restore marks itself busy, streams progress, and blocks a second copy', async () => {
    stopped()
    let release!: () => void
    mocks.restoreSnapshot.mockImplementation(
      (_i: unknown, _s: string, onProgress: (p: unknown) => void) => {
        onProgress({ progress: 0.5, bytesDone: 50, bytesTotal: 100 })
        return new Promise((resolve) => {
          release = () => resolve({ ...instance, plugins: [{ pluginId: 'p', version: '1', linked: true }] })
        })
      },
    )
    const run = useInstanceStore.getState().restoreSnapshot('test', 'snap-1')
    // A restore in flight is visible as pending with live byte progress.
    expect(useInstanceStore.getState().snapshotOps['test']).toBe('restore')
    expect(useInstanceStore.getState().snapshotTransfers['test']).toMatchObject({ bytesTotal: 100 })
    // A repeat click must not start a second copy (it would hit the same
    // instance lock and surface a busy error).
    await useInstanceStore.getState().restoreSnapshot('test', 'snap-1')
    expect(mocks.restoreSnapshot).toHaveBeenCalledTimes(1)
    release()
    await run
    // And the pending/progress state is cleared once it resolves.
    expect(useInstanceStore.getState().snapshotOps['test']).toBeUndefined()
    expect(useInstanceStore.getState().snapshotTransfers['test']).toBeUndefined()
    expect(mocks.toast).toHaveBeenCalledWith(expect.objectContaining({ kind: 'success' }))
  })

  it('create and restore share the one-copy-per-instance guard', async () => {
    stopped()
    let release!: () => void
    mocks.createSnapshot.mockImplementation(
      () => new Promise((resolve) => { release = () => resolve({ id: 'snap-x', label: '', createdAt: '', versionId: '', runtimeId: '', pluginCount: 0, size: 0 }) }),
    )
    const run = useInstanceStore.getState().createSnapshot('test')
    expect(useInstanceStore.getState().snapshotOps['test']).toBe('create')
    // While a create holds the instance, a restore is refused up front.
    await useInstanceStore.getState().restoreSnapshot('test', 'snap-1')
    expect(mocks.restoreSnapshot).not.toHaveBeenCalled()
    release()
    await run
  })

  it('delete marks the snapshot pending and absorbs a repeat click', async () => {
    stopped()
    let release!: () => void
    mocks.deleteSnapshot.mockImplementation(
      () => new Promise<void>((resolve) => { release = () => resolve() }),
    )
    const run = useInstanceStore.getState().deleteSnapshot('test', 'snap-1')
    expect(useInstanceStore.getState().deletingSnapshots['test:snap-1']).toBe(true)
    await useInstanceStore.getState().deleteSnapshot('test', 'snap-1')
    expect(mocks.deleteSnapshot).toHaveBeenCalledTimes(1)
    release()
    await run
    expect(useInstanceStore.getState().deletingSnapshots['test:snap-1']).toBeUndefined()
  })
})
