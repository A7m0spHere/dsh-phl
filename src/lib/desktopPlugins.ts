import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { PluginSource } from '@/types'

/**
 * The plugin half of the desktop bridge (§M2 domain split): catalog listing,
 * install, latest-version probe, enable/disable, and uninstall. Moved out of
 * `desktop.ts` like Tasks / Pack / Sessions / Adoption / Runtimes / Diagnostics;
 * `desktop.ts` re-exports everything here so consumers are unchanged.
 */

export type RemotePluginSource = PluginSource

/** Mirrors the Rust `PluginMeta`. Live registry entries carry `releases: []`. */
export interface RemotePluginMeta {
  id: string
  name: string
  author: string
  category: string
  summary: string
  summaryEn?: string
  repoUrl?: string
  screenshots: string[]
  source: RemotePluginSource
  official?: boolean
  downloads: number
  stars?: number
  addedAt?: string
  releases: []
}

/** Exactly the Rust `PluginProgressEvent` wire words (camelCase serde tags).
 *  `installingDeps` (pnpm) and `committing` carry no numbers — unit variants. */
export type PluginProgressStage =
  | 'preparing'
  | 'downloading'
  | 'verifying'
  | 'installing'
  | 'installingDeps'
  | 'committing'

/** Mirrors the Rust `PluginCatalogWire` — entries plus fetch provenance. */
export interface RemotePluginCatalog {
  /** The base URL that served the catalog, or `'cache'` for an offline replay. */
  servedFrom: string
  usedFallback: boolean
  fromCache: boolean
  updated: string | null
  plugins: RemotePluginMeta[]
}

export async function listDshPlugins(catalogBase: string): Promise<RemotePluginCatalog> {
  if (!isDesktop) {
    return { servedFrom: catalogBase, usedFallback: false, fromCache: false, updated: null, plugins: [] }
  }
  return invoke<RemotePluginCatalog>('list_dsh_plugins', { registryBase: catalogBase })
}

export interface InstallPluginArgs {
  transferId: string
  /** Catalog id (`owner/repo`) — recorded so the install can be read back. */
  pluginId: string
  source: RemotePluginSource
  /** Pin a version (npm); omitted means "latest dist-tag". */
  version?: string
  /** npm registry used to resolve `npm` sources. */
  registryBase: string
  /** The owning instance's id — Rust resolves its profile directory itself. */
  instanceId: string
  onProgress: (event: {
    stage: PluginProgressStage
    progress?: number
    bytesDone?: number
    bytesPerSec?: number
  }) => void
}

export async function installPlugin(args: InstallPluginArgs): Promise<{
  version: string
  registryId: string
}> {
  if (!isDesktop) return { version: '', registryId: '' }
  const channel = new Channel<Parameters<InstallPluginArgs['onProgress']>[0]>()
  channel.onmessage = args.onProgress
  return invoke('install_plugin', {
    transferId: args.transferId,
    pluginId: args.pluginId,
    source: args.source,
    version: args.version ?? null,
    registryBase: args.registryBase,
    instanceId: args.instanceId,
    onProgress: channel,
  })
}

export async function pluginLatestVersion(
  source: RemotePluginSource,
  registryBase: string,
): Promise<{ version: string | null }> {
  if (!isDesktop) return { version: null }
  return invoke('plugin_latest_version', { source, registryBase })
}

export async function setPluginEnabled(
  instanceId: string,
  registryId: string,
  enabled: boolean,
): Promise<void> {
  if (!isDesktop) return
  await invoke('set_plugin_enabled', { instanceId, registryId, enabled })
}

export async function uninstallPlugin(instanceId: string, registryId: string): Promise<void> {
  if (!isDesktop) return
  await invoke('uninstall_plugin', { instanceId, registryId })
}
