import { invoke, Channel } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import type {
  ApiBinding,
  ApiConfig,
  InstanceLiveSnapshot,
  PluginSource,
  VersionSource,
} from '@/types'

/**
 * The seam between PHL's UI and the desktop shell.
 *
 * The app runs in a frameless Tauri window with its own title bar, so window
 * controls are the app's responsibility. Everything here degrades to a no-op
 * in a plain browser, which keeps `npm run dev` usable for pure UI work
 * without booting Rust.
 */

export const isDesktop =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in (window as object)

type Unlisten = () => void

const noop = async () => {}

function currentWindow() {
  return getCurrentWindow()
}

export const desktop = {
  isDesktop,

  /** Reveal the window once the first paint is done (it starts hidden). */
  async ready(): Promise<void> {
    if (!isDesktop) return
    try {
      await invoke('app_ready')
    } catch {
      // If the command is unavailable the Rust-side timeout still shows it.
    }
  },

  async minimize(): Promise<void> {
    if (!isDesktop) return noop()
    await currentWindow().minimize()
  },

  async toggleMaximize(): Promise<void> {
    if (!isDesktop) return noop()
    await currentWindow().toggleMaximize()
  },

  /**
   * Asks the window to close. Rust intercepts the request and hands it back
   * to the frontend through `onCloseRequested`, so the confirmation dialog is
   * the same whether the user clicks our button or the OS asks.
   */
  async requestClose(): Promise<void> {
    if (!isDesktop) return noop()
    await currentWindow().close()
  },

  /** Actually quit, after the user has confirmed. */
  async exit(): Promise<void> {
    if (!isDesktop) return noop()
    await invoke('exit_app')
  },

  async isMaximized(): Promise<boolean> {
    if (!isDesktop) return false
    try {
      return await currentWindow().isMaximized()
    } catch {
      return false
    }
  },

  /** Fires whenever the window is resized, which covers maximize/restore. */
  async onResized(handler: () => void): Promise<Unlisten> {
    if (!isDesktop) return () => {}
    return currentWindow().onResized(handler)
  },

  async onCloseRequested(handler: () => void): Promise<Unlisten> {
    if (!isDesktop) return () => {}
    return currentWindow().listen('phl://close-requested', () => handler())
  },
}

/**
 * Native folder picker. Returns `null` in the browser (and when the user
 * cancels), so callers must handle "no choice was made" either way.
 */
export async function chooseDirectory(defaultPath?: string): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const picked = await open({
      directory: true,
      multiple: false,
      title: '选择 PHL 数据目录',
      defaultPath,
    })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}

/* -------------------------------- storage ------------------------------- */

/** One-level look at `<root>/<kind>` — cheap, no deep walks. */
export interface DirSummary {
  exists: boolean
  entries: number
}

export interface RootDataSummary {
  instances: DirSummary
  versions: DirSummary
  runtimes: DirSummary
  cache: DirSummary
  hasData: boolean
}

export interface MoveProgress {
  /** Which data directory is being moved: instances/versions/runtimes/cache. */
  kind: string
  progress: number
  bytesDone: number
  bytesTotal: number
}

export interface MoveSummary {
  moved: string[]
  bytes: number
  cancelled: boolean
}

/**
 * Free bytes on the drive containing `path`. The browser mock keeps the demo
 * interesting: C: reads roomy, D:/E: tighter. Desktop errors (unmapped drive)
 * resolve to `null` so callers can simply hide the figure.
 */
export async function freeSpace(path: string): Promise<number | null> {
  if (!isDesktop) {
    if (/^[eE]:/.test(path)) return 8.4 * 1024 ** 3
    if (/^[dD]:/.test(path)) return 24.6 * 1024 ** 3
    return 118.3 * 1024 ** 3
  }
  try {
    return await invoke<number>('free_space', { path })
  } catch {
    return null
  }
}

export async function rootDataSummary(root: string): Promise<RootDataSummary> {
  if (!isDesktop) {
    // Browser demo: the mock world always "has" data, so the migration
    // prompt and its flow are demonstrable.
    return {
      instances: { exists: true, entries: 3 },
      versions: { exists: true, entries: 2 },
      runtimes: { exists: true, entries: 1 },
      cache: { exists: true, entries: 1 },
      hasData: true,
    }
  }
  return invoke<RootDataSummary>('root_data_summary', { root })
}

/**
 * Moves instances/versions/runtimes/cache from one root to another.
 * Cancellation is a normal outcome (`cancelled: true`) with partial progress
 * preserved and resumable by calling again with the same roots.
 */
export async function moveRootData(
  from: string,
  to: string,
  transferId: string,
  onProgress: (p: MoveProgress) => void,
): Promise<MoveSummary> {
  if (!isDesktop) {
    const kinds = ['instances', 'versions', 'runtimes', 'cache']
    const sizes = [4.2e9, 0.9e9, 0.6e9, 0.2e9]
    for (let i = 0; i < kinds.length; i++) {
      for (let step = 1; step <= 8; step++) {
        await new Promise((resolve) => setTimeout(resolve, 120))
        onProgress({
          kind: kinds[i],
          progress: step / 8,
          bytesDone: (sizes[i] * step) / 8,
          bytesTotal: sizes[i],
        })
      }
    }
    return { moved: kinds, bytes: sizes.reduce((a, b) => a + b, 0), cancelled: false }
  }
  const channel = new Channel<MoveProgress>()
  channel.onmessage = onProgress
  return invoke<MoveSummary>('move_root_data', { from, to, transferId, onProgress: channel })
}

/* ------------------------------- versions ------------------------------- */

/** `DshVersion` minus the runtime `state`, which the repository derives. */
export type RemoteVersionMeta = Omit<DshVersionShape, 'state'>
interface DshVersionShape {
  id: string
  name: string
  channel: string
  releasedAt: string
  size: number
  requiresNode: number[]
  notes: string[]
  latest?: boolean
  legacy?: boolean
  source?: VersionSource
}

export interface InstalledVersionInfo {
  name: string
  installedAt: string
}

/** PHL's own data root (`%LOCALAPPDATA%\PHL` by default). */
export async function defaultRoot(): Promise<string | null> {
  if (!isDesktop) return null
  try {
    return await invoke<string>('default_root')
  } catch {
    return null
  }
}

export async function listDshVersions(registryBase: string): Promise<RemoteVersionMeta[]> {
  if (!isDesktop) return []
  return invoke<RemoteVersionMeta[]>('list_dsh_versions', { registryBase })
}

export async function listInstalledVersions(root: string): Promise<InstalledVersionInfo[]> {
  if (!isDesktop) return []
  return invoke<InstalledVersionInfo[]>('list_installed_versions', { root })
}

export async function removeVersionDir(root: string, versionName: string): Promise<void> {
  if (!isDesktop) return
  await invoke('remove_version_dir', { root, versionName })
}

export interface DownloadArgs {
  transferId: string
  tarballUrl: string
  integrity?: string
  versionName: string
  root: string
  /** npm registry base — used to install the package's own dependencies. */
  registryBase: string
  keepArchive: boolean
  /** Catalog size in bytes — the CDN often streams without content-length. */
  totalBytes?: number
  onProgress: (event: {
    stage: 'downloading' | 'extracting' | 'verifying'
    progress: number
    bytesDone: number
    bytesPerSec: number
  }) => void
}

export async function downloadDshVersion(args: DownloadArgs): Promise<void> {
  if (!isDesktop) return
  const channel = new Channel<Parameters<DownloadArgs['onProgress']>[0]>()
  channel.onmessage = args.onProgress
  await invoke('download_dsh_version', {
    transferId: args.transferId,
    tarballUrl: args.tarballUrl,
    integrity: args.integrity ?? null,
    versionName: args.versionName,
    root: args.root,
    registryBase: args.registryBase,
    keepArchive: args.keepArchive,
    totalBytes: args.totalBytes ?? null,
    onProgress: channel,
  })
}

export async function cancelTransfer(transferId: string): Promise<void> {
  if (!isDesktop) return
  try {
    await invoke('cancel_transfer', { transferId })
  } catch {
    // The transfer may have already finished — cancelling is best-effort.
  }
}

/* ------------------------------- plugins ------------------------------- */

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

export type PluginProgressStage = 'preparing' | 'downloading' | 'verifying' | 'installing'

export async function listDshPlugins(
  catalogBase: string,
  cacheDir?: string,
): Promise<RemotePluginMeta[]> {
  if (!isDesktop) return []
  return invoke<RemotePluginMeta[]>('list_dsh_plugins', {
    registryBase: catalogBase,
    cacheDir: cacheDir ?? null,
  })
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
  /** The instance profile directory owning `node_modules` + cordis.patch.yml. */
  instanceRoot: string
  onProgress: (event: {
    stage: PluginProgressStage
    progress: number
    bytesDone: number
    bytesPerSec: number
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
    instanceRoot: args.instanceRoot,
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
  instanceRoot: string,
  registryId: string,
  enabled: boolean,
): Promise<void> {
  if (!isDesktop) return
  await invoke('set_plugin_enabled', { instanceRoot, registryId, enabled })
}

export async function uninstallPlugin(instanceRoot: string, registryId: string): Promise<void> {
  if (!isDesktop) return
  await invoke('uninstall_plugin', { instanceRoot, registryId })
}

/* ------------------------------ runtimes ------------------------------ */

/** Mirrors the Rust `NodeRuntimeMeta` — one line per Node major. */
export interface RemoteRuntimeMeta {
  id: string
  major: number
  version: string
  codename?: string | null
  lts: boolean
}

export interface InstalledRuntimeInfo {
  name: string
  installedAt: string
  version: string
}

/**
 * The official dist index plus the npmmirror copy (same layout) are the two
 * sources; which one is passed is the user's download-source setting.
 */
export const NODE_DIST_OFFICIAL = 'https://nodejs.org/dist'
export const NODE_DIST_MIRROR = 'https://cdn.npmmirror.com/binaries/node'

export async function listNodeRuntimeCatalog(distBase: string): Promise<RemoteRuntimeMeta[]> {
  if (!isDesktop) return []
  return invoke('list_node_runtimes', { distBase })
}

export async function listInstalledRuntimes(root: string): Promise<InstalledRuntimeInfo[]> {
  if (!isDesktop) return []
  return invoke('list_installed_runtimes', { root })
}

/** `node --version` on PATH, or null when there is no usable answer. */
export async function systemNodeVersion(): Promise<string | null> {
  if (!isDesktop) return null
  try {
    return await invoke<string | null>('system_node_version')
  } catch {
    return null
  }
}

export interface DownloadRuntimeArgs {
  transferId: string
  distBase: string
  /** Install directory name under `<root>/runtimes/`, e.g. `node-22`. */
  versionName: string
  /** Full semver to fetch, e.g. `22.12.0`. */
  version: string
  root: string
  keepArchive: boolean
  onProgress: (event: {
    stage: 'downloading' | 'extracting' | 'verifying'
    progress: number
    bytesDone: number
    bytesPerSec: number
  }) => void
}

export async function downloadNodeRuntime(args: DownloadRuntimeArgs): Promise<void> {
  if (!isDesktop) throw new Error('Runtime 安装仅在桌面端可用')
  const channel = new Channel<Parameters<DownloadRuntimeArgs['onProgress']>[0]>()
  channel.onmessage = args.onProgress
  await invoke('download_node_runtime', {
    transferId: args.transferId,
    distBase: args.distBase,
    versionName: args.versionName,
    version: args.version,
    root: args.root,
    keepArchive: args.keepArchive,
    onProgress: channel,
  })
}

export async function removeRuntimeDir(root: string, runtimeName: string): Promise<void> {
  if (!isDesktop) return
  await invoke('remove_runtime_dir', { root, runtimeName })
}

/** Bytes on disk per installed runtime directory, keyed by its name. */
export async function runtimesDiskUsage(root: string): Promise<Record<string, number>> {
  if (!isDesktop) return {}
  return invoke('runtimes_disk_usage', { root })
}

/* ------------------------------- launch ------------------------------- */

/** Mirrors the Rust `LaunchEvent`; `stage` is a frontend `LaunchPhase`. */
export interface LaunchEventMsg {
  stage: string
  progress: number
  detail?: string | null
}

export interface LaunchInstanceArgs {
  transferId: string
  root: string
  instanceId: string
  /** Bare DSH version directory name (id minus the `dsh-` prefix). */
  versionName: string
  /** `node-<major>` or `node-system`. */
  runtimeName: string
  /** Registry the launcher pulls missing version deps from, if any. */
  registryBase: string
  profile: string
  port: number
  autoPort: boolean
  env: Record<string, string>
  args: string[]
  /**
   * The store's binding, authoritative over a stale on-disk manifest at
   * launch (binding persistence is fire-and-forget). Rust re-aligns it.
   */
  api?: ApiBinding | null
  onProgress: (event: LaunchEventMsg) => void
}

export interface LaunchResult {
  pid: number
  port: number
  /** Authenticated `dsh web` URL read from the launch log, when available. */
  webUrl?: string
}

export async function launchInstance(args: LaunchInstanceArgs): Promise<LaunchResult> {
  if (!isDesktop) throw new Error('启动仅在桌面端可用')
  const channel = new Channel<LaunchEventMsg>()
  channel.onmessage = args.onProgress
  return invoke('launch_instance', {
    transferId: args.transferId,
    root: args.root,
    instanceId: args.instanceId,
    versionName: args.versionName,
    runtimeName: args.runtimeName,
    registryBase: args.registryBase,
    profile: args.profile,
    port: args.port,
    autoPort: args.autoPort,
    env: args.env,
    args: args.args,
    api: args.api ?? null,
    onProgress: channel,
  })
}

export async function stopInstance(instanceId: string): Promise<void> {
  if (!isDesktop) return
  await invoke('stop_instance', { instanceId })
}

/** Cancels a launch still waiting for readiness; best-effort. */
export async function cancelLaunch(transferId: string): Promise<void> {
  if (!isDesktop) return
  try {
    await invoke('cancel_launch', { transferId })
  } catch {
    // The launch may have already finished.
  }
}

/**
 * A launched DSH process exited — crash, manual stop or clean shutdown. One
 * event drives all three, so the instance state never says "running" for a
 * dead process.
 */
export async function onInstanceExited(
  handler: (event: { instanceId: string; code: number | null }) => void,
): Promise<Unlisten> {
  if (!isDesktop) return () => {}
  return currentWindow().listen<{ instanceId: string; code: number | null }>(
    'phl://instance-exited',
    (event) => handler(event.payload),
  )
}

/* ---------------------------- diagnostics ----------------------------- */

/** Mirrors the Rust `DiagnosticItem`; `level` is `ok` | `warn` | `fail`. */
export interface DiagnosticItem {
  id: string
  level: 'ok' | 'warn' | 'fail'
  label: string
  detail: string
}

export interface DiagnosticReport {
  root: string
  items: DiagnosticItem[]
  cacheBytes: number
  cacheFiles: number
  generatedAt: string
}

/** In the browser there is nothing to inspect; callers show a notice. */
export async function runDiagnostics(root: string): Promise<DiagnosticReport | null> {
  if (!isDesktop) return null
  return invoke('run_diagnostics', { root })
}

/** Frees the download cache (`.part` 残留与保留的压缩包), returns bytes. */
export async function clearDownloadCache(root: string): Promise<number> {
  if (!isDesktop) return 0
  return invoke('clear_download_cache', { root })
}

/* ------------------------------- bundles ------------------------------ */

/** Native save dialog. Returns null in the browser or when cancelled. */
export async function chooseSaveFile(
  title: string,
  defaultPath: string,
  filters: { name: string; extensions: string[] }[],
): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { save } = await import('@tauri-apps/plugin-dialog')
    const picked = await save({ title, defaultPath, filters })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}

/** Native file-open dialog restricted to bundle files. */
export async function chooseBundleFile(): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const picked = await open({
      title: '选择 PHL Bundle 文件',
      multiple: false,
      directory: false,
      filters: [{ name: 'PHL Bundle', extensions: ['json'] }],
    })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}

export interface RemoteBundlePreview {
  name: string
  versionId: string
  runtimeId: string
  port: number
  pluginCount: number
  exportedAt: string
}

export async function exportInstanceBundle(root: string, id: string, dest: string): Promise<void> {
  if (!isDesktop) throw new Error('导出 Bundle 仅在桌面端可用')
  await invoke('export_instance_bundle', { root, id, dest })
}

export async function readInstanceBundle(path: string): Promise<RemoteBundlePreview> {
  if (!isDesktop) throw new Error('导入 Bundle 仅在桌面端可用')
  return invoke('read_instance_bundle', { path })
}

export async function importInstanceBundle(
  root: string,
  path: string,
  manifest: RemoteInstanceManifest,
): Promise<RemoteInstanceRecord> {
  if (!isDesktop) throw new Error('导入 Bundle 仅在桌面端可用')
  return invoke('import_instance_bundle', { root, path, manifest })
}

/* ------------------------------ snapshots ----------------------------- */

export async function createInstanceSnapshot(
  root: string,
  id: string,
  transferId: string,
  onProgress: (event: { progress: number; bytesDone: number; bytesTotal: number }) => void,
): Promise<RemoteSnapshotInfo> {
  if (!isDesktop) throw new Error('创建快照仅在桌面端可用')
  const channel = new Channel<Parameters<typeof onProgress>[0]>()
  channel.onmessage = onProgress
  return invoke('create_instance_snapshot', { root, id, transferId, onProgress: channel })
}

export async function restoreInstanceSnapshot(
  root: string,
  id: string,
  snapshotId: string,
): Promise<RemoteInstanceRecord> {
  if (!isDesktop) throw new Error('还原快照仅在桌面端可用')
  return invoke('restore_instance_snapshot', { root, id, snapshotId })
}

export async function deleteInstanceSnapshot(
  root: string,
  id: string,
  snapshotId: string,
): Promise<void> {
  if (!isDesktop) return
  await invoke('delete_instance_snapshot', { root, id, snapshotId })
}

/* ------------------------------ instances ----------------------------- */

/**
 * `instance.json` on disk, plus everything Rust derives from the directory:
 * the resolved paths and the plugin list read back out of `node_modules`.
 */
export interface RemoteInstanceRecord {
  id: string
  name: string
  note?: string | null
  kind: string
  hue: number
  versionId: string
  runtimeId: string
  port: number
  autoPort: boolean
  profile: string
  createdAt: string
  lastRunAt?: string | null
  totalRuntime: number
  favorite: boolean
  env: Record<string, string>
  args: string[]
  /** Mirrors the Rust `ApiBinding` on the manifest; null = unmanaged. */
  api?: ApiBinding | null
  dshHome: string
  workspace: string
  plugins: { pluginId: string; version: string; enabled: boolean; registryId: string }[]
  snapshots: RemoteSnapshotInfo[]
}

/** Mirrors the Rust `SnapshotFile` — the frontend `Snapshot` shape. */
export interface RemoteSnapshotInfo {
  id: string
  label: string
  createdAt: string
  versionId: string
  runtimeId: string
  pluginCount: number
  size: number
}

/** The manifest fields Rust persists — the record minus everything derived. */
export type RemoteInstanceManifest = Omit<
  RemoteInstanceRecord,
  'dshHome' | 'workspace' | 'plugins' | 'snapshots'
>

export async function listInstanceRecords(root: string): Promise<RemoteInstanceRecord[]> {
  if (!isDesktop) return []
  return invoke('list_instances', { root })
}

export async function createInstanceDir(
  root: string,
  manifest: RemoteInstanceManifest,
): Promise<RemoteInstanceRecord> {
  if (!isDesktop) throw new Error('创建实例目录仅在桌面端可用')
  return invoke('create_instance', { root, manifest })
}

export async function saveInstanceManifest(
  root: string,
  manifest: RemoteInstanceManifest,
): Promise<void> {
  if (!isDesktop) return
  await invoke('save_instance', { root, manifest })
}

export async function deleteInstanceDir(root: string, id: string): Promise<void> {
  if (!isDesktop) return
  await invoke('delete_instance', { root, id })
}

export interface CloneInstanceArgs {
  transferId: string
  root: string
  sourceId: string
  manifest: RemoteInstanceManifest
  onProgress: (event: { progress: number; bytesDone: number; bytesTotal: number }) => void
}

export async function cloneInstanceDir(
  args: CloneInstanceArgs,
): Promise<RemoteInstanceRecord> {
  if (!isDesktop) throw new Error('克隆实例仅在桌面端可用')
  const channel = new Channel<Parameters<CloneInstanceArgs['onProgress']>[0]>()
  channel.onmessage = args.onProgress
  return invoke('clone_instance', {
    transferId: args.transferId,
    root: args.root,
    sourceId: args.sourceId,
    manifest: args.manifest,
    onProgress: channel,
  })
}

export async function instanceDiskUsage(root: string, id: string): Promise<number> {
  if (!isDesktop) return 0
  return invoke('instance_disk_usage', { root, id })
}

/** Directories under `instances/` with no manifest — reclaimable leftovers. */
export async function scanOrphanInstances(
  root: string,
): Promise<{ name: string; size: number }[]> {
  if (!isDesktop) return []
  return invoke('scan_orphan_instances', { root })
}

export async function deleteOrphanInstance(root: string, name: string): Promise<void> {
  if (!isDesktop) return
  await invoke('delete_orphan_instance', { root, name })
}

/* ---------------------------- api config ------------------------------ */

/** The global provider library; `null` until the user creates one. */
export async function loadApiConfig(root: string): Promise<ApiConfig | null> {
  if (!isDesktop) return null
  return invoke('load_api_config', { root })
}

export async function saveApiConfig(root: string, config: ApiConfig): Promise<ApiConfig> {
  if (!isDesktop) throw new Error('API 配置库仅在桌面端可用')
  return invoke('save_api_config', { root, config })
}

/** Materialize a binding into the instance's dsh-home/settings.yaml. */
export async function syncInstanceApi(
  root: string,
  instanceId: string,
  binding: ApiBinding,
  config: ApiConfig,
): Promise<ApiBinding> {
  if (!isDesktop) throw new Error('API 配置同步仅在桌面端可用')
  return invoke('sync_instance_api', { root, instanceId, binding, config })
}

/** Read an instance's live settings.yaml back as a library seed. */
export async function importInstanceApi(
  root: string,
  instanceId: string,
): Promise<ApiConfig | null> {
  if (!isDesktop) return null
  return invoke('import_instance_api', { root, instanceId })
}

/**
 * Read one instance's live settings.yaml + how it compares to the library.
 * The returned `live` view is the truth for display; `localChanges` gates
 * overwrite confirmations, and `planAdoption` (client-side) drives 采纳.
 */
export async function instanceLiveSnapshot(
  root: string,
  instanceId: string,
  binding: ApiBinding,
  config: ApiConfig,
): Promise<InstanceLiveSnapshot> {
  if (!isDesktop) {
    return {
      inheritance: binding.inheritance,
      live: null,
      localChanges: false,
      defaultModelChanged: false,
      missingKeys: [],
    }
  }
  return invoke('instance_live_snapshot', { root, instanceId, binding, config })
}

/**
 * Reveals a folder in the system file manager. `null`/cancel is not a thing
 * here — a missing directory is the caller's data problem and rejects.
 */
export async function revealPath(path: string): Promise<void> {
  if (!isDesktop) return
  await invoke('reveal_path', { path })
}

/**
 * Opens an http(s) URL in the system browser. `<a target="_blank">` is a
 * no-op inside a Tauri window, so every external link must go through the
 * `open_external` command; in the browser it degrades to window.open.
 */
export async function openExternal(url: string): Promise<void> {
  if (!isDesktop) {
    window.open(url, '_blank', 'noopener')
    return
  }
  try {
    await invoke('open_external', { url })
  } catch (err) {
    console.warn('[phl] open_external failed:', err)
  }
}
