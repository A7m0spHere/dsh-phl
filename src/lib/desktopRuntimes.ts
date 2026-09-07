import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'

/**
 * The Node-runtime half of the desktop bridge (§M2 domain split): catalog
 * listing, installed-runtime discovery, system-node probe, download, removal,
 * and disk usage. Moved out of `desktop.ts` one domain at a time (the same way
 * Tasks / Pack / Sessions / Adoption already live in their own `desktop*`
 * modules); `desktop.ts` re-exports everything here so every existing
 * `from '@/lib/desktop'` import — the `desktop` object, command names, argument
 * shapes, the progress `Channel`, and the browser no-op degradation — is
 * unchanged.
 */

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

export async function listInstalledRuntimes(): Promise<InstalledRuntimeInfo[]> {
  if (!isDesktop) return []
  return invoke('list_installed_runtimes', {})
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
    keepArchive: args.keepArchive,
    onProgress: channel,
  })
}

export async function removeRuntimeDir(runtimeName: string): Promise<void> {
  if (!isDesktop) return
  await invoke('remove_runtime_dir', { runtimeName })
}

/** Bytes on disk per installed runtime directory, keyed by its name. */
export async function runtimesDiskUsage(): Promise<Record<string, number>> {
  if (!isDesktop) return {}
  return invoke('runtimes_disk_usage', {})
}
