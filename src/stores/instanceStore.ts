import { create } from 'zustand'
import { repository, Cancelled, LaunchError, type CreateProgress } from '@/services'
import type { Instance, InstanceDraft, InstanceRuntimeState } from '@/types'
import { useCatalogStore } from './catalogStore'
import { useUIStore } from './uiStore'

interface InstanceState {
  instances: Instance[]
  /** Live status keyed by instance id; kept apart from the persisted model. */
  states: Record<string, InstanceRuntimeState>
  loaded: boolean
  /** The instance the launch dock currently targets. */
  focusId: string | null
  setFocus: (id: string) => void

  createProgress: CreateProgress | null

  load: () => Promise<void>

  launch: (id: string) => Promise<void>
  cancelLaunch: (id: string) => void
  stop: (id: string) => Promise<void>
  toggle: (id: string) => void

  createInstance: (draft: InstanceDraft) => Promise<Instance | null>
  cancelCreate: () => void
  cloneInstance: (id: string, name: string) => Promise<Instance | null>
  updateInstance: (id: string, patch: Partial<Instance>) => void
  deleteInstance: (id: string) => Promise<void>
  toggleFavorite: (id: string) => void
  dismissError: (id: string) => void
  /** Rewrites every instance path when the user moves PHL's data root. */
  relocate: (fromRoot: string, toRoot: string) => void

  byId: (id: string) => Instance | undefined
  stateOf: (id: string) => InstanceRuntimeState
  portsInUse: () => Map<number, string>
  suggestPort: () => number
  runningCount: () => number
}

const STOPPED: InstanceRuntimeState = { status: 'stopped' }

const launchControllers = new Map<string, AbortController>()
let createController: AbortController | null = null
let loadStarted = false

export const useInstanceStore = create<InstanceState>()((set, get) => ({
  instances: [],
  states: {},
  loaded: false,
  focusId: null,
  setFocus: (focusId) => set({ focusId }),
  createProgress: null,

  async load() {
    // StrictMode mounts effects twice in development; loading once keeps the
    // seeded runtime states from being reset under the user.
    if (get().loaded || loadStarted) return
    loadStarted = true
    const instances = await repository.listInstances()
    // Seed one already-running instance so the prototype opens on a live
    // system rather than a cold one.
    const states: Record<string, InstanceRuntimeState> = {}
    for (const i of instances) states[i.id] = { status: 'stopped' }
    const prod = instances.find((i) => i.id === 'production')
    if (prod) {
      states[prod.id] = {
        status: 'running',
        pid: 21744,
        startedAt: Date.now() - 3 * 3600_000 - 812_000,
        progress: 1,
      }
    }
    set({ instances, states, loaded: true })
  },

  /* ---------------- lifecycle ---------------- */

  async launch(id) {
    const instance = get().byId(id)
    if (!instance) return
    const current = get().stateOf(id).status
    if (current === 'starting' || current === 'running' || current === 'stopping') return

    const catalog = useCatalogStore.getState()
    const ui = useUIStore.getState()
    const controller = new AbortController()
    launchControllers.set(id, controller)

    const patch = (s: InstanceRuntimeState) => set({ states: { ...get().states, [id]: s } })
    set({ focusId: id })
    patch({ status: 'starting', progress: 0, phase: 'resolve-version' })

    try {
      const outcome = await repository.launch(
        instance,
        {
          version: catalog.versionById(instance.versionId),
          runtime: catalog.runtimeById(instance.runtimeId),
          portsInUse: get().portsInUse(),
        },
        (p) => patch({ status: 'starting', phase: p.phase, progress: p.progress }),
        controller.signal,
      )

      patch({ status: 'running', progress: 1, pid: outcome.pid, startedAt: Date.now() })
      set({
        instances: get().instances.map((i) =>
          i.id === id ? { ...i, lastRunAt: new Date().toISOString(), port: outcome.port } : i,
        ),
      })
      ui.toast({
        kind: 'success',
        title: `${instance.name} 已就绪`,
        message: `WebUI 运行在 localhost:${outcome.port}`,
        action: { label: '打开', run: () => void 0 },
      })
    } catch (err) {
      if (err instanceof Cancelled) {
        patch(STOPPED)
        ui.toast({ kind: 'info', title: `已取消启动 ${instance.name}` })
      } else if (err instanceof LaunchError) {
        patch({
          status: 'error',
          error: { title: err.title, detail: err.detail, hint: err.hint },
        })
        ui.toast({
          kind: 'error',
          title: `${instance.name} 启动失败`,
          message: err.title,
          action: { label: '查看', run: () => ui.navigate({ name: 'instance', id }) },
        })
      } else {
        patch({ status: 'error', error: { title: '未知错误', detail: String(err) } })
      }
    } finally {
      launchControllers.delete(id)
    }
  },

  cancelLaunch(id) {
    launchControllers.get(id)?.abort()
  },

  async stop(id) {
    const instance = get().byId(id)
    if (!instance) return
    const state = get().stateOf(id)
    if (state.status !== 'running') return

    const ranFor = state.startedAt ? Math.floor((Date.now() - state.startedAt) / 1000) : 0
    set({ states: { ...get().states, [id]: { status: 'stopping', pid: state.pid } } })

    const controller = new AbortController()
    try {
      await repository.stop(instance, controller.signal)
    } catch {
      /* stopping is not cancellable in the prototype */
    }

    set({
      states: { ...get().states, [id]: STOPPED },
      instances: get().instances.map((i) =>
        i.id === id
          ? { ...i, totalRuntime: i.totalRuntime + ranFor, lastRunAt: new Date().toISOString() }
          : i,
      ),
    })
    useUIStore.getState().toast({ kind: 'info', title: `${instance.name} 已停止` })
  },

  toggle(id) {
    const status = get().stateOf(id).status
    if (status === 'running') void get().stop(id)
    else if (status === 'starting') get().cancelLaunch(id)
    else void get().launch(id)
  },

  /* ---------------- CRUD ---------------- */

  async createInstance(draft) {
    const catalog = useCatalogStore.getState()
    const template = catalog.templates.find((t) => t.id === draft.templateId)
    createController = new AbortController()
    set({ createProgress: { step: 'create', progress: 0 } })

    try {
      const instance = await repository.createInstance(
        draft,
        template,
        (p) => set({ createProgress: p }),
        createController.signal,
      )
      set({
        instances: [...get().instances, instance],
        states: { ...get().states, [instance.id]: STOPPED },
        createProgress: null,
      })
      useUIStore.getState().toast({
        kind: 'success',
        title: `实例「${instance.name}」已创建`,
        message: '环境已隔离，可以直接启动。',
      })
      return instance
    } catch (err) {
      set({ createProgress: null })
      if (!(err instanceof Cancelled)) {
        useUIStore.getState().toast({ kind: 'error', title: '创建失败', message: String(err) })
      }
      return null
    } finally {
      createController = null
    }
  },

  cancelCreate() {
    createController?.abort()
  },

  async cloneInstance(id, name) {
    const source = get().byId(id)
    if (!source) return null
    const clone = await repository.cloneInstance(source, name, get().suggestPort())
    set({
      instances: [...get().instances, clone],
      states: { ...get().states, [clone.id]: STOPPED },
    })
    useUIStore.getState().toast({
      kind: 'success',
      title: `已克隆为「${clone.name}」`,
      message: `版本、Runtime 与 ${source.plugins.length} 个插件均已复制，端口 :${clone.port}`,
      action: {
        label: '打开',
        run: () => useUIStore.getState().navigate({ name: 'instance', id: clone.id }),
      },
    })
    return clone
  },

  updateInstance(id, patch) {
    set({ instances: get().instances.map((i) => (i.id === id ? { ...i, ...patch } : i)) })
  },

  async deleteInstance(id) {
    const instance = get().byId(id)
    if (!instance) return
    launchControllers.get(id)?.abort()
    await repository.deleteInstance(id)
    const states = { ...get().states }
    delete states[id]
    set({ instances: get().instances.filter((i) => i.id !== id), states })
    if (get().focusId === id) set({ focusId: null })
    useUIStore.getState().toast({
      kind: 'info',
      title: `已删除「${instance.name}」`,
      message: '该实例的 DSH_HOME、插件与 workspace 已一并清理。',
    })
  },

  toggleFavorite(id) {
    set({
      instances: get().instances.map((i) => (i.id === id ? { ...i, favorite: !i.favorite } : i)),
    })
  },

  dismissError(id) {
    if (get().stateOf(id).status !== 'error') return
    set({ states: { ...get().states, [id]: STOPPED } })
  },

  relocate(fromRoot, toRoot) {
    if (!fromRoot || fromRoot === toRoot) return
    const swap = (path: string) =>
      path.startsWith(fromRoot) ? toRoot + path.slice(fromRoot.length) : path
    set({
      instances: get().instances.map((i) => ({
        ...i,
        dshHome: swap(i.dshHome),
        workspace: swap(i.workspace),
      })),
    })
  },

  /* ---------------- selectors ---------------- */

  byId: (id) => get().instances.find((i) => i.id === id),
  stateOf: (id) => get().states[id] ?? STOPPED,

  portsInUse: () => {
    const map = new Map<number, string>()
    for (const i of get().instances) {
      const s = get().stateOf(i.id)
      if (s.status === 'running' || s.status === 'starting' || s.status === 'stopping') {
        map.set(i.port, i.name)
      }
    }
    return map
  },

  suggestPort: () => {
    const taken = new Set(get().instances.map((i) => i.port))
    let port = 3080
    while (taken.has(port)) port += 1
    return port
  },

  runningCount: () =>
    get().instances.filter((i) => get().stateOf(i.id).status === 'running').length,
}))
