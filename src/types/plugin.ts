/**
 * Plugins belong to an Instance, never to the machine. The catalog below is
 * the *registry* view; per-instance installs are recorded on the Instance.
 */
export type PluginCategory = 'routing' | 'tooling' | 'provider' | 'ui' | 'workflow' | 'diagnostics'

export interface PluginRelease {
  version: string
  publishedAt: string
  /** DSH version ids this release is verified against. */
  compatible: string[]
  /** DSH version ids known to break. */
  incompatible?: string[]
}

export interface Plugin {
  id: string
  name: string
  author: string
  category: PluginCategory
  summary: string
  /** Newest release, first element of `releases`. */
  releases: PluginRelease[]
  official?: boolean
  downloads: number
}

/** A plugin as installed inside one Instance. */
export interface InstalledPlugin {
  pluginId: string
  version: string
  enabled: boolean
  /** Local, unpublished plugin being developed against this instance. */
  linked?: boolean
}

export const latestRelease = (p: Plugin) => p.releases[0]
