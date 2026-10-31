import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { PluginSource, PluginEngineCompat } from '@/types'

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
  /** The trust the Rust transaction recorded with this commit (T-107). */
  trust: string
  /** The package's `engines.dsh` claim vs this instance's DSH (issue #10). */
  engineCompat: PluginEngineCompat
}> {
  if (!isDesktop) {
    return {
      version: '',
      registryId: '',
      trust: 'unknown',
      engineCompat: {
        pluginId: '',
        registryId: '',
        installedVersion: '',
        engineRange: null,
        verdict: 'unknown',
        reason: '浏览器模式下没有真实安装，不产生 engines.dsh 结论',
      },
    }
  }
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

/**
 * Read-only per-plugin `engines.dsh` verdicts (issue #10) for one instance's
 * installed plugins, evaluated backend-side against the instance's bound DSH
 * version. Mirrors the Rust `installed_plugin_compat` command.
 */
export async function installedPluginCompat(instanceId: string): Promise<PluginEngineCompat[]> {
  if (!isDesktop) return []
  return invoke<PluginEngineCompat[]>('installed_plugin_compat', { instanceId })
}

/** Mirrors the Rust `PluginHealthFinding` (issue #9). */
export interface PluginHealthFinding {
  kind: string
  severity: 'info' | 'warning' | 'error'
  plugin: string
  evidence: string
  impact: string
  suggestion: string
  repairable: boolean
  action: string | null
}

/** Mirrors the Rust `PluginHealthReport` (issue #9). */
export interface PluginHealthReport {
  instanceId: string
  /** False for external instances: the UI hides every repair button. */
  managed: boolean
  generatedAt: string
  findings: PluginHealthFinding[]
}

/** Issue #9: read-only plugin-environment diagnosis for one instance. */
export async function pluginHealthCheck(instanceId: string): Promise<PluginHealthReport> {
  if (!isDesktop) {
    // Same contract the browser-mode mock answers with (managed + clean):
    // the two fallbacks must not disagree about what "browser" means.
    return { instanceId, managed: true, generatedAt: '', findings: [] }
  }
  return invoke<PluginHealthReport>('plugin_health_check', { instanceId })
}

/**
 * Issue #9: run ONLY the repairs the user explicitly chose (the two safe
 * entry points); the backend refuses anything else and returns a FRESH
 * report so the panel shows the post-repair truth.
 */
export async function pluginHealthRepair(
  instanceId: string,
  actions: string[],
): Promise<PluginHealthReport> {
  if (!isDesktop) throw new Error('插件体检修复仅在桌面应用内可用')
  return invoke<PluginHealthReport>('plugin_health_repair', { instanceId, actions })
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
