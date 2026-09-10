import { Cancelled, repository } from '@/services'
import type { InstalledPlugin } from '@/types'
import { parseThrownError } from '@/lib/errorCodes'
import { acquireTransferSlot, releaseTransferSlot } from './transferCoordinator'
import { useInstanceStore } from './instanceStore'
import { useUIStore } from './uiStore'
import type {
  CatalogGet,
  CatalogSet,
  LatestVersionState,
  PluginActions,
  PluginTransferState,
  TransferControllers,
} from './catalogTypes'

export const pluginKey = (instanceId: string, pluginId: string) => `p:${instanceId}:${pluginId}`

const latestGen = new Map<string, number>()

/// In-flight enable/disable per plugin. Two clicks raced two writes to the
/// same `cordis.patch.yml`: whichever response landed last patched the UI,
/// while whichever *request* landed last decided the file — the switch could
/// then show the opposite of what DSH loads.
const pluginToggles = new Set<string>()

function patchInstancePlugins(instanceId: string, update: (plugins: InstalledPlugin[]) => InstalledPlugin[]): void {
  const store = useInstanceStore.getState()
  const current = store.byId(instanceId)
  if (current) store.updateInstance(instanceId, { plugins: update(current.plugins) })
}

/**
 * Whether profile changes would be noticed by the *currently running*
 * process. DSH reads `cordis.patch.yml` and the profile's `node_modules`
 * at launch — there is no hot-reload path. While the instance runs, these
 * changes describe the NEXT launch, and the toast must not claim the live
 * process already changed.
 */
function processLive(instanceId: string): boolean {
  const status = useInstanceStore.getState().stateOf(instanceId).status
  return status === 'running' || status === 'starting'
}

async function startLatestCheck(get: CatalogGet, set: CatalogSet, pluginId: string): Promise<void> {
  const gen = (latestGen.get(pluginId) ?? 0) + 1
  latestGen.set(pluginId, gen)
  set((current) => ({ latestVersions: { ...current.latestVersions, [pluginId]: { status: 'checking' } } }))
  const plugin = get().pluginById(pluginId)
  let state: LatestVersionState
  if (!plugin) {
    state = { status: 'error', message: `插件 ${pluginId} 不在目录中` }
  } else {
    try {
      const latest = await repository.latestPluginVersion(plugin)
      state = latest === null ? { status: 'unresolvable' } : { status: 'resolved', latest }
    } catch (err) {
      state = { status: 'error', message: parseThrownError(err).message }
    }
  }
  if (latestGen.get(pluginId) !== gen) return
  set((current) => ({ latestVersions: { ...current.latestVersions, [pluginId]: state } }))
}

function launchLatestChecks(get: CatalogGet, set: CatalogSet, instanceId: string, force: boolean): Promise<void>[] {
  const instance = useInstanceStore.getState().byId(instanceId)
  if (!instance) return []
  const runs: Promise<void>[] = []
  for (const installed of instance.plugins) {
    if (installed.linked || !get().pluginById(installed.pluginId)) continue
    const current = get().latestVersions[installed.pluginId]
    if (current?.status === 'checking') continue
    if (!force && current !== undefined && current.status !== 'error') continue
    runs.push(startLatestCheck(get, set, installed.pluginId))
  }
  return runs
}

export function createPluginActions(
  set: CatalogSet,
  get: CatalogGet,
  controllers: TransferControllers,
): PluginActions {
  return {
    async installPlugin(instanceId, pluginId) {
      const instance = useInstanceStore.getState().byId(instanceId)
      const plugin = get().pluginById(pluginId)
      if (!instance || !plugin) return
      const key = pluginKey(instanceId, pluginId)
      if (get().pluginTransfers[key]) return

      const controller = new AbortController()
      controllers.set(key, controller)
      const patch = (transfer: PluginTransferState) =>
        set((current) => ({ pluginTransfers: { ...current.pluginTransfers, [key]: transfer } }))

      patch({ stage: 'queued', progress: 0, bytesDone: 0, bytesPerSec: 0 })
      try {
        if (!(await acquireTransferSlot(key, controller.signal))) throw new Cancelled()
        patch({ stage: 'preparing', progress: 0, bytesDone: 0, bytesPerSec: 0 })
        const { version, registryId, trust } = await repository.installPlugin(
          plugin,
          instance,
          (progress) =>
            patch({
              stage:
                progress.stage === 'extracting' || progress.stage === 'installing-deps'
                  ? 'installing'
                  : progress.stage === 'installingDeps'
                    ? 'deps'
                    : progress.stage,
              progress: progress.progress ?? 0,
              bytesDone: progress.bytesDone ?? 0,
              bytesPerSec: progress.bytesPerSec ?? 0,
            }),
          controller.signal,
        )
        // The badge must match what Rust just committed to disk: `trust`
        // arrives on the outcome and is patched straight onto the row, so
        // the correct level shows immediately instead of waiting for the
        // next full disk reload of the instance record.
        patchInstancePlugins(instanceId, (plugins) =>
          plugins.some((installed) => installed.pluginId === pluginId)
            ? plugins.map((installed) =>
                installed.pluginId === pluginId
                  ? { ...installed, version, registryId, enabled: true, ...(trust ? { trust } : {}) }
                  : installed,
              )
            : [...plugins, { pluginId, version, registryId, enabled: true, ...(trust ? { trust } : {}) }],
        )
        useUIStore.getState().toast({
          kind: 'success',
          title: `已安装到「${instance.name}」`,
          message: processLive(instanceId)
            ? `${plugin.name} ${version} · 实例正在运行，重启后才会加载新插件`
            : `${plugin.name} ${version}`,
        })
      } catch (err) {
        if (err instanceof Cancelled) {
          useUIStore.getState().toast({ kind: 'info', title: `已取消安装 ${plugin.name}` })
        } else {
          const reason = parseThrownError(err).message
          useUIStore.getState().toast({
            kind: 'error',
            title: `${plugin.name} 安装失败`,
            message: reason,
            action: { label: '重试', run: () => void get().installPlugin(instanceId, pluginId) },
          })
        }
      } finally {
        releaseTransferSlot(key)
        controllers.delete(key)
        set((current) => {
          const pluginTransfers = { ...current.pluginTransfers }
          delete pluginTransfers[key]
          return { pluginTransfers }
        })
      }
    },

    cancelPlugin(instanceId, pluginId) {
      controllers.get(pluginKey(instanceId, pluginId))?.abort()
    },

    async refreshLatestVersions(instanceId) {
      await Promise.all(launchLatestChecks(get, set, instanceId, false))
    },

    async recheckLatestVersions(instanceId) {
      await Promise.all(launchLatestChecks(get, set, instanceId, true))
    },

    async setPluginEnabled(instanceId, pluginId, enabled) {
      const instance = useInstanceStore.getState().byId(instanceId)
      if (!instance) return
      const installed = instance.plugins.find((plugin) => plugin.pluginId === pluginId)
      if (!installed) return
      const key = pluginKey(instanceId, pluginId)
      if (pluginToggles.has(key)) return
      pluginToggles.add(key)
      try {
        const label = get().pluginById(pluginId)?.name ?? pluginId
        if (!installed.linked) {
          try {
            await repository.setPluginEnabled(instance, installed, enabled)
          } catch (err) {
            useUIStore.getState().toast({
              kind: 'error',
              title: `${label} ${enabled ? '启用' : '停用'}失败`,
              message: parseThrownError(err).message,
            })
            return
          }
        }
        patchInstancePlugins(instanceId, (plugins) =>
          plugins.map((plugin) => (plugin.pluginId === pluginId ? { ...plugin, enabled } : plugin)),
        )
        // The toggle used to be SILENT on success: the switch visibly moved
        // while the instance was running, implying the live process had
        // re-read `cordis.patch.yml`. It had not.
        useUIStore.getState().toast(
          processLive(instanceId)
            ? { kind: 'info', title: `${label} ${enabled ? '启用' : '停用'}配置已保存，重启实例后生效` }
            : { kind: 'success', title: `${label} ${enabled ? '已启用' : '已停用'}` },
        )
      } finally {
        pluginToggles.delete(key)
      }
    },

    async uninstallPlugin(instanceId, pluginId) {
      const instance = useInstanceStore.getState().byId(instanceId)
      if (!instance) return
      const removed = instance.plugins.find((plugin) => plugin.pluginId === pluginId)
      if (!removed) return
      const label = get().pluginById(pluginId)?.name ?? pluginId
      if (!removed.linked) {
        // Pending on the card while the tree walk runs: before this the row
        // sat unchanged (or vanished, for pinned rows) until the command
        // resolved, and a second click hit the backend busy lock.
        const key = pluginKey(instanceId, pluginId)
        if (get().pluginTransfers[key]) return
        const patch = (transfer: import('./catalogTypes').PluginTransferState) =>
          set((current) => ({ pluginTransfers: { ...current.pluginTransfers, [key]: transfer } }))
        const clear = () =>
          set((current) => {
            const transfers = { ...current.pluginTransfers }
            delete transfers[key]
            return { pluginTransfers: transfers }
          })
        patch({ stage: 'removing', progress: 0, bytesDone: 0, bytesPerSec: 0 })
        try {
          await repository.uninstallPlugin(instance, removed)
        } catch (err) {
          clear()
          useUIStore.getState().toast({ kind: 'error', title: `${label} 卸载失败`, message: parseThrownError(err).message })
          return
        }
        clear()
      }
      patchInstancePlugins(instanceId, (plugins) => plugins.filter((plugin) => plugin.pluginId !== pluginId))
      useUIStore.getState().toast({
        kind: 'info',
        title: `已从「${instance.name}」移除 ${label}`,
        ...(processLive(instanceId)
          ? { message: '实例正在运行，该插件已随进程加载，重启后不再出现' }
          : {}),
      })
    },
  }
}
