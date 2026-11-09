import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { DshVersion, Instance, InstanceDraft, Runtime } from '@/types'

const mocks = vi.hoisted(() => ({
  installVersion: vi.fn(), installRuntime: vi.fn(), createInstance: vi.fn(),
  launch: vi.fn(), stop: vi.fn(), saveInstance: vi.fn(), openDshWebUi: vi.fn(),
}))
vi.mock('@/services', async () => ({ ...await import('@/services/repository'), repository: mocks }))
vi.mock('@/lib/desktop', () => ({
  isDesktop: true, openDshWebUi: mocks.openDshWebUi,
  onInstanceExited: async () => () => {}, onWebuiPageError: async () => () => {},
}))

import { Cancelled, LaunchError } from '@/services'
import { useCatalogStore } from '@/stores/catalogStore'
import { useInstanceStore } from '@/stores/instanceStore'
import { useSettingsStore } from '@/stores/settingsStore'
import { useSetupStore } from './setupStore'

const version: DshVersion = {
  id: 'dsh-1.0.0', name: '1.0.0', channel: 'stable', releasedAt: '2026-11-01',
  requiresNode: [24], size: 1, notes: [], state: { kind: 'available' },
}
const runtime: Runtime = {
  id: 'node-24.1.0', name: 'Node 24', version: '24.1.0', major: 24, lts: true,
  size: 1, state: { kind: 'available' },
}

function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>((done) => { resolve = done })
  return { promise, resolve }
}

beforeEach(async () => {
  await useInstanceStore.getState().flushWrites()
  vi.resetAllMocks()
  useSettingsStore.setState({ root: 'D:/phl-setup-test', guideSeen: false, autoOpenWebUi: true })
  useCatalogStore.setState({ versions: [{ ...version }], runtimes: [{ ...runtime }], loaded: true })
  useInstanceStore.setState({ instances: [], states: {}, createProgress: null })
  useSetupStore.setState({ instanceId: null, status: 'idle', open: false })
  mocks.installRuntime.mockResolvedValue(undefined)
  mocks.installVersion.mockResolvedValue(undefined)
  mocks.saveInstance.mockResolvedValue(undefined)
  mocks.stop.mockResolvedValue(undefined)
  mocks.openDshWebUi.mockResolvedValue({ ok: true, how: 'window' })
  mocks.createInstance.mockImplementation(async (draft: InstanceDraft) => ({
    ...draft, id: 'first-instance', profile: 'web', dshHome: '', workspace: '',
    plugins: [], snapshots: [], createdAt: '', totalRuntime: 0, diskUsage: 0, env: {}, args: [],
  } as Instance))
  mocks.launch.mockResolvedValue({ pid: 100, port: 3080, webUrl: 'http://localhost:3080/?token=test' })
  useSetupStore.getState().show()
})

describe('first-run setup', () => {
  it('installs Node before DSH, then creates, launches and opens exactly once', async () => {
    const order: string[] = []
    mocks.installRuntime.mockImplementation(async () => { order.push('runtime') })
    mocks.installVersion.mockImplementation(async () => { order.push('version') })
    mocks.createInstance.mockImplementationOnce(async (draft: InstanceDraft) => {
      order.push('create')
      return { ...draft, id: 'first-instance', plugins: [], totalRuntime: 0 } as unknown as Instance
    })
    mocks.launch.mockImplementation(async () => {
      order.push('launch')
      return { pid: 100, port: 3080, webUrl: 'http://localhost:3080/?token=test' }
    })
    mocks.openDshWebUi.mockImplementation(async () => { order.push('open'); return { ok: true } })
    await useSetupStore.getState().run()
    expect(order).toEqual(['runtime', 'version', 'create', 'launch', 'open'])
    expect(useSetupStore.getState().status).toBe('complete')
    expect(useSettingsStore.getState().guideSeen).toBe(true)
    expect(mocks.openDshWebUi).toHaveBeenCalledTimes(1)
  })

  it('stops after a download failure and retries without duplicating completed components', async () => {
    mocks.installVersion.mockRejectedValueOnce(new Error('registry timed out'))
    await useSetupStore.getState().run()
    expect(useSetupStore.getState()).toMatchObject({ status: 'failed', instanceId: null })
    expect(useSetupStore.getState().error).toContain('registry timed out')
    expect(mocks.createInstance).not.toHaveBeenCalled()
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().status).toBe('complete')
    expect(mocks.installRuntime).toHaveBeenCalledTimes(1)
    expect(mocks.createInstance).toHaveBeenCalledTimes(1)
  })

  it('preserves a runtime failure reason and creates no instance', async () => {
    mocks.installRuntime.mockRejectedValueOnce(new Error('checksum mismatch'))
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().error).toContain('checksum mismatch')
    expect(mocks.installVersion).not.toHaveBeenCalled()
    expect(mocks.createInstance).not.toHaveBeenCalled()
  })

  it('reuses the same instance after launch failure', async () => {
    mocks.launch.mockRejectedValueOnce(new LaunchError('启动失败', '端口被占用'))
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().status).toBe('failed')
    expect(useSetupStore.getState().error).toContain('端口被占用')
    expect(useSettingsStore.getState().guideSeen).toBe(false)
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().status).toBe('complete')
    expect(mocks.createInstance).toHaveBeenCalledTimes(1)
  })

  it('retries only opening WebUI when the window fails', async () => {
    mocks.openDshWebUi.mockResolvedValueOnce({ ok: false, reason: 'window failed' })
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().status).toBe('failed')
    expect(useSetupStore.getState().error).toContain('window failed')
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().status).toBe('complete')
    expect(mocks.launch).toHaveBeenCalledTimes(1)
    expect(mocks.createInstance).toHaveBeenCalledTimes(1)
    expect(mocks.openDshWebUi).toHaveBeenCalledTimes(2)
  })

  it('rejects incompatible selections before any download or creation', async () => {
    useCatalogStore.setState({ runtimes: [{ ...runtime, major: 20 }] })
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().error).toContain('不匹配')
    expect(mocks.installRuntime).not.toHaveBeenCalled()
  })

  it('ignores repeated start clicks and cancels a download before creating', async () => {
    const started = deferred()
    mocks.installRuntime.mockImplementation(async (_runtime, _progress, signal: AbortSignal) => {
      started.resolve()
      await new Promise<void>((_resolve, reject) => signal.addEventListener('abort', () => reject(new Cancelled())))
    })
    const run = useSetupStore.getState().run()
    await started.promise
    await useSetupStore.getState().run()
    useSetupStore.getState().cancel()
    expect(useSetupStore.getState().status).toBe('cancelling')
    await run
    expect(useSetupStore.getState().status).toBe('cancelled')
    expect(mocks.installRuntime).toHaveBeenCalledTimes(1)
    expect(mocks.createInstance).not.toHaveBeenCalled()
  })

  it('stops a launch that finishes while cancellation is arriving and can resume it', async () => {
    const started = deferred()
    const finish = deferred()
    mocks.launch.mockImplementationOnce(async () => {
      started.resolve()
      await finish.promise
      return { pid: 100, port: 3080, webUrl: 'http://localhost:3080/?token=test' }
    })
    const run = useSetupStore.getState().run()
    await started.promise
    useSetupStore.getState().cancel()
    finish.resolve()
    await run
    expect(useSetupStore.getState().status).toBe('cancelled')
    expect(mocks.stop).toHaveBeenCalledTimes(1)
    expect(mocks.openDshWebUi).not.toHaveBeenCalled()
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().status).toBe('complete')
    expect(mocks.createInstance).toHaveBeenCalledTimes(1)
  })

  it('refuses to borrow or cancel another workflow transfer', async () => {
    useCatalogStore.setState({ versions: [{ ...version, state: { kind: 'queued' } }] })
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().error).toContain('其他任务')
    expect(mocks.installRuntime).not.toHaveBeenCalled()
  })

  it('keeps an atomically committed instance when cancellation races creation', async () => {
    const started = deferred()
    const finish = deferred()
    mocks.createInstance.mockImplementationOnce(async (draft: InstanceDraft) => {
      started.resolve()
      await finish.promise
      return { ...draft, id: 'first-instance', plugins: [], totalRuntime: 0 } as unknown as Instance
    })
    const run = useSetupStore.getState().run()
    await started.promise
    useSetupStore.getState().cancel()
    finish.resolve()
    await run
    expect(useSetupStore.getState()).toMatchObject({ status: 'cancelled', instanceId: 'first-instance' })
    expect(useInstanceStore.getState().createProgress).toBeNull()
    expect(mocks.launch).not.toHaveBeenCalled()
    useSetupStore.getState().configure({ name: 'another name' })
    expect(useSetupStore.getState().name).toBe('我的 DSH')
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().status).toBe('complete')
    expect(mocks.createInstance).toHaveBeenCalledTimes(1)
  })

  it('does not report cancellation as successful while a surviving process cannot be stopped', async () => {
    const started = deferred()
    const finish = deferred()
    mocks.launch.mockImplementationOnce(async () => {
      started.resolve()
      await finish.promise
      return { pid: 100, port: 3080, webUrl: 'http://localhost:3080/?token=test' }
    })
    mocks.stop.mockRejectedValueOnce(new Error('access denied'))
    const run = useSetupStore.getState().run()
    await started.promise
    useSetupStore.getState().cancel()
    finish.resolve()
    await run
    expect(useSetupStore.getState().status).toBe('failed')
    expect(useSetupStore.getState().error).toContain('未能确认')
    expect(useInstanceStore.getState().stateOf('first-instance').status).toBe('running')
    expect(mocks.openDshWebUi).not.toHaveBeenCalled()
  })

  it('does not continue creation after a root switch', async () => {
    mocks.installVersion.mockImplementationOnce(async () => {
      useSettingsStore.setState({ root: 'D:/another-root' })
    })
    await useSetupStore.getState().run()
    expect(useSetupStore.getState().error).toContain('数据目录已变更')
    expect(mocks.createInstance).not.toHaveBeenCalled()
  })
})
