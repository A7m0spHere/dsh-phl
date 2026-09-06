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
