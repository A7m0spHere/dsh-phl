import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { VersionSource } from '@/types'

/**
 * The DSH-version half of the desktop bridge plus the data-root handshake and
 * the shared transfer-cancel command (§M2 domain split). Moved out of
 * `desktop.ts` behind the compat re-export door so consumers are unchanged.
 * `cancelTransfer` is generic (any download/runtime/plugin/version transfer),
 * so it lives here and is re-exported alongside.
 */

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
  /** `healthy` | `degraded` (dependency pruning happened at install). */
  installHealth?: string
  skippedDependencies?: string[]
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

/**
 * Boot handshake with the backend's authoritative data root. The backend
 * adopts `hint` (the root persisted in settings) only when it has no pointer
 * file of its own yet — an existing install keeps its root through the
 * upgrade, while the pointer file wins everywhere else. Returns the
 * authoritative root, or null in the browser / on failure.
 */
export async function initPhlRoot(hint: string | null): Promise<string | null> {
  if (!isDesktop) return null
  try {
    return await invoke<string>('init_phl_root', { hint })
  } catch {
    return null
  }
}

/**
 * An explicit root change (first-run chooser, storage settings). The backend
 * persists the choice; returns the root it accepted, or null on failure.
 */
export async function setPhlRoot(root: string): Promise<string | null> {
  if (!isDesktop) return null
  try {
    return await invoke<string>('set_phl_root', { root })
  } catch {
    return null
  }
}

export async function listDshVersions(registryBase: string): Promise<RemoteVersionMeta[]> {
  if (!isDesktop) return []
  return invoke<RemoteVersionMeta[]>('list_dsh_versions', { registryBase })
}

export async function listInstalledVersions(): Promise<InstalledVersionInfo[]> {
  if (!isDesktop) return []
  return invoke<InstalledVersionInfo[]>('list_installed_versions', {})
}

export async function removeVersionDir(versionName: string): Promise<void> {
  if (!isDesktop) return
  await invoke('remove_version_dir', { versionName })
}

export interface DownloadArgs {
  transferId: string
  tarballUrl: string
  integrity?: string
  versionName: string
  /** npm registry base — used to install the package's own dependencies. */
  registryBase: string
  keepArchive: boolean
  /** Catalog size in bytes — the CDN often streams without content-length. */
  totalBytes?: number
  /** Exactly the Rust `ProgressEvent` wire shape (`rename_all = "camelCase"`):
   *  the npm dependency stage arrives as `installingDeps`, and the unit
   *  variants carry no numbers. */
  onProgress: (event: {
    stage: 'downloading' | 'extracting' | 'verifying' | 'installingDeps'
    progress?: number
    bytesDone?: number
    bytesPerSec?: number
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
