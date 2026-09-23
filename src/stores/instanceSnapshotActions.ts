import { repository, Cancelled, type CopyProgress } from '@/services'
import { parseThrownError } from '@/lib/errorCodes'
import { isInstanceLiveForSnapshot } from '@/types/instance'
import { catalogState } from './storeRefs'
import { useUIStore } from './uiStore'
import type { InstanceGet, InstanceSet } from './instanceTypes'

/**
 * Snapshot operations for the instance store.
 *
 * Extracted from `instanceStore` to separate "copy the dsh-home tree"
 * workflows (create / restore / cancel / delete, their progress and
 * mutual-exclusion bookkeeping) from the instance CRUD and persistence that
 * remain there. The public store surface is unchanged: the actions are spread
 * into the store object in `instanceStore.ts`.
 *
 * Ownership:
 * - `snapshotControllers` (module Map, owned here): one live copy per
 *   instance. Both create and restore register before the long copy leg and
 *   delete in `finally`; `abortSnapshotCopy` is the instance-store side's
 *   door in (called from `deleteInstance` before the tree is removed).
 * - `snapshotTransfers` / `snapshotOps` / `deletingSnapshots`: UI state on
 *   the store; this module is the only writer, and every writer clears its
 *   keys in `finally` on all paths.
 * - `pluginInstallActive`: a read of the catalog store, not of this module —
 *   the guard exists because Rust cannot see a plugin transfer while it
 *   copies/rolls back the very tree npm is writing into.
 */
const snapshotControllers = new Map<string, AbortController>()

/**
 * 插件安装正在往实例的 node_modules 里写文件；此刻拷贝它（快照）或换掉它
 * （回滚）都会得到撕裂的结果。Rust 看不到这些传输，只能在这里拦。
 */
function pluginInstallActive(id: string): boolean {
  const transfers = catalogState().pluginTransfers
  return Object.keys(transfers).some((key) => key.startsWith(`p:${id}:`))
}

/**
 * Aborts the instance's in-flight create/restore copy, if any. Called by the
 * store's `deleteInstance` before the backend removes the tree, so the copy
 * cannot read (or write) into a directory that is vanishing.
 */
export function abortSnapshotCopy(id: string): void {
  snapshotControllers.get(id)?.abort()
  snapshotControllers.delete(id)
}

export function createSnapshotActions(set: InstanceSet, get: InstanceGet) {
  const patchTransfer = (id: string, p: CopyProgress) =>
    set((cur) => ({ snapshotTransfers: { ...cur.snapshotTransfers, [id]: p } }))
  const clearTransfer = (id: string) =>
    set((cur) => {
      const transfers = { ...cur.snapshotTransfers }
      delete transfers[id]
      const ops = { ...cur.snapshotOps }
      delete ops[id]
      return { snapshotTransfers: transfers, snapshotOps: ops }
    })

  return {
    async createSnapshot(id: string) {
      const instance = get().byId(id)
      if (!instance || snapshotControllers.has(id)) return null
      const status = get().stateOf(id).status
      if (isInstanceLiveForSnapshot(status)) {
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
      set((cur) => ({ snapshotOps: { ...cur.snapshotOps, [id]: 'create' as const } }))
      patchTransfer(id, { progress: 0, bytesDone: 0, bytesTotal: 0 })
      try {
        const snap = await repository.createSnapshot(instance, (p) => patchTransfer(id, p), controller.signal)
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
            .toast({ kind: 'error', title: '创建快照失败', message: parseThrownError(err).message })
        }
        return null
      } finally {
        snapshotControllers.delete(id)
        clearTransfer(id)
      }
    },

    cancelSnapshot(id: string) {
      snapshotControllers.get(id)?.abort()
    },

    async restoreSnapshot(id: string, snapshotId: string) {
      const instance = get().byId(id)
      if (!instance) return
      // One snapshot copy per instance at a time — the same guard create uses;
      // a double click used to fire a second guarded task straight into a
      // busy-lock error while the first was still copying.
      if (snapshotControllers.has(id)) return
      const status = get().stateOf(id).status
      if (isInstanceLiveForSnapshot(status)) {
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
      set((cur) => ({ snapshotOps: { ...cur.snapshotOps, [id]: 'restore' as const } }))
      patchTransfer(id, { progress: 0, bytesDone: 0, bytesTotal: 0 })
      try {
        const fresh = await repository.restoreSnapshot(
          instance,
          snapshotId,
          (p) => patchTransfer(id, p),
          controller.signal,
        )
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
            .toast({ kind: 'error', title: '还原快照失败', message: parseThrownError(err).message })
        }
      } finally {
        snapshotControllers.delete(id)
        clearTransfer(id)
      }
    },

    async deleteSnapshot(id: string, snapshotId: string) {
      const instance = get().byId(id)
      if (!instance) return
      const key = `${id}:${snapshotId}`
      if (get().deletingSnapshots[key]) return
      // Deleting walks the copied tree; show the row busy and absorb repeat
      // clicks the same way instance deletion does.
      set((cur) => ({ deletingSnapshots: { ...cur.deletingSnapshots, [key]: true } }))
      try {
        await repository.deleteSnapshot(instance, snapshotId)
        get().updateInstance(id, {
          snapshots: (get().byId(id)?.snapshots ?? []).filter((s) => s.id !== snapshotId),
        })
      } catch (err) {
        useUIStore
          .getState()
          .toast({ kind: 'error', title: '删除快照失败', message: parseThrownError(err).message })
      } finally {
        set((cur) => {
          const deleting = { ...cur.deletingSnapshots }
          delete deleting[key]
          return { deletingSnapshots: deleting }
        })
      }
    },
  }
}
