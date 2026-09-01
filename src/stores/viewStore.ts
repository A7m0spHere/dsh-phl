import { create } from 'zustand'

/**
 * Per-page view state. It lives outside the pages because the left context
 * panel and the content area are separate subtrees that must agree on the
 * same filters.
 */

export type InstanceFilter = 'all' | 'running' | 'stopped' | 'favorite'
export type InstanceSort = 'recent' | 'name' | 'version' | 'created'
export type VersionFilter = 'all' | 'installed' | 'available' | 'legacy'
export type PluginTab = 'installed' | 'registry' | 'updates'
export type SettingsSection = 'general' | 'downloads' | 'appearance' | 'storage' | 'advanced' | 'about'

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
  setPluginTab: (t: PluginTab) => void
  setPluginScope: (s: string) => void
  setPluginQuery: (q: string) => void

  settingsSection: SettingsSection
  setSettingsSection: (s: SettingsSection) => void
}

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
  pluginScope: 'plugin-dev',
  pluginQuery: '',
  setPluginTab: (pluginTab) => set({ pluginTab }),
  setPluginScope: (pluginScope) => set({ pluginScope }),
  setPluginQuery: (pluginQuery) => set({ pluginQuery }),

  settingsSection: 'general',
  setSettingsSection: (settingsSection) => set({ settingsSection }),
}))
