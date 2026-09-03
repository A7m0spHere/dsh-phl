import { create } from 'zustand'
import { persist, createJSONStorage } from 'zustand/middleware'
import { PHL_ROOT } from '@/data/instances'
import { chooseDirectory, defaultRoot, isDesktop, rootDataSummary } from '@/lib/desktop'
import { useUIStore } from './uiStore'

/**
 * Preferences that belong to PHL Core rather than to the UI shell. They are
 * persisted here so the prototype behaves like the real thing; Phase 2 moves
 * the same shape behind the repository.
 */
export type StartupBehaviour = 'none' | 'last' | 'favorites'
export type DownloadSource = 'official' | 'mirror-cn' | 'custom'
export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace'

/**
 * Where PHL keeps versions, runtimes and instances.
 *
 * This is deliberately *not* the install directory. A DSH version is ~50 MB,
 * a runtime ~30 MB, and an instance with its own `node_modules` can run to
 * several GB — putting that on the system drive by default is exactly the
 * thing users complain about. The default stays next to the user profile so
 * a fresh install works with no setup, but it is a first-class setting.
 */
export const DEFAULT_ROOT = PHL_ROOT

interface SettingsState {
  root: string
  startup: StartupBehaviour
  minimizeToTray: boolean
  checkUpdates: boolean
  closeStopsInstances: boolean

  source: DownloadSource
  customSource: string
  concurrency: number
  keepArchives: boolean
  /**
   * How often the version catalog is silently re-synced from npm/GitHub,
   * in minutes. 0 disables the timer entirely. The poller only fires while
   * the window is visible, so a tray-minimized PHL does no background work.
   */
  versionRefreshMinutes: number

  portStart: number
  logLevel: LogLevel
  developerMode: boolean
  isolateNodeModules: boolean

  /** First-run guide has been completed or skipped. */
  guideSeen: boolean
  markGuideSeen: () => void

  /**
   * The "where should the data live" question has been settled — either the
   * user picked a root on first launch, or an existing install proved it
   * already has data and must not be nagged.
   */
  rootDecided: boolean
  markRootDecided: () => void

  set: <K extends keyof SettingsState>(key: K, value: SettingsState[K]) => void
  setRoot: (root: string) => void
  reset: () => void
}

const defaults = {
  root: DEFAULT_ROOT,
  startup: 'none' as StartupBehaviour,
  minimizeToTray: true,
  checkUpdates: true,
  closeStopsInstances: true,
  source: 'official' as DownloadSource,
  customSource: '',
  concurrency: 2,
  keepArchives: false,
  versionRefreshMinutes: 30,
  portStart: 3080,
  logLevel: 'info' as LogLevel,
  developerMode: false,
  isolateNodeModules: true,
}

/** Trim trailing separators so joined paths never double up. */
export const normalizeRoot = (raw: string) => raw.trim().replace(/[\\/]+$/, '')

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set) => ({
      ...defaults,
      guideSeen: false,
      markGuideSeen: () => set({ guideSeen: true }),
      rootDecided: false,
      markRootDecided: () => set({ rootDecided: true }),
      set: (key, value) => set({ [key]: value } as Partial<SettingsState>),
      setRoot: (root) => set({ root: normalizeRoot(root) || DEFAULT_ROOT }),
      /**
       * Everything except the data root. `defaults.root` is the prototype's
       * placeholder path, and the root now drives real filesystem commands —
       * resetting it would silently re-point the whole app at a directory
       * belonging to a user that does not exist on this machine.
       */
      reset: () => set((s) => ({ ...defaults, root: s.root })),
    }),
    {
      name: 'phl.settings',
      storage: createJSONStorage(() => localStorage),
      // `guideSeen` must survive a settings reset — re-showing the tutorial
      // because someone reset their download source would be obnoxious.
      partialize: (s) => s,
    },
  ),
)

/**
 * The prototype's fake root (`C:\Users\dev\…`) is only a placeholder. On a
 * real desktop the default becomes the machine's actual local-appdata path —
 * resolve it once at boot and migrate any settings that still carry the
 * placeholder (an explicitly chosen root is left alone).
 */
export async function initDesktopRoot(): Promise<void> {
  if (!isDesktop) return
  const real = await defaultRoot()
  if (!real) return
  const normalized = normalizeRoot(real)
  if (useSettingsStore.getState().root === PHL_ROOT) {
    useSettingsStore.getState().setRoot(normalized)
  }
}

/**
 * One-time "where should the data live" prompt, aimed at fresh installs whose
 * default root sits on the system drive. Runs only when the question has not
 * been answered yet *and* the current root has no data — an existing install
 * is silently marked decided instead of being nagged. Skipped entirely in the
 * browser preview, where none of it can be acted on.
 */
export async function maybeOfferRootChoice(): Promise<void> {
  if (!isDesktop) return
  const settings = useSettingsStore.getState()
  if (settings.rootDecided) return
  settings.markRootDecided()
  const summary = await rootDataSummary(settings.root).catch(() => null)
  if (!summary || summary.hasData) return

  const wantsChange = await useUIStore.getState().confirm({
    title: '选择 PHL 数据目录',
    message:
      '实例、DSH 版本与 Runtime 默认存放在下面这个位置。如果系统盘空间紧张，建议现在改到其他分区；之后可以随时在 设置 → 存储 中更改。',
    detail: settings.root,
    confirmLabel: '选择其他位置…',
    cancelLabel: '使用默认位置',
  })
  if (!wantsChange) return
  const picked = await chooseDirectory(settings.root)
  if (!picked) return
  const next = normalizeRoot(picked)
  if (!next || next === settings.root) return
  useSettingsStore.getState().setRoot(next)
  useUIStore.getState().toast({ kind: 'success', title: '数据目录已更新', message: next })
}
