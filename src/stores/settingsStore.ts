import { create } from 'zustand'
import { persist, createJSONStorage } from 'zustand/middleware'
import { PHL_ROOT } from '@/data/instances'
import {
  chooseDirectory,
  defaultRoot,
  initPhlRoot,
  isDesktop,
  rootDataSummary,
  setPhlRoot,
} from '@/lib/desktop'
import { useUIStore } from './uiStore'
import { normalizeRoot } from '@/lib/paths'
export { normalizeRoot } from '@/lib/paths'

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
  closeStopsInstances: boolean
  /**
   * After a desktop instance finishes launching, open (or focus) its WebUI in
   * the embedded window automatically. On by default — it is what a launcher
   * is for — but surfaced so a user who drives DSH from their own browser can
   * silence the popup. Browser builds never open a window regardless.
   */
  autoOpenWebUi: boolean
  /**
   * Check GitHub for a newer PHL build shortly after launch. The check only
   * reads a signed manifest — nothing is downloaded or installed without an
   * explicit click in 设置 → 关于.
   */
  autoUpdateCheck: boolean

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
  /**
   * Toast when a version that was GitHub-only becomes installable from npm.
   * Rides the catalog sync above — no extra network activity of its own.
   */
  pendingReleaseAlerts: boolean

  portStart: number
  /**
   * Not yet consumed by any logging pipeline — the settings UI shows it as a
   * disabled placeholder. Wiring it is part of "下载并发和日志等级在对应后端
   * 能力完成后开放" (roadmap O-03); until then it must not look live.
   */
  logLevel: LogLevel
  /**
   * Let model enrichment consult the public OpenRouter model list as a second
   * tier (only for OpenRouter routes / `vendor/model` ids; no key is sent).
   * Consumed by every `enrichModelMetadata` call.
   */
  enrichFromOpenRouter: boolean

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
  /**
   * The storage page's switch: adopt the root only if the backend accepted it.
   * Resolves true when the app and the backend agree on the new root.
   */
  setRootVerified: (root: string) => Promise<boolean>
  reset: () => void
}

const defaults = {
  root: DEFAULT_ROOT,
  startup: 'none' as StartupBehaviour,
  minimizeToTray: true,
  closeStopsInstances: true,
  autoOpenWebUi: true,
  autoUpdateCheck: true,
  source: 'official' as DownloadSource,
  customSource: '',
  concurrency: 2,
  keepArchives: false,
  versionRefreshMinutes: 30,
  pendingReleaseAlerts: true,
  portStart: 3080,
  logLevel: 'info' as LogLevel,
  enrichFromOpenRouter: true,
}
/*
 * Removed fake settings (roadmap O-03): `isolateNodeModules` (instance
 * isolation is a core product constraint, never a user-facing toggle) and
 * `developerMode` (no consumer existed). Older persisted stores may still
 * carry these keys; they are ignored — the merge reads only typed fields.
 */

export const useSettingsStore = create<SettingsState>()(
  persist(
    (set) => ({
      ...defaults,
      guideSeen: false,
      markGuideSeen: () => set({ guideSeen: true }),
      rootDecided: false,
      markRootDecided: () => set({ rootDecided: true }),
      set: (key, value) => set({ [key]: value } as Partial<SettingsState>),
      setRoot: (root) => {
        const next = normalizeRoot(root) || DEFAULT_ROOT
        set({ root: next })
        // Backend results (boot handshake or completed migration) are already
        // committed. Mirroring them must never write another root pointer.
        // User-requested changes go through setRootVerified instead.
      },
      /**
       * Switching the data root from 设置 → 存储.
       *
       * `setRoot` alone is not enough here: it only mirrors local state, so a root
       * the backend refuses (a relative path, an unwritable config dir) left the
       * field showing the new path while every read and write still resolved
       * against the old root — the list looked unchanged and edits landed in a
       * directory the user thought they had left. The backend's answer decides.
       */
      setRootVerified: async (root) => {
        const next = normalizeRoot(root) || DEFAULT_ROOT
        if (!isDesktop) {
          set({ root: next })
          return true
        }
        const accepted = await setPhlRoot(next)
        if (!accepted) return false
        set({ root: normalizeRoot(accepted) || next })
        return true
      },
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
 * The boot handshake with the backend's authoritative data root. The backend
 * adopts the root persisted here only when it has no pointer file yet —
 * existing installs keep their root through the upgrade to backend-owned
 * roots, and from then on the pointer file (not localStorage) wins. The
 * prototype placeholder is never offered: it is not a real directory.
 */
export async function syncPhlRootWithBackend(): Promise<void> {
  if (!isDesktop) return
  const stored = useSettingsStore.getState().root
  const authoritative = await initPhlRoot(stored === PHL_ROOT ? null : stored)
  if (!authoritative) return
  const normalized = normalizeRoot(authoritative)
  if (normalized && normalized !== useSettingsStore.getState().root) {
    useSettingsStore.getState().setRoot(normalized)
  }
}

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
  if (!(await useSettingsStore.getState().setRootVerified(next))) {
    useUIStore.getState().toast({
      kind: 'error',
      title: '无法使用所选目录',
      message: `${next} 没有被后端接受，数据目录保持不变。`,
      duration: 8000,
    })
    return
  }
  // Everything already read was resolved against the previous root; without
  // this the first screen keeps describing the directory the user just left.
  // Imported lazily: this module sits below the stores that would otherwise
  // cycle back into it.
  const [{ useInstanceStore }, { useCatalogStore }, { useApiConfigStore }] = await Promise.all([
    import('./instanceStore'),
    import('./catalogStore'),
    import('./apiConfigStore'),
  ])
  await Promise.all([
    useInstanceStore.getState().reload(),
    useCatalogStore.getState().load(),
    useApiConfigStore.getState().load(),
  ]).catch(() => undefined)
  useUIStore.getState().toast({ kind: 'success', title: '数据目录已更新', message: next })
}
