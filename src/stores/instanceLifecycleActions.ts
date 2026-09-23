import { repository, Cancelled, KeptRunningError, LaunchError } from '@/services'
import { isDesktop, openDshWebUi, onInstanceExited } from '@/lib/desktop'
import { parseThrownError } from '@/lib/errorCodes'
import type { InstanceRuntimeState } from '@/types'
import { MIN_WEB_PORT } from '@/lib/ports'
import { catalogState } from './storeRefs'
import { useUIStore } from './uiStore'
import { useSettingsStore } from './settingsStore'
import type { InstanceGet, InstanceSet } from './instanceTypes'

/** The resting state of every instance row. Shared with the CRUD store. */
export const STOPPED: InstanceRuntimeState = { status: 'stopped' }

/**
 * Start/stop/exit-event handling for the instance store.
 *
 * Extracted from `instanceStore` to separate the process-lifecycle concerns
 * (launching and killing DSH children, tracking their exit state, and the
 * auto-open flow) from the CRUD and persistence concerns that remain there.
 *
 * Ownership is explicit:
 * - `launchControllers`: abort handles for in-flight launches; the store's
 *   `cancelLaunch` reaches into this map. Each launch clears its own entry in
 *   `finally`; nothing resets them on reload (a launch in flight across a
 *   root switch is prevented by that flow's busy guard instead).
 * - `exitedDuringStop`: marks an instance whose process died while its
 *   `stop()` call was pending; that is the normal case, and `stop()` checks
 *   the mark before treating a rejected call as a failure.
 * - `exitedDuringLaunch`: holds the (pid, code) of a process that died
 *   before `launch()` resolved successfully. The launch flow checks this
 *   after the repository returns so a "launch succeeded" UI transition is
 *   never shown for a process that was already dead on arrival.
 * - The `onInstanceExited` subscription is bound exactly once per session
 *   (`exitListenerBound`): zustand's `set`/`get` are stable across reloads,
 *   so the handler keeps addressing the live state after a root change and
 *   re-binding would double-stack the toast/banking path.
 *
 * The exit handler itself is the one event every process can fire: the Rust
 * watcher emits it when a child dies, and it folds the UI state back to
 * stopped (banking runtime when applicable). A manual stop resolves the state
 * before the event arrives, so the handler detects the `stopping` status and
 * marks the id in `exitedDuringStop` instead of double-banking.
 */
export function createLifecycleActions(
  set: InstanceSet,
  get: InstanceGet,
) {
  const launchControllers = new Map<string, AbortController>()
  const exitedDuringStop = new Set<string>()
  const exitedDuringLaunch = new Map<string, { pid: number; code: number | null }>()
  let exitListenerBound = false

  function bindExitListener(): void {
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
      if (state.status === 'stopping') {
        exitedDuringStop.add(instanceId)
        return
      }
      const crashed = code !== 0
      const ranFor = state.startedAt ? Math.floor((Date.now() - state.startedAt) / 1000) : 0
      set((s) => ({
        states: {
          ...s.states,
          [instanceId]: crashed
            ? { status: 'stopped', lastExit: { code: code ?? null, at: Date.now(), ranFor } }
            : STOPPED,
        },
      }))
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

  return {
    /** Called from `load()`; the latch makes repeat calls no-ops. */
    ensureExitListener: () => bindExitListener(),

    async launch(id: string) {
      const instance = get().byId(id)
      if (!instance) return
      const current = get().stateOf(id).status
      if (current === 'starting' || current === 'running' || current === 'stopping') return

      const catalog = catalogState()
      const ui = useUIStore.getState()
      const controller = new AbortController()
      launchControllers.set(id, controller)
      exitedDuringLaunch.delete(id)

      const patch = (s: InstanceRuntimeState) => set((cur) => ({ states: { ...cur.states, [id]: s } }))
      set({ focusId: id })
      const launchTarget =
        instance.autoPort && instance.port < MIN_WEB_PORT
          ? { ...instance, port: get().suggestPort() }
          : instance
      const ctx = {
        version: catalog.versionById(instance.versionId),
        runtime: catalog.runtimeById(instance.runtimeId),
        portsInUse: get().portsInUse(),
      }
      patch({ status: 'starting', progress: 0, phase: 'resolve-version' })

      try {
        const outcome = await repository.launch(
          launchTarget,
          ctx,
          (p) => patch({ status: 'starting', phase: p.phase, progress: p.progress }),
          controller.signal,
        )

        const exited = exitedDuringLaunch.get(id)
        if (exited?.pid === outcome.pid) {
          throw new LaunchError(
            '进程在启动完成前退出',
            `退出码 ${exited.code ?? '未知'}，请查看实例 logs/ 下的日志。`,
          )
        }

        patch({
          status: 'running',
          progress: 1,
          pid: outcome.pid,
          startedAt: Date.now(),
          webUrl: outcome.webUrl,
        })
        get().updateInstance(id, { lastRunAt: new Date().toISOString(), port: outcome.port })
        const autoOpen = isDesktop && useSettingsStore.getState().autoOpenWebUi
        if (autoOpen) void get().openWebUi(id)
        ui.toast({
          kind: 'success',
          title: `${instance.name} 已就绪`,
          message: autoOpen ? 'WebUI 已在独立窗口打开' : `WebUI 运行在 localhost:${outcome.port}`,
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

    cancelLaunch(id: string) {
      launchControllers.get(id)?.abort()
    },

    async stop(id: string): Promise<boolean> {
      const instance = get().byId(id)
      if (!instance) return true
      const state = get().stateOf(id)
      if (state.status !== 'running') return state.status === 'stopped' || state.status === 'error'

      const ranFor = state.startedAt ? Math.floor((Date.now() - state.startedAt) / 1000) : 0
      set((cur) => ({ states: { ...cur.states, [id]: { ...state, status: 'stopping' } } }))

      const controller = new AbortController()
      exitedDuringStop.delete(id)
      try {
        await repository.stop(instance, controller.signal)
      } catch (err) {
        if (!exitedDuringStop.has(id)) {
          set((cur) => ({ states: { ...cur.states, [id]: state } }))
          useUIStore.getState().toast({
            kind: 'error',
            title: `${instance.name} 停止失败`,
            message: parseThrownError(err).message,
          })
          return false
        }
      } finally {
        exitedDuringStop.delete(id)
      }

      set((cur) => ({ states: { ...cur.states, [id]: STOPPED } }))
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

    toggle(id: string) {
      const status = get().stateOf(id).status
      if (status === 'running') void get().stop(id)
      else if (status === 'starting') get().cancelLaunch(id)
      else void get().launch(id)
    },

    async openWebUi(id: string) {
      const instance = get().byId(id)
      if (!instance) return
      const state = get().states[id]
      const url = state?.webUrl ?? `http://localhost:${instance.port}`
      try {
        const port = Number(new URL(url).port)
        if (port > 0 && port < MIN_WEB_PORT) {
          useUIStore.getState().toast({
            kind: 'warn',
            title: `${instance.name} 的 WebUI 端口 ${port} 无法在浏览器中打开`,
            message: '这是浏览器封锁的保留端口。停止实例后重新启动，会自动换用可用端口。',
            duration: 9000,
            action: { label: '停止', run: () => void get().stop(id) },
          })
          return
        }
      } catch {
        // Unparseable URL: let the open itself surface the failure as before.
      }
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
  }
}
