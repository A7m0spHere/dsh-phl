import { create } from 'zustand'
import type { Instance } from '@/types'

/**
 * Per-page view state. It lives outside the pages because the left context
 * panel and the content area are separate subtrees that must agree on the
 * same filters.
 */

export type InstanceFilter = 'all' | 'running' | 'stopped' | 'favorite'
export type InstanceSort = 'recent' | 'name' | 'version' | 'created'
export type VersionFilter = 'all' | 'installed' | 'available' | 'legacy'
export type PluginTab = 'installed' | 'registry' | 'updates'
export type SettingsSection =
  | 'general'
  | 'downloads'
  | 'appearance'
  | 'shortcuts'
  | 'storage'
  | 'diagnostics'
  | 'advanced'
  | 'about'

interface ViewState {
  instanceQuery: string
  instanceFilter: InstanceFilter
  instanceSort: InstanceSort
  setInstanceQuery: (q: string) => void
  setInstanceFilter: (f: InstanceFilter) => void
  setInstanceSort: (s: InstanceSort) => void

  versionFilter: VersionFilter
  versionQuery: string
  setVersionFilter: (f: VersionFilter) => void
  setVersionQuery: (q: string) => void

  pluginTab: PluginTab
  /** `all` or an instance id — plugins are always scoped to an instance. */
  pluginScope: string
  pluginQuery: string
  /** Registry filter; `all` or a real registry category key. */
  pluginCategory: string
  /** Registry source filter: `all` / `npm` / `github` / `tarball`. */
  pluginSource: 'all' | 'npm' | 'github' | 'tarball'
  pluginSort: 'recommended' | 'stars' | 'newest'
  setPluginTab: (t: PluginTab) => void
  setPluginScope: (s: string) => void
  setPluginQuery: (q: string) => void
  setPluginCategory: (c: string) => void
  setPluginSource: (s: 'all' | 'npm' | 'github' | 'tarball') => void
  setPluginSort: (s: 'recommended' | 'stars' | 'newest') => void

  /**
   * Plugin ids in the discovery strip. Held here rather than in the page so
   * the batch survives a tab switch — rerolling every time the user glances
   * at 已安装 and comes back would make the strip impossible to act on.
   */
  pluginPicks: string[]
  /** Ids from the last few batches, so 换一批 does not repeat itself. */
  pluginPicksSeen: string[]
  setPluginPicks: (ids: string[]) => void

  settingsSection: SettingsSection
  setSettingsSection: (s: SettingsSection) => void
}

/**
 * How many recently-shown ids to remember. Three batches is enough to stop
 * the obvious repeats without narrowing the candidate pool in any way that
 * matters against a registry of this size.
 */
const PICK_MEMORY = 36

export const useViewStore = create<ViewState>()((set) => ({
  instanceQuery: '',
  instanceFilter: 'all',
  instanceSort: 'recent',
  setInstanceQuery: (instanceQuery) => set({ instanceQuery }),
  setInstanceFilter: (instanceFilter) => set({ instanceFilter }),
  setInstanceSort: (instanceSort) => set({ instanceSort }),

  versionFilter: 'all',
  versionQuery: '',
  setVersionFilter: (versionFilter) => set({ versionFilter }),
  setVersionQuery: (versionQuery) => set({ versionQuery }),

  pluginTab: 'installed',
  pluginScope: '',
  pluginQuery: '',
  pluginCategory: 'all',
  pluginSource: 'all',
  pluginSort: 'recommended',
  setPluginTab: (pluginTab) => set({ pluginTab }),
  setPluginScope: (pluginScope) => set({ pluginScope }),
  setPluginQuery: (pluginQuery) => set({ pluginQuery }),
  setPluginCategory: (pluginCategory) => set({ pluginCategory }),
  setPluginSource: (pluginSource) => set({ pluginSource }),
  setPluginSort: (pluginSort) => set({ pluginSort }),

  pluginPicks: [],
  pluginPicksSeen: [],
  setPluginPicks: (pluginPicks) =>
    set((s) => ({
      pluginPicks,
      // Newest first, capped — the tail falls off and becomes eligible again.
      pluginPicksSeen: [...pluginPicks, ...s.pluginPicksSeen].slice(0, PICK_MEMORY),
    })),

  settingsSection: 'general',
  setSettingsSection: (settingsSection) => set({ settingsSection }),
}))

/**
 * The instance the plugin pages are scoped to.
 *
 * `pluginScope` starts empty and can point at a deleted instance, so it has
 * to be resolved — but every consumer must resolve it *the same way*. The
 * page falling back to `instances[0]` while the panel and the scope selector
 * used the raw lookup meant the UI showed no selection while 安装 quietly
 * targeted the first instance.
 */
export function resolvePluginScope(instances: Instance[], scope: string): Instance | undefined {
  return instances.find((i) => i.id === scope) ?? instances[0]
}
