import { create } from 'zustand'
import {
  inspectOfficialDesktop,
  launchOfficialDesktop,
  quitOfficialDesktop,
} from '@/lib/desktop'
import { parseThrownError } from '@/lib/errorCodes'
import { useUIStore } from './uiStore'
import type { OfficialDesktopInfo } from '@/types'

/**
 * State for the 「官方桌面端」 singleton card. Everything here is a live
 * probe — the official app self-updates on the nightly channel, so there is
 * no cache to invalidate, only the latest observation. The poll loop is
 * card-scoped: it runs while the instances page shows the card and stops
 * when the card unmounts (or once there is nothing left to watch).
 *
 * Browser mode: `inspectOfficialDesktop` resolves `null` and the card shows
 * a desktop-only notice — this store never fabricates an official install
 * (the same rule `adoptionStore` follows for discovery).
 */

interface OfficialDesktopState {
  info: OfficialDesktopInfo | null
  /** True once the first probe answered (including `null` / errors). */
  loaded: boolean
  loading: boolean
  launching: boolean
  quitting: boolean
  error: string | null

  refresh: () => Promise<void>
  launch: () => Promise<void>
  /** Graceful quit; on `still_running` asks the user before going force. */
  requestQuit: () => Promise<void>
}

export const useOfficialDesktopStore = create<OfficialDesktopState>((set, get) => ({
  info: null,
  loaded: false,
  loading: false,
  launching: false,
  quitting: false,
  error: null,

  async refresh() {
    set({ loading: true, error: null })
    try {
      const info = await inspectOfficialDesktop()
      set({ info, loaded: true, loading: false })
    } catch (err) {
      // The probe failed: keep the card showing 「无法确认」, not whatever
      // the last poll said.
      set({ loaded: true, loading: false, error: parseThrownError(err).message })
    }
  },

  async launch() {
    const ui = useUIStore.getState()
    set({ launching: true, error: null })
    try {
      await launchOfficialDesktop()
      ui.toast({ kind: 'success', title: '官方桌面端已启动', message: '窗口由官方壳自行拉起。' })
    } catch (err) {
      ui.toast({ kind: 'error', title: '无法启动官方桌面端', message: parseThrownError(err).message })
    } finally {
      set({ launching: false })
      await get().refresh()
    }
  },

  async requestQuit() {
    const ui = useUIStore.getState()
    set({ quitting: true, error: null })
    try {
      const outcome = await quitOfficialDesktop(false)
      if (outcome === 'still_running') {
        // Fail-closed: PHL cannot see the official app's own quit-inspection
        // (running agents, scheduled jobs) — the polite request was delivered
        // but the app stayed. Only the user decides what happens next.
        const ok = await ui.confirm({
          title: '强制结束官方桌面端？',
          message:
            '官方桌面端没有在宽限期内退出。PHL 无法确认它是否空闲——可能有正在运行的任务或未保存的会话。',
          detail: '也可以先去官方桌面端确认：它可能正在等待你处理一个退出提示。',
          confirmLabel: '强制结束',
          cancelLabel: '先不结束',
          tone: 'danger',
        })
        if (!ok) return
        const forced = await quitOfficialDesktop(true)
        if (forced === 'still_running') {
          ui.toast({
            kind: 'error',
            title: '强制结束未生效',
            message: '官方桌面端仍在运行，请稍后重试或手动关闭。',
          })
        } else {
          ui.toast({ kind: 'success', title: '官方桌面端已退出' })
        }
      } else {
        ui.toast({ kind: 'success', title: '官方桌面端已退出' })
      }
    } catch (err) {
      ui.toast({ kind: 'error', title: '无法退出官方桌面端', message: parseThrownError(err).message })
    } finally {
      set({ quitting: false })
      await get().refresh()
    }
  },
}))

/* ------------------------------ poll loop -------------------------------- */

const POLL_MS = 5000

let pollActive = false
let pollTimer: number | null = null

function worthWatching(info: OfficialDesktopInfo | null): boolean {
  // installed/unknown keep watching; not_installed/unsupported have nothing
  // to watch, and the loop stops until the card remounts or a launch is
  // attempted.
  return info?.status === 'installed' || info?.status === 'unknown'
}

function scheduleTick(): void {
  pollTimer = window.setTimeout(async () => {
    const store = useOfficialDesktopStore.getState()
    await store.refresh()
    if (!pollActive) return
    if (worthWatching(useOfficialDesktopStore.getState().info)) {
      scheduleTick()
    } else {
      stopOfficialDesktopPolling()
    }
  }, POLL_MS)
}

/** Start (or resume) the card-scoped poll loop. Called from the card's mount
 *  effect; idempotent, so several mounts share one loop. */
export function startOfficialDesktopPolling(): void {
  if (pollActive) return
  pollActive = true
  void useOfficialDesktopStore.getState().refresh()
  scheduleTick()
}

export function stopOfficialDesktopPolling(): void {
  pollActive = false
  if (pollTimer !== null) {
    window.clearTimeout(pollTimer)
    pollTimer = null
  }
}
