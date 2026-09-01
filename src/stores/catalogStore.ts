import { create } from 'zustand'
import { repository, Cancelled } from '@/services'
import type { DshVersion, InstanceTemplate, Plugin, Runtime } from '@/types'
import { useUIStore } from './uiStore'

interface CatalogState {
  versions: DshVersion[]
  runtimes: Runtime[]
  plugins: Plugin[]
  templates: InstanceTemplate[]
  loaded: boolean
  loading: boolean

  load: () => Promise<void>

  installVersion: (id: string) => Promise<void>
  cancelVersion: (id: string) => void
  removeVersion: (id: string) => Promise<void>

  installRuntime: (id: string) => Promise<void>
  cancelRuntime: (id: string) => void
  removeRuntime: (id: string) => Promise<void>

  versionById: (id: string) => DshVersion | undefined
  runtimeById: (id: string) => Runtime | undefined
  pluginById: (id: string) => Plugin | undefined
  /** Number of transfers currently in flight — drives the title-bar indicator. */
  activeTransfers: () => number
}

const controllers = new Map<string, AbortController>()

export const useCatalogStore = create<CatalogState>()((set, get) => ({
  versions: [],
  runtimes: [],
  plugins: [],
  templates: [],
  loaded: false,
  loading: false,

  async load() {
    if (get().loading) return
    set({ loading: true })
    const [versions, runtimes, plugins, templates] = await Promise.all([
      repository.listVersions(),
      repository.listRuntimes(),
      repository.listPlugins(),
      repository.listTemplates(),
    ])
    set({ versions, runtimes, plugins, templates, loaded: true, loading: false })
  },

  /* ---------------- versions ---------------- */

  async installVersion(id) {
    const version = get().versions.find((v) => v.id === id)
    if (!version) return
    const controller = new AbortController()
    controllers.set(`v:${id}`, controller)

    const patch = (state: DshVersion['state']) =>
      set({ versions: get().versions.map((v) => (v.id === id ? { ...v, state } : v)) })

    patch({ kind: 'queued' })
    try {
      await repository.installVersion(
        version,
        (p) => {
          if (p.stage === 'downloading') {
            patch({
              kind: 'downloading',
              progress: p.progress,
              bytesDone: p.bytesDone,
              bytesPerSec: p.bytesPerSec,
            })
          } else if (p.stage === 'extracting') {
            patch({ kind: 'extracting', progress: p.progress })
          } else {
            patch({ kind: 'verifying' })
          }
        },
        controller.signal,
      )
      patch({ kind: 'installed', installedAt: new Date().toISOString() })
      useUIStore.getState().toast({
        kind: 'success',
        title: `DSH ${version.name} 安装完成`,
        message: '现在可以在实例中固定使用这个版本。',
      })
    } catch (err) {
      if (err instanceof Cancelled) {
        patch({ kind: 'available' })
        useUIStore.getState().toast({ kind: 'info', title: `已取消下载 ${version.name}` })
      } else {
        patch({ kind: 'failed', reason: '下载中断，未能校验完整性' })
        useUIStore.getState().toast({
          kind: 'error',
          title: `DSH ${version.name} 安装失败`,
          message: '下载中断，未能校验完整性。',
          action: { label: '重试', run: () => get().installVersion(id) },
        })
      }
    } finally {
      controllers.delete(`v:${id}`)
    }
  },

  cancelVersion(id) {
    controllers.get(`v:${id}`)?.abort()
  },

  async removeVersion(id) {
    await repository.removeVersion(id)
    set({
      versions: get().versions.map((v) => (v.id === id ? { ...v, state: { kind: 'available' } } : v)),
    })
  },

  /* ---------------- runtimes ---------------- */

  async installRuntime(id) {
    const runtime = get().runtimes.find((r) => r.id === id)
    if (!runtime) return
    const controller = new AbortController()
    controllers.set(`r:${id}`, controller)

    const patch = (state: Runtime['state']) =>
      set({ runtimes: get().runtimes.map((r) => (r.id === id ? { ...r, state } : r)) })

    try {
      await repository.installRuntime(
        runtime,
        (p) => {
          if (p.stage === 'downloading') {
            patch({
              kind: 'downloading',
              progress: p.progress,
              bytesDone: p.bytesDone,
              bytesPerSec: p.bytesPerSec,
            })
          } else {
            patch({ kind: 'extracting', progress: p.progress })
          }
        },
        controller.signal,
      )
      patch({ kind: 'installed', installedAt: new Date().toISOString() })
      useUIStore.getState().toast({ kind: 'success', title: `${runtime.name} 安装完成` })
    } catch (err) {
      if (err instanceof Cancelled) {
        patch({ kind: 'available' })
      } else {
        patch({ kind: 'failed', reason: '下载失败' })
        useUIStore.getState().toast({ kind: 'error', title: `${runtime.name} 安装失败` })
      }
    } finally {
      controllers.delete(`r:${id}`)
    }
  },

  cancelRuntime(id) {
    controllers.get(`r:${id}`)?.abort()
  },

  async removeRuntime(id) {
    await repository.removeRuntime(id)
    set({
      runtimes: get().runtimes.map((r) => (r.id === id ? { ...r, state: { kind: 'available' } } : r)),
    })
  },

  /* ---------------- lookups ---------------- */

  versionById: (id) => get().versions.find((v) => v.id === id),
  runtimeById: (id) => get().runtimes.find((r) => r.id === id),
  pluginById: (id) => get().plugins.find((p) => p.id === id),
  activeTransfers: () =>
    get().versions.filter((v) => ['downloading', 'extracting', 'verifying', 'queued'].includes(v.state.kind))
      .length +
    get().runtimes.filter((r) => ['downloading', 'extracting'].includes(r.state.kind)).length,
}))
