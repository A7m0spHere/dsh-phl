import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  moveRootData: vi.fn(),
  migrationStatus: vi.fn(),
  migrationUndo: vi.fn(),
  migrationFinish: vi.fn(),
  cancelTransfer: vi.fn(),
  rootDataSummary: vi.fn(),
  toast: vi.fn(),
  confirm: vi.fn(),
  setRoot: vi.fn(),
  setRootVerified: vi.fn(),
  instanceReload: vi.fn(),
  catalogLoad: vi.fn(),
  apiConfigLoad: vi.fn(),
  instanceStates: {} as Record<string, { status: string }>,
}))

vi.mock('@/lib/desktop', () => ({
  moveRootData: mocks.moveRootData,
  migrationStatus: mocks.migrationStatus,
  migrationUndo: mocks.migrationUndo,
  migrationFinish: mocks.migrationFinish,
  cancelTransfer: mocks.cancelTransfer,
  rootDataSummary: mocks.rootDataSummary,
}))
vi.mock('./uiStore', () => ({
  useUIStore: { getState: () => ({ toast: mocks.toast, confirm: mocks.confirm }) },
}))
vi.mock('./settingsStore', () => ({
  useSettingsStore: {
    getState: () => ({
      root: 'C:/PHL',
      setRoot: mocks.setRoot,
      setRootVerified: mocks.setRootVerified,
    }),
  },
}))
vi.mock('./instanceStore', () => ({
  useInstanceStore: {
    getState: () => ({
      get states() {
        return mocks.instanceStates
      },
      hasPendingWrites: () => false,
      createProgress: null,
      snapshotTransfers: {},
      reload: mocks.instanceReload,
    }),
  },
}))
vi.mock('./catalogStore', () => ({
  useCatalogStore: {
    getState: () => ({ load: mocks.catalogLoad, activeTransfers: () => 0 }),
  },
}))
vi.mock('./apiConfigStore', () => ({
  useApiConfigStore: {
    getState: () => ({ load: mocks.apiConfigLoad, saving: false, pendingSaves: 0, syncing: null }),
  },
}))

import { rootProblem, useDataRootStore } from './dataRootStore'

const migrationToasts = () => mocks.toast.mock.calls.map(([t]) => t)

beforeEach(() => {
  vi.clearAllMocks()
  mocks.instanceStates = {}
  useDataRootStore.setState({
    migration: null,
    moveProgress: null,
    journal: null,
    journalBusy: null,
    applying: false,
  })
  mocks.migrationStatus.mockResolvedValue(null)
  mocks.instanceReload.mockResolvedValue(undefined)
  mocks.catalogLoad.mockResolvedValue(undefined)
  mocks.apiConfigLoad.mockResolvedValue(undefined)
})

describe('startMigration post-commit refresh', () => {
  it('reports success only when every view actually re-read from the committed root', async () => {
    mocks.moveRootData.mockResolvedValue({
      moved: ['instances', 'versions'],
      bytes: 2048,
      cancelled: false,
      root: 'D:/PHL',
    })
    await useDataRootStore.getState().startMigration('C:/PHL', 'D:/PHL')
    expect(mocks.setRoot).toHaveBeenCalledWith('D:/PHL')
    const titles = migrationToasts().map((t) => t.title)
    expect(titles).toContain('数据迁移完成')
    expect(titles.some((t) => String(t).includes('未能刷新'))).toBe(false)
  })

  it('names the refresh failure instead of claiming full success when a reload throws', async () => {
    // The exact regression: the backend committed the new root, the view
    // reload failed, and the old page still toasted "数据迁移完成".
    mocks.moveRootData.mockResolvedValue({
      moved: ['instances'],
      bytes: 1024,
      cancelled: false,
      root: 'D:/PHL',
    })
    mocks.instanceReload.mockRejectedValue(new Error('磁盘未就绪'))
    await useDataRootStore.getState().startMigration('C:/PHL', 'D:/PHL')
    const toasts = migrationToasts()
    expect(toasts.some((t) => t.title === '数据迁移完成' && t.kind === 'success')).toBe(false)
    const honest = toasts.find((t) => String(t.title).includes('未能刷新'))
    expect(honest).toBeDefined()
    expect(honest.kind).toBe('warn')
    expect(honest.message).toContain('磁盘未就绪')
  })

  it('keeps the root untouched and offers resume when the move reports cancelled', async () => {
    mocks.moveRootData.mockResolvedValue({ moved: [], bytes: 512, cancelled: true })
    await useDataRootStore.getState().startMigration('C:/PHL', 'D:/PHL')
    expect(mocks.setRoot).not.toHaveBeenCalled()
    const toasts = migrationToasts()
    expect(toasts.some((t) => t.title === '迁移已取消' && t.kind === 'info')).toBe(true)
    expect(toasts.some((t) => t.kind === 'success')).toBe(false)
  })
})

describe('applyRootFlow', () => {
  it('refuses an invalid path before any confirmation', async () => {
    await useDataRootStore.getState().applyRootFlow('D:/PHL/../etc')
    expect(mocks.confirm).not.toHaveBeenCalled()
    const toasts = migrationToasts()
    expect(toasts.some((t) => t.title === '数据目录无效')).toBe(true)
  })

  it('blocks the switch while an instance is running', async () => {
    mocks.instanceStates.a = { status: 'running' }
    await useDataRootStore.getState().applyRootFlow('D:/PHL')
    expect(mocks.confirm).not.toHaveBeenCalled()
    expect(migrationToasts().some((t) => t.title === '暂时无法更改数据目录')).toBe(true)
  })

  it('switches without migration when the user declines the move offer', async () => {
    mocks.confirm.mockResolvedValueOnce(true) // 更改数据目录
    mocks.confirm.mockResolvedValueOnce(false) // 立即迁移? → 以后再说
    mocks.rootDataSummary.mockResolvedValue({
      hasData: true,
      instances: { exists: true, entries: 2 },
      versions: { exists: true, entries: 0 },
      runtimes: { exists: true, entries: 0 },
      config: { exists: true, entries: 0 },
      cache: { exists: true, entries: 0 },
    })
    mocks.setRootVerified.mockResolvedValue('D:/PHL')
    await useDataRootStore.getState().applyRootFlow('D:/PHL')
    expect(mocks.moveRootData).not.toHaveBeenCalled()
    expect(mocks.setRootVerified).toHaveBeenCalledWith('D:/PHL')
    expect(migrationToasts().some((t) => t.title === '数据目录已更新')).toBe(true)
  })
})

describe('rootProblem', () => {
  it('rejects empty, relative, and dot-segment paths; accepts absolute roots', () => {
    expect(rootProblem('')).toContain('请输入')
    expect(rootProblem('PHL/data')).toContain('绝对路径')
    expect(rootProblem('D:/PHL/../elsewhere')).toContain('相对段')
    expect(rootProblem('D:/PHL')).toBeNull()
    expect(rootProblem('\\\\server\\share')).toBeNull()
  })
})
