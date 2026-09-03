import * as desktop from '@/lib/desktop'
import { useSettingsStore } from '@/stores/settingsStore'
import type { InstalledPlugin, Instance, Plugin } from '@/types'
import { Cancelled, newTransferId } from './repository'
import type { PhlRepository, PluginCatalog, TransferProgress } from './repository'

/**
 * Desktop overrides for the **plugin module**: catalog from the real DSH
 * community registry (awesome-dsh-plugin `plugins.json`) and a real
 * download / verify / install pipeline through Rust. Everything the plugin
 * module does not own still falls through to the mock.
 */

/** Where `plugins.json` lives, per the download-source setting. */
function catalogBase(): string {
  const { source, customSource } = useSettingsStore.getState()
  // The repo has no aggregated JSON on jsDelivr/raw (only per-plugin YAML),
  // so the China-friendly mirror is dsh-ai.org's byte-compatible copy.
  if (source === 'mirror-cn') return 'https://dsh-ai.org'
  if (source === 'custom' && customSource.trim()) return customSource.trim()
  return 'https://awesome-dsh-plugin.com'
}

/** Catalog cache lives next to the version tarball cache. */
function cacheDir(): string {
  const { root } = useSettingsStore.getState()
  const sep = root.includes('\\') ? '\\' : '/'
  return `${root}${sep}cache`
}

/** Mirrors the version module's npm registry resolution. */
function npmRegistryBase(): string {
  const { source, customSource } = useSettingsStore.getState()
  if (source === 'mirror-cn') return 'https://registry.npmmirror.com'
  if (source === 'custom' && customSource.trim()) return customSource.trim()
  return 'https://registry.npmjs.org'
}

/**
 * The directory that owns `node_modules` and `cordis.patch.yml` for an
 * instance — mirrors DSH's own `~/.dsh/profiles/<profile>` layout, but kept
 * inside the instance's DSH home so instances stay fully isolated.
 */
export function pluginProfileRoot(instance: Instance): string {
  const sep = instance.dshHome.includes('\\') ? '\\' : '/'
  return `${instance.dshHome}${sep}profiles${sep}${instance.profile}`
}

/**
 * The id `cordis.patch.yml` is keyed by.
 *
 * Rust computes this at install time and hands it back, and it is stored on
 * the install record — so this is only the fallback for a record written
 * before that was recorded. It must not be re-derived from a catalog entry:
 * doing so needed the catalog (empty whenever the registry is unreachable)
 * and gave a second implementation that could disagree with Rust's.
 */
function fallbackRegistryId(installed: InstalledPlugin): string {
  // The catalog id is `owner/repo`; the registry id is the trailing segment
  // for GitHub sources and the full package name for npm ones.
  return installed.registryId || installed.pluginId.split('/').pop() || installed.pluginId
}

async function listPlugins(): Promise<PluginCatalog> {
  try {
    const remote = await desktop.listDshPlugins(catalogBase(), cacheDir())
    return { plugins: remote.map((meta) => ({ ...meta, category: meta.category as Plugin['category'] })) }
  } catch (err) {
    // The Rust side already tried every mirror plus its on-disk cache. The
    // bundled seed is *demo* data — packages like `@dsh-core/routing-suite`
    // do not exist on npm — so offering it here would give the user a list
    // of entries whose 安装 button can only ever 404. An empty catalog with
    // the real reason and a retry is the honest failure.
    const error = err instanceof Error ? err.message : String(err)
    console.warn('[phl] plugin catalog unavailable:', error)
    return { plugins: [], offline: true, error }
  }
}

async function installPlugin(
  plugin: Plugin,
  instance: Instance,
  onProgress: (p: TransferProgress) => void,
  signal: AbortSignal,
): Promise<{ version: string; registryId?: string }> {
  const transferId = newTransferId(`p:${instance.id}:${plugin.id}`)
  // `addEventListener('abort')` never fires on an already-aborted signal, so
  // a cancel that lands before this call would otherwise be ignored outright.
  if (signal.aborted) throw new Cancelled()
  const onAbort = () => void desktop.cancelTransfer(transferId)
  signal.addEventListener('abort', onAbort, { once: true })
  try {
    const outcome = await desktop.installPlugin({
      transferId,
      pluginId: plugin.id,
      source: plugin.source,
      registryBase: npmRegistryBase(),
      instanceRoot: pluginProfileRoot(instance),
      onProgress,
    })
    return { version: outcome.version, registryId: outcome.registryId }
  } catch (err) {
    if (signal.aborted) throw new Cancelled()
    throw err instanceof Error ? err : new Error(String(err))
  } finally {
    signal.removeEventListener('abort', onAbort)
  }
}

async function latestPluginVersion(plugin: Plugin): Promise<string | null> {
  const { version } = await desktop.pluginLatestVersion(plugin.source, npmRegistryBase())
  return version
}

/** The repository overrides, spread into the desktop repository selection. */
export const tauriPluginOverrides: Pick<
  PhlRepository,
  'listPlugins' | 'installPlugin' | 'setPluginEnabled' | 'uninstallPlugin' | 'latestPluginVersion'
> = {
  listPlugins,
  installPlugin,
  latestPluginVersion,
  setPluginEnabled: async (instance, installed, enabled) => {
    await desktop.setPluginEnabled(
      pluginProfileRoot(instance),
      fallbackRegistryId(installed),
      enabled,
    )
  },
  uninstallPlugin: async (instance, installed) => {
    await desktop.uninstallPlugin(pluginProfileRoot(instance), fallbackRegistryId(installed))
  },
}
