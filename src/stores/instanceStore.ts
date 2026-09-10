import { parseThrownError } from '@/lib/errorCodes'
import { create } from 'zustand'
import { repository, Cancelled, KeptRunningError, LaunchError, type CopyProgress, type CreateProgress } from '@/services'
import { adoptProcesses, isDesktop, onInstanceExited, openDshWebUi } from '@/lib/desktop'
import { createOptimisticQueue } from '@/lib/optimisticQueue'
import type { Instance, InstanceDraft, InstanceRuntimeState, Snapshot } from '@/types'
import { useCatalogStore } from './catalogStore'
import { useSettingsStore } from './settingsStore'
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
  /**
   * Live snapshot copies keyed by instance id. Snapshot create AND restore
   * are local tree copies that can run for a while on a plugin-heavy
   * instance; both report progress here.
   */
  snapshotTransfers: Record<string, CopyProgress>
  /**
   * Which copy the progress above belongs to, so the UI can say
   * "正在复制 / 正在还原" truthfully. The transfer map keys one slot per
   * instance: create and restore are mutually exclusive (`snapshotControllers`
   * guards both).
   */
  snapshotOps: Record<string, 'create' | 'restore'>
  /**
   * Snapshot deletes walking the tree, keyed `${instanceId}:${snapshotId}`.
   * Same lesson as `deleting` below: the row must show work-in-progress and
   * ignore repeat clicks instead of parking the user on a busy-lock error.
   */
  deletingSnapshots: Record<string, true>
  /**
   * Instances whose teardown the backend is walking right now (a GB-deep
   * `node_modules` delete takes real seconds). The row shows "删除中…" and
   * ignores repeat clicks — before this the interface sat frozen through
   * the delete and users clicked into a busy-lock error.
   */
  deleting: Record<string, true>
  /**
   * Instances whose tree a clone is copying right now (the copy can run
   * minutes on a plugin-heavy instance). Same lesson as `deleting`: the row
   * must show the click landed, and a second clone of the same source is
   * absorbed instead of hitting the backend busy lock.
   */
  cloning: Record<string, true>

  load: () => Promise<void>
  /** Forces a re-read — used when the data root changes under the app. */
  reload: () => Promise<void>
  /**
   * Takes over DSH children that survived a PHL restart (their instances
   * were launched by a previous session), and reports ones PHL refused to
   * adopt. Called once at boot after `load()` resolved.
   */
  adoptPreviousSession: () => Promise<void>

  launch: (id: string) => Promise<void>
  cancelLaunch: (id: string) => void
  stop: (id: string) => Promise<boolean>
  toggle: (id: string) => void
  /** Opens (or focuses) the instance's WebUI, reporting a double failure. */
  openWebUi: (id: string) => Promise<void>

  createSnapshot: (id: string) => Promise<Snapshot | null>
  restoreSnapshot: (id: string, snapshotId: string) => Promise<void>
  /** Abort the copy leg of the instance's in-flight create/restore. */
  cancelSnapshot: (id: string) => void
  deleteSnapshot: (id: string, snapshotId: string) => Promise<void>

  createInstance: (draft: InstanceDraft) => Promise<Instance | null>
  cancelCreate: () => void
  cloneInstance: (id: string, name: string) => Promise<Instance | null>
  /** Registers an instance created outside the create flow (bundle import). */
  admitInstance: (instance: Instance) => void
  updateInstance: (id: string, patch: Partial<Instance>) => void
  deleteInstance: (id: string) => Promise<void>
  toggleFavorite: (id: string) => void
  dismissError: (id: string) => void
  /**
   * Fills in `diskUsage` by measuring the real trees. Memory-only: the size
   * is observed, not configuration, so it must not be written back into
   * `instance.json`.
   */
  measureDiskUsage: () => Promise<void>

  byId: (id: string) => Instance | undefined
  stateOf: (id: string) => InstanceRuntimeState
  portsInUse: () => Map<number, string>
  suggestPort: () => number
  runningCount: () => number
  hasPendingWrites: () => boolean
  flushWrites: () => Promise<void>
}

const STOPPED: InstanceRuntimeState = { status: 'stopped' }

const launchControllers = new Map<string, AbortController>()
const snapshotControllers = new Map<string, AbortController>()
const exitedDuringStop = new Set<string>()
const exitedDuringLaunch = new Map<string, { pid: number; code: number | null }>()
let createController: AbortController | null = null
let loadStarted = false
let exitListenerBound = false
const instanceWriters = new Map<string, ReturnType<typeof createOptimisticQueue<Instance>>>()
const derivedFields = new Set<keyof Instance>(['plugins', 'snapshots', 'diskUsage', 'dshHome', 'workspace'])

/**
 * 插件安装正在往实例的 node_modules 里写文件；此刻拷贝它（快照）或换掉它
 * （回滚）都会得到撕裂的结果。Rust 看不到这些传输，只能在这里拦。
 */
function pluginInstallActive(id: string): boolean {
  const transfers = useCatalogStore.getState().pluginTransfers
  return Object.keys(transfers).some((key) => key.startsWith(`p:${id}:`))
}

/**
 * One event for every way a process can die. The Rust watcher removes its map
 * entry and emits; here the instance state folds back to stopped and the run
 * time is banked. A manual stop already resolved the state before the event
 * arrives, so this is a no-op on that path.
 */
function bindExitListener(
  set: (partial: Partial<InstanceState>) => void,
  get: () => InstanceState,
) {
  if (exitListenerBound || !isDesktop) return
  exitListenerBound = true
  void onInstanceExited(({ instanceId, pid, code }) => {
    const state = get().stateOf(instanceId)
    if (state.status === 'starting') {
      exitedDuringLaunch.set(instanceId, { pid, code })
      return
    }
    if (state.pid !== undefined && state.pid !== pid) return
    if (state.status !== 'running' && state.status !== 'stopping') return
    // A manual stop owns the bookkeeping: it captured `ranFor` before awaiting
    // and banks it once the call returns. Banking here too counted the session
    // twice and fired a second toast whenever the process happened to die
    // inside that await — which is the normal case, not an edge one.
    if (state.status === 'stopping') {
      exitedDuringStop.add(instanceId)
      return
    }
    const crashed = code !== 0
    const ranFor = state.startedAt ? Math.floor((Date.now() - state.startedAt) / 1000) : 0
    set({
      states: {
        ...get().states,
        [instanceId]: crashed
          ? { status: 'stopped', lastExit: { code: code ?? null, at: Date.now(), ranFor } }
          : STOPPED,
      },
    })
    const instance = get().byId(instanceId)
    if (instance && ranFor > 0) {
      get().updateInstance(instanceId, {
        totalRuntime: instance.totalRuntime + ranFor,
        lastRunAt: new Date().toISOString(),
      })
    }
    if (crashed) {
      useUIStore.getState().toast({
        kind: 'error',
        title: `${instance?.name ?? instanceId} 进程异常退出`,
        message: `退出码 ${code ?? '未知'}。日志在实例目录的 logs/ 下。`,
      })
    } else {
      useUIStore.getState().toast({ kind: 'info', title: `${instance?.name ?? instanceId} 已退出` })
    }
  })
}

export const useInstanceStore = create<InstanceState>()((set, get) => ({
  instances: [],
  states: {},
  loaded: false,
  focusId: null,
  setFocus: (focusId) => set({ focusId }),
  createProgress: null,
  snapshotTransfers: {},
  snapshotOps: {},
  deletingSnapshots: {},
  deleting: {},
  cloning: {},

  async load() {
    // StrictMode mounts effects twice in development; loading once keeps the
    // seeded runtime states from being reset under the user.
    if (get().loaded || loadStarted) return
    loadStarted = true
    bindExitListener(set, get)
    try {
      const instances = await repository.listInstances()
      const states: Record<string, InstanceRuntimeState> = {}
      for (const i of instances) states[i.id] = { status: 'stopped' }
      set({ instances, states, loaded: true })
    } catch (err) {
      // Leaving the latch set would wedge instance loading for the rest of
      // the session with no way back — a retry has to remain possible.
      loadStarted = false
      throw err
    }
  },

  /**
   * Re-reads the instance list from scratch.
   *
   * `load()` latches so StrictMode's double mount cannot reset runtime state,
   * which also made it a one-shot for the whole session — after the data root
   * changed, the list kept describing the old root while every write went to
   * the new one. Changing the root has to be able to force a re-read.
   */
  async reload() {
    loadStarted = false
    set({ loaded: false })
    await get().load()
  },

  async adoptPreviousSession() {
    if (!isDesktop) return
    let report
    try {
      report = await adoptProcesses()
    } catch (err) {
      // Adoption is a boot nicety: a failure must not break the app the
      // user just restarted — the instances simply start as stopped.
      console.error('[phl] process adoption failed:', err)
      return
    }
    if (!report.adopted.length && !report.dropped.length) return
    let changed = false
    const states = { ...get().states }
    for (const a of report.adopted) {
      if (!get().byId(a.instanceId)) continue
      states[a.instanceId] = {
        status: 'running',
        progress: 1,
        pid: a.pid,
        startedAt: Date.now(),
        webUrl: a.webUrl ?? undefined,
      }
      changed = true
    }
    if (changed) set({ states })
    const ui = useUIStore.getState()
    if (report.adopted.length > 0) {
      ui.toast({
        kind: 'info',
        title: `已接管上次会话仍在本机运行的 ${report.adopted.length} 个 DSH 进程`,
        message: '停止与「打开 WebUI」对这些实例照常可用。',
        duration: 6000,
      })
    }
    for (const d of report.dropped) {
      if (!d.keptRunning) continue
      ui.toast({
        kind: 'warn',
        title: '发现无法确认的遗留 DSH 进程',
        message: `${d.reason}（PID ${d.pid}）。PHL 不会接管或终止它。`,
        duration: 8000,
      })
    }
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
    exitedDuringLaunch.delete(id)

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

      const exited = exitedDuringLaunch.get(id)
      if (exited?.pid === outcome.pid) {
        throw new LaunchError('进程在启动完成前退出', `退出码 ${exited.code ?? '未知'}，请查看实例 logs/ 下的日志。`)
      }

      patch({
        status: 'running',
        progress: 1,
        pid: outcome.pid,
        startedAt: Date.now(),
        webUrl: outcome.webUrl,
      })
      // The allocated port and the run timestamp are real configuration now —
      // memory-only updates used to lose the port on restart.
      get().updateInstance(id, { lastRunAt: new Date().toISOString(), port: outcome.port })
      // A launcher's delivered promise: getting to the running app must not
      // cost an extra click. Desktop opens (or focuses) the instance's
      // embedded WebUI window; the browser mock build keeps the manual
      // action instead of spawning unasked tabs on every mock launch.
      if (isDesktop) void get().openWebUi(id)
      ui.toast({
        kind: 'success',
        title: `${instance.name} 已就绪`,
        message: isDesktop ? 'WebUI 已在独立窗口打开' : `WebUI 运行在 localhost:${outcome.port}`,
        action: { label: '打开', run: () => void get().openWebUi(id) },
      })
    } catch (err) {
      if (err instanceof KeptRunningError) {
        const exited = exitedDuringLaunch.get(id)
        if (exited?.pid === err.pid) {
          patch(exited.code === 0 ? STOPPED : {
            status: 'stopped',
            lastExit: { code: exited.code, at: Date.now(), ranFor: 0 },
          })
          ui.toast({ kind: 'info', title: `${instance.name} 已确认退出` })
          return
        }
        // R3: the DSH process is alive but termination could not be
        // confirmed, so the backend kept it registered on `err.port`.
        // Presenting it as running is what preserves a working stop entry —
        // a generic error state would strand a live process with no button.
        patch({ status: 'running', progress: 1, pid: err.pid, startedAt: Date.now() })
        get().updateInstance(id, { port: err.port, lastRunAt: new Date().toISOString() })
        ui.toast({
          kind: 'warn',
          title: `${instance.name} 未能确认退出，已保留为运行中`,
          message: `${err.detail} 端口 ${err.port} 仍被占用；可直接重试停止。`,
          duration: 9000,
          action: { label: '停止', run: () => void get().stop(id) },
        })
      } else if (err instanceof Cancelled) {
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
          action: { label: '查看', run: () => ui.push({ name: 'instance', id }) },
        })
      } else {
        patch({ status: 'error', error: { title: '未知错误', detail: parseThrownError(err).message } })
      }
    } finally {
      launchControllers.delete(id)
      exitedDuringLaunch.delete(id)
    }
  },

  cancelLaunch(id) {
    launchControllers.get(id)?.abort()
  },

  async stop(id) {
    const instance = get().byId(id)
    if (!instance) return true
    const state = get().stateOf(id)
    if (state.status !== 'running') return state.status === 'stopped' || state.status === 'error'

    const ranFor = state.startedAt ? Math.floor((Date.now() - state.startedAt) / 1000) : 0
    set({ states: { ...get().states, [id]: { ...state, status: 'stopping' } } })

    const controller = new AbortController()
    exitedDuringStop.delete(id)
    try {
      await repository.stop(instance, controller.signal)
    } catch (err) {
      // The exit event can confirm death while the stop command is pending.
      // Otherwise keep the process tracked and allow a retry.
      if (!exitedDuringStop.has(id)) {
        set({ states: { ...get().states, [id]: state } })
        useUIStore.getState().toast({
          kind: 'error', title: `${instance.name} 停止失败`, message: parseThrownError(err).message,
        })
        return false
      }
    } finally {
      exitedDuringStop.delete(id)
    }

    set({ states: { ...get().states, [id]: STOPPED } })
    // Re-read after the await: `instance` is a pre-await snapshot, and a
    // plugin install or an edit that landed while the process was shutting
    // down would be added back on top of a stale `totalRuntime`.
    const current = get().byId(id)
    if (current) {
      get().updateInstance(id, {
        totalRuntime: current.totalRuntime + ranFor,
        lastRunAt: new Date().toISOString(),
      })
    }
    useUIStore.getState().toast({ kind: 'info', title: `${instance.name} 已停止` })
    return true
  },

  toggle(id) {
    const status = get().stateOf(id).status
    if (status === 'running') void get().stop(id)
    else if (status === 'starting') get().cancelLaunch(id)
    else void get().launch(id)
  },

  /* ---------------- snapshots ---------------- */

  async createSnapshot(id) {
    const instance = get().byId(id)
    if (!instance || snapshotControllers.has(id)) return null
    const status = get().stateOf(id).status
    if (status === 'running' || status === 'starting' || status === 'stopping') {
      useUIStore.getState().toast({ kind: 'info', title: '先停止实例，再创建快照' })
      return null
    }
    if (pluginInstallActive(id)) {
      useUIStore
        .getState()
        .toast({ kind: 'info', title: '有插件正在安装', message: '等插件安装完成后再创建快照。' })
      return null
    }
    const controller = new AbortController()
    snapshotControllers.set(id, controller)
    const patchTransfer = (p: CopyProgress) =>
      set({ snapshotTransfers: { ...get().snapshotTransfers, [id]: p } })
    set({ snapshotOps: { ...get().snapshotOps, [id]: 'create' } })
    patchTransfer({ progress: 0, bytesDone: 0, bytesTotal: 0 })
    try {
      const snap = await repository.createSnapshot(instance, patchTransfer, controller.signal)
      // snapshots 是磁盘派生的列表；saveInstance 的 manifest 不含它，落内存即可。
      // Read the list back after the await rather than closing over the
      // pre-await copy: a snapshot deleted while this one was being written
      // would otherwise be resurrected by the stale array.
      const current = get().byId(id)
      get().updateInstance(id, { snapshots: [snap, ...(current?.snapshots ?? [])] })
      useUIStore.getState().toast({ kind: 'success', title: '已创建快照', message: snap.label })
      return snap
    } catch (err) {
      if (err instanceof Cancelled) {
        useUIStore.getState().toast({ kind: 'info', title: '已取消创建快照' })
      } else {
        useUIStore
          .getState()
          .toast({
            kind: 'error',
            title: '创建快照失败',
            message: parseThrownError(err).message,
          })
      }
      return null
    } finally {
      snapshotControllers.delete(id)
      const transfers = { ...get().snapshotTransfers }
      delete transfers[id]
      const ops = { ...get().snapshotOps }
      delete ops[id]
      set({ snapshotTransfers: transfers, snapshotOps: ops })
    }
  },

  cancelSnapshot(id) {
    snapshotControllers.get(id)?.abort()
  },

  async restoreSnapshot(id, snapshotId) {
    const instance = get().byId(id)
    if (!instance) return
    // One snapshot copy per instance at a time — the same guard create uses;
    // a double click used to fire a second guarded task straight into a
    // busy-lock error while the first was still copying.
    if (snapshotControllers.has(id)) return
    const status = get().stateOf(id).status
    if (status === 'running' || status === 'starting' || status === 'stopping') {
      useUIStore.getState().toast({ kind: 'info', title: '先停止实例，再还原快照' })
      return
    }
    if (pluginInstallActive(id)) {
      useUIStore
        .getState()
        .toast({ kind: 'info', title: '有插件正在安装', message: '等插件安装完成后再回滚。' })
      return
    }
    const controller = new AbortController()
    snapshotControllers.set(id, controller)
    const patchTransfer = (p: CopyProgress) =>
      set({ snapshotTransfers: { ...get().snapshotTransfers, [id]: p } })
    set({ snapshotOps: { ...get().snapshotOps, [id]: 'restore' } })
    patchTransfer({ progress: 0, bytesDone: 0, bytesTotal: 0 })
    try {
      const fresh = await repository.restoreSnapshot(instance, snapshotId, patchTransfer, controller.signal)
      // 还原换掉了整个 dsh-home：插件列表由磁盘反推，必须以还原后的为准。
      get().updateInstance(id, { plugins: fresh.plugins })
      useUIStore
        .getState()
        .toast({ kind: 'success', title: '已还原快照', message: '插件与配置已回到快照时的状态。' })
    } catch (err) {
      if (err instanceof Cancelled) {
        useUIStore.getState().toast({ kind: 'info', title: '已取消还原快照' })
      } else {
        useUIStore
          .getState()
          .toast({
            kind: 'error',
            title: '还原快照失败',
            message: parseThrownError(err).message,
          })
      }
    } finally {
      snapshotControllers.delete(id)
      const transfers = { ...get().snapshotTransfers }
      delete transfers[id]
      const ops = { ...get().snapshotOps }
      delete ops[id]
      set({ snapshotTransfers: transfers, snapshotOps: ops })
    }
  },

  async deleteSnapshot(id, snapshotId) {
    const instance = get().byId(id)
    if (!instance) return
    const key = `${id}:${snapshotId}`
    if (get().deletingSnapshots[key]) return
    // Deleting walks the copied tree; show the row busy and absorb repeat
    // clicks the same way instance deletion does.
    set({ deletingSnapshots: { ...get().deletingSnapshots, [key]: true } })
    try {
      await repository.deleteSnapshot(instance, snapshotId)
      get().updateInstance(id, {
        snapshots: (get().byId(id)?.snapshots ?? []).filter((s) => s.id !== snapshotId),
      })
    } catch (err) {
      useUIStore
        .getState()
        .toast({
          kind: 'error',
          title: '删除快照失败',
          message: parseThrownError(err).message,
        })
    } finally {
      const deleting = { ...get().deletingSnapshots }
      delete deleting[key]
      set({ deletingSnapshots: deleting })
    }
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
        useUIStore.getState().toast({ kind: 'error', title: '创建失败', message: parseThrownError(err).message })
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
    if (!source || get().cloning[id]) return null
    // Pending FIRST, exactly like `deleting`: the clone copies the whole
    // dsh-home (minutes on a plugin-heavy instance), and before this the row
    // sat unchanged until the copy resolved — repeat clicks just stacked
    // backend busy-lock toasts.
    set({ cloning: { ...get().cloning, [id]: true } })
    // A failed clone used to surface nowhere: the throw escaped as an
    // unhandled rejection and the list kept showing the old count. Report it
    // like every other destructive-write failure does.
    let clone: Instance | null = null
    try {
      clone = await repository.cloneInstance(source, name, get().suggestPort())
    } catch (err) {
      const e = parseThrownError(err)
      useUIStore.getState().toast({
        kind: 'error',
        title: `克隆「${source.name}」失败`,
        message: e.message,
      })
    } finally {
      set((s) => {
        const cloning = { ...s.cloning }
        delete cloning[id]
        return { cloning }
      })
    }
    if (!clone) return null
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
        run: () => useUIStore.getState().push({ name: 'instance', id: clone.id }),
      },
    })
    return clone
  },

  admitInstance(instance) {
    if (get().instances.some((i) => i.id === instance.id)) return
    set({
      instances: [...get().instances, instance],
      states: { ...get().states, [instance.id]: STOPPED },
    })
  },

  updateInstance(id, patch) {
    const before = get().byId(id)
    if (!before) return
    const publish = (changes: Partial<Instance>) => set({
      instances: get().instances.map((i) => i.id === id ? { ...i, ...changes } : i),
    })
    const persisted: Partial<Instance> = {}
    const derived: Partial<Instance> = {}
    for (const key of Object.keys(patch) as (keyof Instance)[]) {
      if (key === 'id') continue
      Object.assign(derivedFields.has(key) ? derived : persisted, { [key]: patch[key] })
    }
    publish(derived)
    if (!Object.keys(persisted).length) return
    let writer = instanceWriters.get(id)
    if (!writer) {
      writer = createOptimisticQueue({
        read: () => get().byId(id)!,
        publish,
        write: (value) => repository.saveInstance(value),
        onError: (err) => useUIStore.getState().toast({
          kind: 'error', title: `保存「${before.name}」的修改失败`, message: parseThrownError(err).message,
        }),
      })
      instanceWriters.set(id, writer)
    }
    writer.enqueue(persisted)
  },

  async deleteInstance(id) {
    const instance = get().byId(id)
    if (!instance) return
    if (get().deleting[id]) return // a teardown for this row is already walking
    if (instanceWriters.get(id)?.busy) {
      useUIStore.getState().toast({ kind: 'info', title: '请等待实例配置保存完成后再删除' })
      return
    }
    launchControllers.get(id)?.abort()
    // A snapshot copy in flight is reading the very tree that is about to
    // vanish — abort it and drop its progress row.
    snapshotControllers.get(id)?.abort()
    snapshotControllers.delete(id)
    const transfers = { ...get().snapshotTransfers }
    delete transfers[id]
    if (transfers[id] !== undefined || get().snapshotTransfers[id] !== undefined) {
      set({ snapshotTransfers: transfers })
    }
    // Pending FIRST: the tree walk takes seconds, and the user must see the
    // click land (row badge + task center "删除实例") before it does.
    set({ deleting: { ...get().deleting, [id]: true } })
    try {
      await repository.deleteInstance(id)
    } catch (err) {
      set((s) => {
        const deleting = { ...s.deleting }
        delete deleting[id]
        return { deleting }
      })
      const parsed = parseThrownError(err)
      useUIStore.getState().toast({
        kind: 'error',
        title: `删除「${instance.name}」失败`,
        message: parsed.message,
        action: { label: '重试', run: () => void get().deleteInstance(id) },
      })
      return
    }
    set((s) => {
      const deleting = { ...s.deleting }
      delete deleting[id]
      const states = { ...s.states }
      delete states[id]
      return {
        deleting,
        instances: s.instances.filter((i) => i.id !== id),
        states,
        focusId: s.focusId === id ? null : s.focusId,
      }
    })
    instanceWriters.delete(id)
    useUIStore.getState().toast({
      kind: 'info',
      title: `已删除「${instance.name}」`,
      message: '该实例的 DSH_HOME、插件与 workspace 已一并清理。',
    })
  },

  toggleFavorite(id) {
    // Routed through `updateInstance` so it persists: `favorite` is a real
    // manifest field, and a raw `set()` here meant the star silently
    // disappeared on the next launch.
    const current = get().byId(id)
    if (current) get().updateInstance(id, { favorite: !current.favorite })
  },

  dismissError(id) {
    if (get().stateOf(id).status !== 'error') return
    set({ states: { ...get().states, [id]: STOPPED } })
  },

  async measureDiskUsage() {
    const sizes = await Promise.all(
      get().instances.map(async (i) => {
        try {
          return [i.id, await repository.measureDiskUsage(i)] as const
        } catch {
          return [i.id, i.diskUsage] as const
        }
      }),
    )
    const measured = new Map(sizes)
    set({
      instances: get().instances.map((i) => ({
        ...i,
        diskUsage: measured.get(i.id) ?? i.diskUsage,
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
    // The user's configured start, not a constant: the advanced setting had
    // no consumer at all, so changing it was a silent lie (O-03).
    let port = useSettingsStore.getState().portStart || 3080
    while (taken.has(port)) port += 1
    return port
  },

  runningCount: () =>
    get().instances.filter((i) => get().stateOf(i.id).status === 'running').length,
  hasPendingWrites: () => [...instanceWriters.values()].some((writer) => writer.busy),

  /**
   * "打开 WebUI" for one instance. The bridge reports whether the request
   * reached an embedded window or fell back to the system browser; a double
   * failure becomes a toast here instead of a promise rejected into `void`.
   */
  async openWebUi(id: string) {
    const instance = get().byId(id)
    if (!instance) return
    const state = get().states[id]
    const url = state?.webUrl ?? `http://localhost:${instance.port}`
    const result = await openDshWebUi(id, url, instance.name)
    if (!result.ok) {
      useUIStore.getState().toast({
        kind: 'error',
        title: `无法打开 ${instance.name} 的 WebUI`,
        message: result.reason,
        duration: 8000,
      })
    }
  },
  flushWrites: async () => { await Promise.all([...instanceWriters.values()].map((writer) => writer.flush())) },
}))
