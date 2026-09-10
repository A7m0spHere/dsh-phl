import { create } from 'zustand'
import { repository } from '@/services'
import { parseThrownError } from '@/lib/errorCodes'
import { resolveBoundVersion } from '@/lib/instanceVersion'
import { isVersionBusy } from '@/types/version'
import { isRuntimeBusy, keepRuntimeStateOnRefresh } from '@/types/runtime'
import { createVersionActions } from './catalogVersionActions'
import { createRuntimeActions } from './catalogRuntimeActions'
import { createPluginActions, pluginKey } from './catalogPluginActions'
import { observePendingReleases } from './catalogNotifications'
import { useUIStore } from './uiStore'
import type { CatalogState, TransferControllers } from './catalogTypes'

export { pluginKey }
export type { CatalogState, LatestVersionState, PluginTransferState } from './catalogTypes'

const controllers: TransferControllers = new Map()

/**
 * One public catalog store remains the UI's source of truth. Version, Runtime
 * and plugin workflows are composed from domain action creators so their state
 * transitions, notifications and cancellation policies stay independent.
 */
export const useCatalogStore = create<CatalogState>()((set, get) => ({
  versions: [],
  runtimes: [],
  plugins: [],
  templates: [],
  loaded: false,
  loading: false,
  versionsLoaded: false,
  versionsSyncing: false,
  versionsSyncedAt: null,
  versionsSyncAttemptedAt: 0,
  pluginTransfers: {},
  pluginsOffline: false,
  pluginsOrigin: null,
  latestVersions: {},

  async load() {
    if (get().loading) return
    set({ loading: true, versionsSyncAttemptedAt: Date.now() })
    const failures: Array<[string, unknown]> = []
    const recordFailure = (label: string) => (err: unknown) => {
      console.warn(`[phl] ${label} load failed:`, err)
      failures.push([label, err])
    }
    try {
      await Promise.all([
        repository
          .listVersions()
          .then((versions) => {
            set({ versions, versionsLoaded: true, versionsSyncedAt: Date.now() })
            observePendingReleases(versions, get().installVersion)
          })
          .catch((err) => {
            recordFailure('版本目录')(err)
            set({ versionsLoaded: true })
          }),
        repository
          .listRuntimes()
          .then((runtimes) =>
            set((current) => ({
              runtimes: runtimes.map((runtime) => {
                // Same rule as the version refresh merge: an in-flight or
                // failed row keeps its optimistic state — the disk lags the
                // operation, and a retried `load()` must not reset it.
                const old = current.runtimes.find((candidate) => candidate.id === runtime.id)
                return old && keepRuntimeStateOnRefresh(old.state.kind)
                  ? { ...runtime, state: old.state }
                  : runtime
              }),
            })),
          )
          .catch(recordFailure('Runtime 列表')),
        repository
          .listPlugins()
          .then((catalog) =>
            set({
              plugins: catalog.plugins,
              pluginsOffline: !!catalog.offline,
              pluginsError: catalog.error,
              pluginsOrigin: catalog.origin ?? null,
            }),
          )
          .catch(recordFailure('插件市场')),
        repository.listTemplates().then((templates) => set({ templates })).catch(recordFailure('实例模板')),
      ])
      set({ loaded: true })
      for (const [label, err] of failures) {
        useUIStore.getState().toast({
          kind: 'error',
          title: `${label}加载失败`,
          message: parseThrownError(err).message || '无法连接发布源，请检查网络。',
          action: { label: '重试', run: () => void get().load() },
        })
      }
    } finally {
      set({ loading: false })
    }
  },

  ...createVersionActions(set, get, controllers),
  ...createRuntimeActions(set, get, controllers),
  ...createPluginActions(set, get, controllers),

  // Legacy-safe: an instance adopted before the binding fix stores the bare
  // detected version; resolve it against the canonical id so launch context
  // and version lookups agree with the detail page (see lib/instanceVersion).
  versionById: (id) => resolveBoundVersion(get().versions, id) ?? undefined,
  runtimeById: (id) => get().runtimes.find((runtime) => runtime.id === id),
  pluginById: (id) => get().plugins.find((plugin) => plugin.id === id),
  activeTransfers: () =>
    get().versions.filter(isVersionBusy).length +
    get().runtimes.filter(isRuntimeBusy).length +
    Object.keys(get().pluginTransfers).length,
}))
