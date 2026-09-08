import { beforeEach, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  choosePackOpenPath: vi.fn(),
  previewPack: vi.fn(),
  installPack: vi.fn(),
  admit: vi.fn(),
  load: vi.fn(),
  toast: vi.fn(),
  push: vi.fn(),
}))
vi.mock('@/lib/desktop', () => ({
  isDesktop: true,
  choosePackOpenPath: mocks.choosePackOpenPath,
  previewPack: mocks.previewPack,
  installPack: mocks.installPack,
}))
vi.mock('@/lib/desktopCore', () => ({ isDesktop: true }))
vi.mock('./instanceStore', () => ({
  useInstanceStore: { getState: () => ({ admitInstance: mocks.admit, load: mocks.load }) },
}))
vi.mock('./uiStore', () => ({ useUIStore: { getState: () => ({ toast: mocks.toast, push: mocks.push }) } }))
import { usePackStore } from './packStore'

function preview(over: Record<string, unknown> = {}) {
  return {
    packId: 'demo',
    name: 'Demo Pack',
    version: '1.0.0',
    author: '',
    description: '',
    icon: null,
    dshVersion: '0.1.2-rc.1',
    runtime: 'node-22',
    pluginCount: 1,
    embeddedPluginCount: 1,
    sessionsIncluded: false,
    sessionCount: 0,
    secretsExcluded: true,
    dependencies: [],
    warnings: [],
    blocked: false,
    ...over,
  }
}

beforeEach(() => {
  vi.resetAllMocks()
  usePackStore.getState().reset()
})

it('pick then preview lands on the confirm step with the pack name as default', async () => {
  mocks.choosePackOpenPath.mockResolvedValue('/tmp/demo.phlpack')
  mocks.previewPack.mockResolvedValue(preview())
  await usePackStore.getState().pickPack()
  const s = usePackStore.getState()
  expect(s.step).toBe('preview')
  expect(s.previewState).toBe('ready')
  expect(s.name).toBe('Demo Pack')
})

it('a validation failure keeps the user on the preview step with an error', async () => {
  mocks.choosePackOpenPath.mockResolvedValue('/tmp/bad.phlpack')
  mocks.previewPack.mockRejectedValue(new Error('压缩包包含越界路径'))
  await usePackStore.getState().pickPack()
  const s = usePackStore.getState()
  expect(s.previewState).toBe('error')
  expect(s.previewError).toContain('越界')
  expect(mocks.admit).not.toHaveBeenCalled()
})

it('install admits + reloads the new instance', async () => {
  await usePackStore.setState({ step: 'preview', path: '/tmp/demo.phlpack', preview: preview(), name: 'Demo Pack' })
  mocks.installPack.mockResolvedValue({
    record: {
      id: 'demo-pack-1', name: 'Demo Pack', kind: 'sandbox', hue: 0, versionId: '0.1.2-rc.1',
      runtimeId: 'node-22', port: 0, autoPort: true, profile: 'web', createdAt: 'now', totalRuntime: 0,
      favorite: false, env: {}, args: [], dshHome: '/root/instances/demo-pack-1/dsh-home',
      workspace: '/root/instances/demo-pack-1/workspace', plugins: [], snapshots: [],
      managementMode: 'pack-installed', source: 'phlpack',
    },
    deferredPlugins: ['remote-a'],
    sessionsImported: 3,
    credentialNames: ['DEEPSEEK_API_KEY'],
  })
  await usePackStore.getState().install()
  expect(mocks.admit).toHaveBeenCalled()
  expect(mocks.load).toHaveBeenCalled()
  expect(mocks.installPack).toHaveBeenCalledWith(
    '/tmp/demo.phlpack',
    expect.objectContaining({ manifest: expect.objectContaining({ name: 'Demo Pack' }) }),
    expect.any(Function),
    expect.any(AbortSignal),
  )
  expect(usePackStore.getState().installing).toBe(false)
})

it('a failed install reports the error and does not admit', async () => {
  await usePackStore.setState({ step: 'preview', path: '/tmp/x.phlpack', preview: preview() })
  mocks.installPack.mockRejectedValue(new Error('必需依赖缺失'))
  await usePackStore.getState().install()
  expect(usePackStore.getState().installError).toContain('必需依赖缺失')
  expect(mocks.admit).not.toHaveBeenCalled()
})

const successOutcome = {
  record: {
    id: 'x-1', name: 'X', kind: 'sandbox', hue: 0, versionId: '0.1.2-rc.1',
    runtimeId: 'node-22', port: 0, autoPort: true, profile: 'web', createdAt: 'now', totalRuntime: 0,
    favorite: false, env: {}, args: [], dshHome: '/root/instances/x-1/dsh-home',
    workspace: '/root/instances/x-1/workspace', plugins: [], snapshots: [],
    managementMode: 'pack-installed', source: 'phlpack',
  },
  deferredPlugins: [], sessionsImported: 0, credentialNames: [],
}

it('reset isolates a new install from an old rejection and progress callback', async () => {
  const pending: Array<{ reject: (error: Error) => void; progress: (p: { progress: number }) => void; signal: AbortSignal }> = []
  mocks.installPack.mockImplementation((_path, _req, progress, signal) =>
    new Promise((_resolve, reject) => pending.push({ reject, progress, signal })),
  )
  usePackStore.setState({ path: '/tmp/first.phlpack', preview: preview() })
  const first = usePackStore.getState().install()
  await usePackStore.getState().install()
  expect(mocks.installPack).toHaveBeenCalledTimes(1)
  usePackStore.getState().reset()
  usePackStore.setState({ path: '/tmp/second.phlpack', preview: preview() })
  const second = usePackStore.getState().install()
  pending[0].progress({ progress: 0.9 })
  pending[0].reject(new Error('cancelled'))
  await first
  expect(usePackStore.getState().installError).toBeNull()
  expect(usePackStore.getState().installing).toBe(true)
  expect(usePackStore.getState().progress).toBe(0)
  usePackStore.getState().cancelInstall()
  expect(pending[1].signal.aborted).toBe(true)
  pending[1].reject(new Error('cancelled'))
  await second
})

it('a preview resolving after back cannot repopulate a discarded selection', async () => {
  let finish!: (value: ReturnType<typeof preview>) => void
  mocks.choosePackOpenPath.mockResolvedValue('/tmp/first.phlpack')
  mocks.previewPack.mockImplementation(() => new Promise((resolve) => { finish = resolve }))
  const picking = usePackStore.getState().pickPack()
  await vi.waitFor(() => expect(mocks.previewPack).toHaveBeenCalled())
  usePackStore.getState().back()
  finish(preview())
  await picking
  expect(usePackStore.getState().preview).toBeNull()
  expect(usePackStore.getState().step).toBe('pick')
})

it('after a failed install, back returns to the preview for a clean retry (§R4)', async () => {
  await usePackStore.setState({
    step: 'preview', path: '/tmp/x.phlpack', preview: preview(), name: 'My Instance',
  })
  mocks.installPack.mockRejectedValueOnce(new Error('必需依赖缺失'))
  await usePackStore.getState().install()
  const failed = usePackStore.getState()
  expect(failed.step).toBe('progress')
  expect(failed.installError).toContain('必需依赖缺失')

  // The failure page's "返回修改" button hits back().
  usePackStore.getState().back()
  const s = usePackStore.getState()
  expect(s.step).toBe('preview')
  expect(s.installError).toBeNull()
  expect(s.progress).toBe(0)
  // Chosen pack + edited name survive so the retry needs no re-picking.
  expect(s.path).toBe('/tmp/x.phlpack')
  expect(s.name).toBe('My Instance')

  // A retry from here works and admits — proves the round-trip is functional,
  // not just that the error text was cleared.
  mocks.installPack.mockResolvedValueOnce(successOutcome)
  await usePackStore.getState().install()
  expect(mocks.admit).toHaveBeenCalledTimes(1)
  expect(usePackStore.getState().step).toBe('progress')
})

it('back during an in-flight install is a no-op — never a second submit (§R4)', async () => {
  await usePackStore.setState({ step: 'progress', installing: true, installError: null, progress: 0.4 })
  usePackStore.getState().back()
  const s = usePackStore.getState()
  expect(s.step).toBe('progress')
  expect(s.installing).toBe(true)
  expect(mocks.installPack).not.toHaveBeenCalled()
})

it('cancelInstall aborts the live IPC attempt and reports a cancelled rollback (§R7)', async () => {
  usePackStore.setState({
    step: 'preview', path: '/tmp/large.phlpack', preview: preview(), name: 'Large',
  })
  mocks.installPack.mockImplementation(
    (_path: string, _req: unknown, _progress: unknown, signal: AbortSignal) =>
      new Promise((_resolve, reject) => {
        signal.addEventListener('abort', () => reject(new Error('cancelled')), { once: true })
      }),
  )
  const running = usePackStore.getState().install()
  await new Promise((resolve) => setTimeout(resolve, 0))
  usePackStore.getState().cancelInstall()
  await running
  expect(usePackStore.getState().installing).toBe(false)
  expect(usePackStore.getState().installError).toBe('安装已取消')
  expect(mocks.admit).not.toHaveBeenCalled()
})
