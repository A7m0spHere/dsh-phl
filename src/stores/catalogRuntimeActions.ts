import { Cancelled, repository } from '@/services'
import type { Runtime } from '@/types'
import { acquireTransferSlot, releaseTransferSlot } from './transferCoordinator'
import { maybeHintMirror, officialRegistryLooksSlow } from './catalogNotifications'
import { useUIStore } from './uiStore'
import type { CatalogGet, CatalogSet, RuntimeActions, TransferControllers } from './catalogTypes'

export function createRuntimeActions(
  set: CatalogSet,
  get: CatalogGet,
  controllers: TransferControllers,
): RuntimeActions {
  return {
    async installRuntime(id) {
      const runtime = get().runtimes.find((candidate) => candidate.id === id)
      if (!runtime) return
      const controller = new AbortController()
      const key = `r:${id}`
      controllers.set(key, controller)
      const startedAt = Date.now()
      const patch = (state: Runtime['state']) =>
        set((current) => ({
          runtimes: current.runtimes.map((candidate) => (candidate.id === id ? { ...candidate, state } : candidate)),
        }))

      patch({ kind: 'queued' })
      try {
        if (!(await acquireTransferSlot(key, controller.signal))) throw new Cancelled()
        await repository.installRuntime(
          runtime,
          (progress) => {
            if (progress.stage === 'downloading') {
              patch({
                kind: 'downloading',
                progress: progress.progress ?? 0,
                bytesDone: progress.bytesDone ?? 0,
                bytesPerSec: progress.bytesPerSec ?? 0,
              })
              maybeHintMirror(() => officialRegistryLooksSlow(startedAt, progress.bytesDone ?? 0))
            } else {
              patch({ kind: 'extracting', progress: progress.progress ?? 0 })
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
        releaseTransferSlot(key)
        controllers.delete(key)
      }
    },

    cancelRuntime(id) {
      controllers.get(`r:${id}`)?.abort()
    },

    async removeRuntime(id) {
      await repository.removeRuntime(id)
      set((current) => ({
        runtimes: current.runtimes.map((runtime) =>
          runtime.id === id ? { ...runtime, state: { kind: 'available' } } : runtime,
        ),
      }))
    },
  }
}
