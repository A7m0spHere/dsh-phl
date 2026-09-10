import { create } from 'zustand'
import {
  checkAppUpdate,
  clearPendingUpdate,
  installAppUpdate,
  type AppUpdateInfo,
} from '@/lib/desktopUpdate'
import { useUIStore } from './uiStore'

/**
 * Update state for the whole app: the startup check, the settings panel and the
 * install flow all read one source. Kept out of settingsStore (which persists
 * preferences) because none of this survives a restart.
 */

export type UpdateStatus =
  | 'idle'
  | 'checking'
  | 'available'
  | 'current'
  | 'downloading'
  | 'installing'
  | 'error'

interface UpdateState {
  status: UpdateStatus
  info: AppUpdateInfo | null
  error: string | null
  downloaded: number
  total: number | null
  /** Epoch ms of the last completed check, for the "last checked" line. */
  checkedAt: number | null
  check: (options?: { silent?: boolean }) => Promise<void>
  install: () => Promise<void>
  dismiss: () => void
}

export const useUpdateStore = create<UpdateState>()((set, get) => ({
  status: 'idle',
  info: null,
  error: null,
  downloaded: 0,
  total: null,
  checkedAt: null,

  /**
   * silent is the startup path: a failed check there must not interrupt
   * someone who is busy, so the error is kept for the settings panel instead
   * of being toasted. A manual check always reports what happened.
   */
  check: async (options) => {
    const silent = options?.silent ?? false
    const status = get().status
    if (status === 'checking' || status === 'downloading' || status === 'installing') return
    set({ status: 'checking', error: null })
    try {
      const info = await checkAppUpdate()
      set({
        status: info ? 'available' : 'current',
        info,
        error: null,
        checkedAt: Date.now(),
      })
      if (info) {
        useUIStore.getState().toast({
          kind: 'info',
          title: '有新版本 ' + info.version,
          message: '设置 → 关于 里可以下载并安装。',
          duration: 6000,
        })
      } else if (!silent) {
        useUIStore.getState().toast({
          kind: 'success',
          title: '已是最新版本',
          message: '当前版本 ' + (get().info?.currentVersion ?? ''),
        })
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      set({ status: 'error', error: message, checkedAt: Date.now() })
      if (!silent) {
        useUIStore.getState().toast({
          kind: 'error',
          title: '检查更新失败',
          message,
        })
      }
    }
  },

  install: async () => {
    const info = get().info
    const status = get().status
    if (!info || status === 'downloading' || status === 'installing') return
    set({ status: 'downloading', downloaded: 0, total: null, error: null })
    try {
      await installAppUpdate((progress) => {
        if (progress.kind === 'started') set({ total: progress.total })
        else if (progress.kind === 'progress')
          set({ downloaded: progress.downloaded, total: progress.total })
        else set({ status: 'installing' })
      })
      // Reached only when the installer did not replace this process (a
      // cancelled run): say so instead of pretending the update landed.
      set({ status: 'installing' })
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      set({ status: 'error', error: message })
      useUIStore.getState().toast({ kind: 'error', title: '安装更新失败', message })
    }
  },

  dismiss: () => {
    clearPendingUpdate()
    set({ status: 'idle', info: null, error: null, downloaded: 0, total: null })
  },
}))
