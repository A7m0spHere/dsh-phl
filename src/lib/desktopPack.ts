/**
 * `.phlpack` export/install bridge (domain module, roadmap O-13).
 *
 * Mirrors the Rust `pack` command surface (P1-2..P1-4). Bundles and packs both
 * bypass the repository boundary and call `invoke` directly (see the bundle
 * precedent in `desktop.ts`): they are desktop-file features whose source of
 * truth is the filesystem, not the instance store. Browser mode degrades —
 * `previewPack`/`installPack` throw, the export helpers return null — so the
 * wizard can show a desktop-only notice rather than faking a transfer.
 */
import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import { cancelTransfer } from './desktopVersions'
import type { RemoteInstanceManifest } from './desktop'

let packTransferSequence = 0
function packTransferId(kind: 'export' | 'install'): string {
  packTransferSequence += 1
  return `pack-${kind}:${Date.now()}:${packTransferSequence}`
}

async function invokeCancellable<T>(
  command: string,
  args: Record<string, unknown>,
  kind: 'export' | 'install',
  signal?: AbortSignal,
): Promise<T> {
  const transferId = packTransferId(kind)
  const abort = () => void cancelTransfer(transferId)
  signal?.addEventListener('abort', abort, { once: true })
  if (signal?.aborted) abort()
  try {
    return await invoke<T>(command, { ...args, transferId })
  } finally {
    signal?.removeEventListener('abort', abort)
  }
}

/* ------------------------------- export ------------------------------- */

/** Mirrors the Rust `ExportPluginPlan`. */
export interface RemoteExportPluginPlan {
  pluginId: string
  version: string
  registryId: string
  license: string | null
  licenseUnknown: boolean
  registryAvailable: boolean
  embedRecommended: boolean
}

/** Mirrors the Rust `PackExportPlan`. */
export interface RemotePackExportPlan {
  instanceId: string
  name: string
  versionId: string
  runtimeId: string
  profile: string
  plugins: RemoteExportPluginPlan[]
  sessionCount: number
  credentialNames: string[]
  machineOnly: string[]
  estimatedBytes: number
  /** §12 secret-named files found in plugin dirs at preview; never packed. */
  secretFiles: string[]
  warnings: string[]
}

/** The user's confirmed export selection (mirrors `PackExportOptions`). */
export interface PackExportOptions {
  embedRegistryIds: string[]
  includeSessions: boolean
  sessionsPrivacyAck: boolean
}

/** Mirrors the Rust `PackExportReport`. */
export interface RemotePackExportReport {
  packPath: string
  pluginCount: number
  embeddedCount: number
  sessionsIncluded: boolean
  credentialNames: string[]
  /** Secret-named files the core tree-walker withheld (empty for a clean pack). */
  secretFilesWithheld: string[]
  /** Filesystem links the walker skipped (a pack carries bytes, never machine
   * paths). Managed `versions/` links rebuild themselves on the target at
   * install; the list is informational transparency, not an error. */
  linksSkipped: string[]
}

export async function previewInstancePackExport(id: string): Promise<RemotePackExportPlan> {
  if (!isDesktop) throw new Error('导出整合包仅在桌面端可用')
  return invoke<RemotePackExportPlan>('preview_instance_pack_export', { id })
}

export async function exportInstancePack(
  id: string,
  dest: string,
  options: PackExportOptions,
  signal?: AbortSignal,
): Promise<RemotePackExportReport> {
  if (!isDesktop) throw new Error('导出整合包仅在桌面端可用')
  return invokeCancellable<RemotePackExportReport>('export_instance_pack', { id, dest, options }, 'export', signal)
}

/* ------------------------------- install ------------------------------ */

/** Mirrors the Rust `DependencyStatus`. */
export type PackDependencyStatus = 'installed' | 'downloadable' | 'embedded' | 'missing'

/** Mirrors the Rust `Dependency`. */
export interface RemotePackDependency {
  kind: string
  id: string
  version: string
  status: PackDependencyStatus
  required: boolean
  note: string | null
}

/** Mirrors the Rust `PackPreview`. */
export interface RemotePackPreview {
  packId: string
  name: string
  version: string
  author: string
  description: string
  icon: string | null
  dshVersion: string
  runtime: string
  pluginCount: number
  embeddedPluginCount: number
  sessionsIncluded: boolean
  sessionCount: number
  secretsExcluded: boolean
  dependencies: RemotePackDependency[]
  warnings: string[]
  blocked: boolean
}

/** Mirrors the Rust `PackInstallRequest`. */
export interface PackInstallRequest {
  manifest: RemoteInstanceManifest
  allowMissing: boolean
}

/** Mirrors the Rust `PackInstallOutcome`. */
export interface RemotePackInstallOutcome {
  record: import('./desktop').RemoteInstanceRecord
  deferredPlugins: string[]
  sessionsImported: number
  credentialNames: string[]
}

export async function previewPack(path: string): Promise<RemotePackPreview> {
  if (!isDesktop) throw new Error('整合包安装仅在桌面端可用')
  return invoke<RemotePackPreview>('preview_pack', { path })
}

export async function installPack(
  path: string,
  req: PackInstallRequest,
  onProgress?: (p: { progress: number; bytesDone: number; bytesTotal: number }) => void,
  signal?: AbortSignal,
): Promise<RemotePackInstallOutcome> {
  if (!isDesktop) throw new Error('整合包安装仅在桌面端可用')
  const channel = new Channel<{ progress: number; bytesDone: number; bytesTotal: number }>()
  if (onProgress) channel.onmessage = onProgress
  return invokeCancellable<RemotePackInstallOutcome>(
    'install_pack',
    { path, req, onProgress: channel },
    'install',
    signal,
  )
}

/* -------------------------------- dialogs ----------------------------- */

/** Native save dialog for a `.phlpack`. */
export async function choosePackSavePath(defaultName: string): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { save } = await import('@tauri-apps/plugin-dialog')
    const picked = await save({
      title: '导出整合包',
      defaultPath: defaultName.endsWith('.phlpack') ? defaultName : `${defaultName}.phlpack`,
      filters: [{ name: 'PHL 整合包', extensions: ['phlpack'] }],
    })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}

/** Native open dialog restricted to `.phlpack`. */
export async function choosePackOpenPath(): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const picked = await open({
      title: '选择 PHL 整合包',
      multiple: false,
      directory: false,
      filters: [{ name: 'PHL 整合包', extensions: ['phlpack'] }],
    })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}
