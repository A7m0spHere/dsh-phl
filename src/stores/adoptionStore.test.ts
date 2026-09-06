import { beforeEach, expect, it, vi } from 'vitest'
import type { RemoteDshCandidate, RemoteSessionInfo } from '@/lib/desktop'

function session(id: string): RemoteSessionInfo {
  return {
    sessionDir: `session-${id}`,
    project: '--P--',
    id: `session-${id}`,
    createdAtMs: 1_700_000_000_000,
    cwd: '/work/p',
    parent: null,
    originSubagent: false,
  }
}

function selectedPreview(over: Record<string, unknown> = {}) {
  return {
    sourceHome: '/home/me/.dsh', mode: 'managed-copy', sessionStrategy: 'selected',
    profile: 'web', detectedVersion: '0.1.2-rc.1', pluginCount: 5, sessionCount: 12,
    copyBytes: 30, symlinkEntries: 0, warnings: [], keepsExistingSessions: false,
    selectedSessionCount: 1, ...over,
  }
}

const mocks = vi.hoisted(() => ({
  adoptInstance: vi.fn(),
  previewAdoption: vi.fn(),
  discoverDsh: vi.fn(),
  inspectDshHome: vi.fn(),
  inspectDshExecutable: vi.fn(),
  listHomeSessions: vi.fn(),
  chooseDshHome: vi.fn(),
  chooseDshExecutable: vi.fn(),
  toast: vi.fn(),
  admit: vi.fn(),
  load: vi.fn(),
  push: vi.fn(),
}))
vi.mock('@/lib/desktop', () => ({
  isDesktop: true,
  adoptInstance: mocks.adoptInstance,
  previewAdoption: mocks.previewAdoption,
  discoverDsh: mocks.discoverDsh,
  inspectDshHome: mocks.inspectDshHome,
  inspectDshExecutable: mocks.inspectDshExecutable,
  listHomeSessions: mocks.listHomeSessions,
  chooseDshHome: mocks.chooseDshHome,
  chooseDshExecutable: mocks.chooseDshExecutable,
}))
vi.mock('@/lib/desktopCore', () => ({ isDesktop: true }))
vi.mock('./instanceStore', () => ({
  useInstanceStore: { getState: () => ({ admitInstance: mocks.admit, load: mocks.load }) },
}))
vi.mock('./uiStore', () => ({ useUIStore: { getState: () => ({ toast: mocks.toast, push: mocks.push }) } }))
import { useAdoptionStore } from './adoptionStore'

function candidate(over: Partial<RemoteDshCandidate> = {}): RemoteDshCandidate {
  return {
    id: 'dsh-abc',
    displayName: '~/.dsh',
    dshHome: '/home/me/.dsh',
    executablePath: null,
    detectedVersion: '0.1.2-rc.1',
    nodePath: 'node',
    nodeVersion: '24.0.0',
    profile: 'web',
    pluginCount: 5,
    sessionCount: 12,
    sizeBytes: 1024,
    source: 'defaultHome',
    confidence: 'high',
    warnings: [],
    alreadyManaged: false,
    managedInstance: null,
    ...over,
  }
}

beforeEach(() => {
  vi.resetAllMocks()
  useAdoptionStore.getState().reset()
  useAdoptionStore.setState({ candidates: [candidate()] })
})

it('choosing external mode forces the session strategy to all', () => {
  useAdoptionStore.getState().setSessionStrategy('none')
  useAdoptionStore.getState().setMode('external')
  expect(useAdoptionStore.getState().mode).toBe('external')
  expect(useAdoptionStore.getState().sessionStrategy).toBe('all')
})

it('keeps an explicit strategy when returning to copy mode', () => {
  useAdoptionStore.getState().setMode('external') // forces 'all'
  useAdoptionStore.getState().setMode('managed-copy')
  useAdoptionStore.getState().setSessionStrategy('none')
  expect(useAdoptionStore.getState().sessionStrategy).toBe('none')
})

it('does not advance from discovery when the pick is already managed', () => {
  useAdoptionStore.setState({
    candidates: [candidate({ alreadyManaged: true, managedInstance: { id: 'x', name: '实例X' } })],
    selectedId: 'dsh-abc',
  })
  useAdoptionStore.getState().toConfigure()
  expect(useAdoptionStore.getState().step).toBe('discover')
  expect(mocks.toast).toHaveBeenCalled()
})

it('records a submission failure without admitting an instance', async () => {
  useAdoptionStore.setState({ selectedId: 'dsh-abc', name: '我的 DSH' })
  mocks.previewAdoption.mockResolvedValue({
    sourceHome: '/home/me/.dsh', mode: 'managed-copy', sessionStrategy: 'all', profile: 'web',
    detectedVersion: '0.1.2-rc.1', pluginCount: 5, sessionCount: 12, copyBytes: 10,
    symlinkEntries: 0, warnings: [], keepsExistingSessions: false,
  })
  mocks.adoptInstance.mockRejectedValue(new Error('disk full'))
  await useAdoptionStore.getState().toPreview()
  await useAdoptionStore.getState().commit()
  expect(useAdoptionStore.getState().submitError).toBe('disk full')
  expect(mocks.admit).not.toHaveBeenCalled()
})

it('admits and reloads the instance list on a successful commit', async () => {
  useAdoptionStore.setState({ selectedId: 'dsh-abc', name: '我的 DSH' })
  mocks.previewAdoption.mockResolvedValue({
    sourceHome: '/home/me/.dsh', mode: 'managed-copy', sessionStrategy: 'none', profile: 'web',
    detectedVersion: '0.1.2-rc.1', pluginCount: 5, sessionCount: 0, copyBytes: 10,
    symlinkEntries: 0, warnings: [], keepsExistingSessions: false,
  })
  mocks.adoptInstance.mockImplementation(async (_req, onProgress) => {
    onProgress?.({ progress: 0.5, bytesDone: 5, bytesTotal: 10 })
    return {
      record: {
        id: 'my-dsh-1', name: '我的 DSH', kind: 'sandbox', hue: 0, versionId: '0.1.2-rc.1',
        runtimeId: 'node-24', port: 6100, autoPort: true, profile: 'web', createdAt: 'now',
        totalRuntime: 0, favorite: false, env: {}, args: [], dshHome: '/p/dsh-home',
        workspace: '/p/workspace', plugins: [], snapshots: [],
        managementMode: 'managed-copy', source: 'adopted',
      },
      adoptedFrom: { dshHome: '/home/me/.dsh', detectedVersion: '0.1.2-rc.1', adoptedAt: 'now', mode: 'managed-copy' },
    }
  })
  await useAdoptionStore.getState().toPreview()
  await useAdoptionStore.getState().commit()
  expect(mocks.admit).toHaveBeenCalled()
  expect(mocks.load).toHaveBeenCalled()
  expect(useAdoptionStore.getState().submitting).toBe(false)
})

/* ------------------------- selected-session flow ------------------------- */

it('the picker lazily lists the source home and tracks toggles', async () => {
  useAdoptionStore.setState({ selectedId: 'dsh-abc' })
  mocks.listHomeSessions.mockResolvedValue([session('a'), session('b')])
  useAdoptionStore.getState().setSessionStrategy('selected')
  await vi.waitFor(() =>
    expect(useAdoptionStore.getState().sourceSessions?.length).toBe(2),
  )
  expect(mocks.listHomeSessions).toHaveBeenCalledWith('/home/me/.dsh')
  useAdoptionStore.getState().toggleSessionDir('session-a')
  useAdoptionStore.getState().toggleSessionDir('session-b')
  expect(useAdoptionStore.getState().selectedSessionDirs).toEqual(['session-a', 'session-b'])
  useAdoptionStore.getState().toggleSessionDir('session-a')
  expect(useAdoptionStore.getState().selectedSessionDirs).toEqual(['session-b'])
})

it('choosing another source candidate clears the stale picker', () => {
  useAdoptionStore.setState({
    selectedId: 'dsh-abc',
    sourceSessions: [session('a')],
    selectedSessionDirs: ['session-a'],
    sourceSessionsError: 'boom',
  })
  useAdoptionStore.getState().select('other')
  const s = useAdoptionStore.getState()
  expect(s.sourceSessions).toBeNull()
  expect(s.selectedSessionDirs).toEqual([])
  expect(s.sourceSessionsError).toBeNull()
})

it('does not preview an empty selected-session choice', async () => {
  useAdoptionStore.setState({ selectedId: 'dsh-abc', name: 'x', sessionStrategy: 'selected' })
  await useAdoptionStore.getState().toPreview()
  expect(useAdoptionStore.getState().step).toBe('discover')
  expect(mocks.previewAdoption).not.toHaveBeenCalled()
})

it('passes the selected dirs through preview and commit', async () => {
  useAdoptionStore.setState({
    selectedId: 'dsh-abc',
    name: '我的 DSH',
    sessionStrategy: 'selected',
    selectedSessionDirs: ['session-a'],
  })
  mocks.previewAdoption.mockResolvedValue(selectedPreview())
  mocks.adoptInstance.mockResolvedValue({
    record: {
      id: 'my-dsh-1', name: '我的 DSH', kind: 'sandbox', hue: 0, versionId: '0.1.2-rc.1',
      runtimeId: 'node-24', port: 6100, autoPort: true, profile: 'web', createdAt: 'now',
      totalRuntime: 0, favorite: false, env: {}, args: [], dshHome: '/p/dsh-home',
      workspace: '/p/workspace', plugins: [], snapshots: [],
      managementMode: 'managed-copy', source: 'adopted',
    },
    adoptedFrom: { dshHome: '/home/me/.dsh', detectedVersion: '0.1.2-rc.1', adoptedAt: 'now', mode: 'managed-copy' },
  })
  await useAdoptionStore.getState().toPreview()
  const previewReq = mocks.previewAdoption.mock.calls[0][0]
  expect(previewReq.sessionStrategy).toBe('selected')
  expect(previewReq.sessionDirs).toEqual(['session-a'])
  await useAdoptionStore.getState().commit()
  const commitReq = mocks.adoptInstance.mock.calls[0][0]
  expect(commitReq.sessionDirs).toEqual(['session-a'])
  expect(mocks.toast).toHaveBeenCalledWith(
    expect.objectContaining({ message: expect.stringContaining('已迁移 1 条') }),
  )
})
