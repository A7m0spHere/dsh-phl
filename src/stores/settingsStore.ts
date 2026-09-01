import { create } from 'zustand'
import { persist, createJSONStorage } from 'zustand/middleware'
import { PHL_ROOT } from '@/data/instances'

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

  portStart: number
  logLevel: LogLevel
  developerMode: boolean
  isolateNodeModules: boolean

  /** First-run guide has been completed or skipped. */
  guideSeen: boolean
  markGuideSeen: () => void

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
      set: (key, value) => set({ [key]: value } as Partial<SettingsState>),
      setRoot: (root) => set({ root: normalizeRoot(root) || DEFAULT_ROOT }),
      reset: () => set({ ...defaults }),
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
