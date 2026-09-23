import { create } from 'zustand'
import { parseThrownError } from '@/lib/errorCodes'
import { normalizeRoot } from '@/lib/paths'
import {
  cancelTransfer,
  migrationFinish,
  migrationStatus,
  migrationUndo,
  moveRootData,
  rootDataSummary,
  type MigrationJournal,
  type MoveProgress,
} from '@/lib/desktop'
import { formatBytes } from '@/lib/format'
import { useSettingsStore } from './settingsStore'
import { useUIStore } from './uiStore'

/**
 * The data-root switch flow as one business module.
 *
 * This used to live inline in the storage section of `SettingsPage`, which
 * meant the three paths that move the root under the app — the first-run
 * chooser, a manual switch, and a completed migration — each carried their
 * own copy of the "re-read everything from the new root" step, and the copies
 * had already drifted: one of them swallowed refresh failures and still
 * reported full success. The page now collects input and renders state; the
 * sequencing, guards, and post-commit refresh coordination live here.
 *
 * Conventions this module preserves:
 * - The backend decides whether a root is adopted (`setRootVerified`); the
 *   store mirrors the answer, never the other way around.
 * - Root switches are refused while any transfer, snapshot, save, or running
 *   instance work is in flight.
 * - Every commit-then-refresh reports honestly: a failed refresh is named,
 *   not folded into a success toast.
 */

interface DataRootState {
  /**
   * An in-flight root migration. While it exists the storage view shows a
   * progress overlay; the root itself only flips once the move reports done.
   */
  migration: { from: string; to: string; transferId: string } | null
  moveProgress: MoveProgress | null
  /**
   * A migration cancelled or interrupted by a crash leaves its journal
   * behind; entering the storage view surfaces it as 继续 / 撤销 (O-06),
   * so the half-moved directories are never a silent dead end.
   */
  journal: MigrationJournal | null
  journalBusy: 'undo' | 'finish' | null
  /** A root switch (simple or migration) is being applied right now. */
  applying: boolean

  /** Re-read the recovery journal from the backend (call when the storage view opens). */
  refreshJournal: () => Promise<void>
  /**
   * Re-read instances / catalog / API config from the (already committed)
   * new root. Resolves with an error summary, or null when everything came
   * back. Callers MUST NOT present unconditional success on a non-null
   * answer — the backend moved, but what the UI shows may still describe the
   * old root.
   */
  reloadViewsAfterRootSwitch: () => Promise<string | null>
  /**
   * The storage page's switch: adopt the root only if the backend accepted
   * it, then refresh. Resolves true when the switch landed cleanly.
   */
  switchRootTo: (next: string) => Promise<boolean>
  /** Run (or resume, with the journal's own from/to) a migration. */
  startMigration: (from: string, to: string) => Promise<void>
  cancelMigration: () => void
  /** Undo a non-committed migration after a dangerous confirm. */
  undoMigration: () => Promise<void>
  /** Commit a migration whose data all arrived but whose pointer switch never did. */
  finishCommittedMigration: () => Promise<void>
  /** Validate, guard, confirm — then switch or offer to migrate. `raw` is the user's input. */
  applyRootFlow: (raw: string) => Promise<void>
}

/**
 * Why a typed path cannot be a data root, in the same terms the backend
 * enforces (`paths::validate_root`). Checked before the two confirmations so
 * the user is not walked through a migration decision that cannot happen.
 */
export function rootProblem(raw: string): string | null {
  const path = raw.trim()
  if (!path) return '请输入目录路径。'
  if (!/^(?:[a-zA-Z]:[\\/]|\\\\|\/)/.test(path)) return '请填写绝对路径，例如 D:\\PHL。'
  if (/[\\/]\.{1,2}(?:[\\/]|$)/.test(path)) return '路径里不能包含 . 或 .. 这样的相对段。'
  return null
}

/**
 * Whether a root change must wait: running instances, saves, creates,
 * snapshot copies, API saves/syncs, and active transfers all hold paths
 * under the current root.
 */
async function rootSwitchBlocked(): Promise<boolean> {
  const [{ useInstanceStore }, { useCatalogStore }, { useApiConfigStore }] = await Promise.all([
    import('./instanceStore'),
    import('./catalogStore'),
    import('./apiConfigStore'),
  ])
  const live = useInstanceStore.getState()
  return (
    Object.values(live.states).some((s) => ['running', 'starting', 'stopping'].includes(s.status)) ||
    live.hasPendingWrites() ||
    !!live.createProgress ||
    Object.keys(live.snapshotTransfers).length > 0 ||
    useApiConfigStore.getState().saving ||
    useApiConfigStore.getState().pendingSaves > 0 ||
    !!useApiConfigStore.getState().syncing ||
    useCatalogStore.getState().activeTransfers() > 0
  )
}

export const useDataRootStore = create<DataRootState>()((set, get) => ({
  migration: null,
  moveProgress: null,
  journal: null,
  journalBusy: null,
  applying: false,

  async refreshJournal() {
    try {
      set({ journal: await migrationStatus() })
    } catch {
      set({ journal: null })
    }
  },

  async reloadViewsAfterRootSwitch() {
    const [{ useInstanceStore }, { useCatalogStore }, { useApiConfigStore }] = await Promise.all([
      import('./instanceStore'),
      import('./catalogStore'),
      import('./apiConfigStore'),
    ])
    const failures: string[] = []
    const attempt = async (label: string, run: () => Promise<void>) => {
      try {
        await run()
      } catch (err) {
        failures.push(`${label}：${parseThrownError(err).message}`)
      }
    }
    await Promise.all([
      attempt('实例列表', () => useInstanceStore.getState().reload()),
      attempt('版本与 Runtime', () => useCatalogStore.getState().load()),
      attempt('API 配置', () => useApiConfigStore.getState().load()),
    ])
    // `loaded`/`loading` guards inside the stores make repeat `load()` calls
    // no-ops after the first success, but the reload above is exactly what a
    // root switch needs; the instance reload is the only one that can throw
    // today (catalog and API config toast their own section failures).
    return failures.length ? failures.join('；') : null
  },

  async switchRootTo(next) {
    const settings = useSettingsStore.getState()
    // The backend has to adopt the path before the UI moves: a local-only
    // switch would leave the field pointing at one directory while every read
    // and write resolved against the other (see `setRootVerified`).
    if (!(await settings.setRootVerified(next))) {
      useUIStore.getState().toast({
        kind: 'error',
        title: '无法切换数据目录',
        message: `PHL 没有接受 ${next}。路径必须是绝对路径（例如 D:\\PHL），且配置目录可写；当前数据目录保持不变。`,
        duration: 8000,
      })
      return false
    }
    const refreshError = await get().reloadViewsAfterRootSwitch()
    if (refreshError) {
      useUIStore.getState().toast({
        kind: 'warn',
        title: '数据目录已切换，但部分列表未能刷新',
        message: `${refreshError} 目录已由后端切换；重启 PHL 可恢复显示。`,
        duration: 9000,
      })
    }
    return true
  },

  async startMigration(from, to) {
    if (get().migration) return
    const transferId = `move-${Date.now()}`
    set({ migration: { from, to, transferId }, moveProgress: null })
    const ui = useUIStore.getState()
    try {
      const summary = await moveRootData(from, to, transferId, (p) => set({ moveProgress: p }))
      if (summary.cancelled) {
        ui.toast({
          kind: 'info',
          title: '迁移已取消',
          message: `已完成 ${formatBytes(summary.bytes)}，数据目录未更改。进度已记录，可在「存储」页继续或撤销。`,
          duration: 6000,
        })
        return
      }
      if (!summary.root) {
        throw new Error('迁移结果缺少已提交的数据目录，请重启 PHL 重新读取目录状态。')
      }
      // The move committed the pointer backend-side; mirror it into the
      // settings store (never a second root write) and re-read the views.
      useSettingsStore.getState().setRoot(summary.root)
      const refreshError = await get().reloadViewsAfterRootSwitch()
      await get().refreshJournal()
      set({ journal: null })
      if (refreshError) {
        // The honest shape: the data moved, the view may not follow.
        ui.toast({
          kind: 'warn',
          title: '数据已迁移，但部分列表未能刷新',
          message: `${refreshError} 新目录已由后端生效；重启 PHL 可恢复显示。`,
          duration: 9000,
        })
      } else {
        ui.toast({
          kind: 'success',
          title: '数据迁移完成',
          message: `已迁移 ${summary.moved.length} 个目录（${formatBytes(summary.bytes)}），新目录即刻生效。`,
          duration: 6000,
        })
      }
    } catch (err) {
      // Rust reports a cancelled transfer as `Err("cancelled")`; showing the
      // failure toast for it told the user their migration broke when they had
      // simply stopped it.
      if (parseThrownError(err).message.trim() === 'cancelled') {
        ui.toast({
          kind: 'info',
          title: '迁移已取消',
          message: '数据目录未更改；已完成的部分保留在新目录，可在本页继续或撤销。',
          duration: 6000,
        })
      } else {
        ui.toast({
          kind: 'error',
          title: '迁移失败',
          message: `${parseThrownError(err).message} 数据目录未更改；已完成的部分保留在新目录，处理后可重新迁移续传。`,
          duration: 8000,
        })
      }
    } finally {
      set({ migration: null, moveProgress: null })
      // The journal is the truth: a cancelled run leaves a resume entry, a
      // committed one deletes itself. Reflect whichever outcome happened.
      await get().refreshJournal()
    }
  },

  cancelMigration() {
    const migration = get().migration
    if (migration) void cancelTransfer(migration.transferId)
  },

  async undoMigration() {
    const { journal, journalBusy } = get()
    if (!journal || journalBusy) return
    const ui = useUIStore.getState()
    const ok = await ui.confirm({
      title: '撤销未完成的迁移',
      message: '已复制到新目录的数据将全部搬回旧目录，新目录恢复迁移前的状态。期间不要移动这两个目录。',
      detail: `${journal.from}\n  ↑\n${journal.to}`,
      tone: 'danger',
      confirmLabel: '撤销迁移',
    })
    if (!ok) return
    set({ journalBusy: 'undo' })
    try {
      await migrationUndo()
      ui.toast({ kind: 'success', title: '迁移已撤销', message: '数据已回到旧目录。' })
      set({ journal: null })
      const refreshError = await get().reloadViewsAfterRootSwitch()
      if (refreshError) {
        ui.toast({
          kind: 'warn',
          title: '迁移已撤销，但部分列表未能刷新',
          message: `${refreshError} 数据已回到旧目录；重启 PHL 可恢复显示。`,
          duration: 9000,
        })
      }
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '撤销失败',
        message: parseThrownError(err).message,
        duration: 8000,
      })
    } finally {
      set({ journalBusy: null })
    }
  },

  async finishCommittedMigration() {
    const { journal, journalBusy } = get()
    if (!journal || journalBusy) return
    const ui = useUIStore.getState()
    set({ journalBusy: 'finish' })
    try {
      const adopted = await migrationFinish()
      if (adopted) {
        // The pointer is already the backend's; this mirrors it into the
        // settings store and re-reads instances/catalog/API from it.
        useSettingsStore.getState().setRoot(adopted)
        const refreshError = await get().reloadViewsAfterRootSwitch()
        set({ journal: null })
        if (refreshError) {
          ui.toast({
            kind: 'warn',
            title: '数据目录已切换，但部分列表未能刷新',
            message: `${refreshError} 重启 PHL 可恢复显示。`,
            duration: 9000,
          })
          return
        }
      }
      ui.toast({
        kind: 'success',
        title: '数据目录已切换',
        message: '迁移早已完成数据搬运，目录指针现已指向新目录。',
      })
      set({ journal: null })
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '完成切换失败',
        message: parseThrownError(err).message,
        duration: 8000,
      })
    } finally {
      set({ journalBusy: null })
    }
  },

  async applyRootFlow(raw) {
    const settings = useSettingsStore.getState()
    const ui = useUIStore.getState()
    if (get().applying || get().migration) return
    const next = normalizeRoot(raw)
    if (!next || next === settings.root) return
    const problem = rootProblem(next)
    if (problem) {
      ui.toast({ kind: 'warn', title: '数据目录无效', message: problem })
      return
    }
    set({ applying: true })
    try {
      if (await rootSwitchBlocked()) {
        ui.toast({
          kind: 'warn',
          title: '暂时无法更改数据目录',
          message: '请先停止所有实例，并等待下载、快照和配置保存完成。',
        })
        return
      }
      const ok = await ui.confirm({
        title: '更改数据目录',
        message: 'PHL 之后会从新目录读写版本、Runtime 与实例。',
        detail: `${settings.root}\n  ↓\n${next}`,
        tone: 'danger',
        confirmLabel: '仍要更改',
      })
      if (!ok) return
      const summary = await rootDataSummary(settings.root).catch(() => null)
      if (summary?.hasData) {
        const parts = [
          summary.instances.entries && `${summary.instances.entries} 个实例`,
          summary.versions.entries && `${summary.versions.entries} 个版本`,
          summary.runtimes.entries && `${summary.runtimes.entries} 个 Runtime`,
          summary.config.entries && 'API 配置库',
        ].filter(Boolean)
        const move = await ui.confirm({
          title: '立即迁移现有数据？',
          message: `旧目录中仍有 ${parts.join('、') || '数据'}。迁移会把它们连同下载缓存完整搬到新目录；不迁移的话它们将留在原处，且不再出现在 PHL 的列表里。`,
          confirmLabel: '立即迁移',
          cancelLabel: '以后再说',
        })
        if (move) {
          await get().startMigration(settings.root, next)
          return
        }
      }
      if (await get().switchRootTo(next)) {
        ui.toast({
          kind: 'info',
          title: '数据目录已更新',
          message: '旧目录的数据仍在原处，之后可在本页重新迁移。',
        })
      }
    } finally {
      set({ applying: false })
    }
  },
}))
