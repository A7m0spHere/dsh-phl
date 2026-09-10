import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { ApiBinding } from '@/types'

/**
 * The instance-lifecycle half of the desktop bridge (§M2 domain split): the
 * record type Rust derives on disk, create/save/delete/clone/disk-usage/orphan
 * commands, and the snapshot commands that read/write that same record. Re-
 * exported from `desktop.ts`; `desktopBundles.ts` imports the record/manifest
 * types from here (a one-way dependency — instances never import bundles).
 */

/** Provenance recorded when an instance was adopted from a local DSH. */
export interface RemoteAdoptedFrom {
  dshHome: string
  detectedVersion?: string | null
  adoptedAt: string
  /** The management mode chosen at adoption: 'managed-copy' | 'external'. */
  mode: string
}

/**
 * `instance.json` on disk, plus everything Rust derives from the directory:
 * the resolved paths and the plugin list read back out of `node_modules`.
 */
export interface RemoteInstanceRecord {
  /** On-disk manifest format version. Stamped by Rust — the frontend never sends it. */
  schemaVersion?: number
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
  /**
   * Adoption fields (schema v2). All optional on the wire: a v1 manifest has
   * none and Rust reads it as `managed-copy`/`created`. But `save_instance`
   * round-trips the *whole* manifest, so once Rust hands these back the
   * frontend must echo them unchanged — dropping them would let serde
   * defaults reset an `external` instance to `managed-copy` and orphan its
   * real DSH_HOME. That is why `toManifest` carries them.
   */
  managementMode?: 'managed-copy' | 'external' | 'pack-installed'
  source?: 'created' | 'adopted' | 'phlpack'
  externalHome?: string | null
  adoptedFrom?: RemoteAdoptedFrom | null
  dshHome: string
  workspace: string
  plugins: {
    pluginId: string
    version: string
    enabled: boolean
    registryId: string
    /** verified | pinned | unverified | unknown (T-107). */
    trust?: string
  }[]
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

/**
 * Instance commands take only stable ids — the data root is resolved
 * Rust-side from `PhlState`, so the WebView can never aim a destructive
 * operation outside the real root.
 */
export async function listInstanceRecords(): Promise<RemoteInstanceRecord[]> {
  if (!isDesktop) return []
  return invoke('list_instances', {})
}

export async function createInstanceDir(
  manifest: RemoteInstanceManifest,
): Promise<RemoteInstanceRecord> {
  if (!isDesktop) throw new Error('创建实例目录仅在桌面端可用')
  return invoke('create_instance', { manifest })
}

export async function saveInstanceManifest(manifest: RemoteInstanceManifest): Promise<void> {
  if (!isDesktop) return
  await invoke('save_instance', { manifest })
}

export async function deleteInstanceDir(id: string): Promise<void> {
  if (!isDesktop) return
  await invoke('delete_instance', { id })
}

export interface CloneInstanceArgs {
  transferId: string
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
    sourceId: args.sourceId,
    manifest: args.manifest,
    onProgress: channel,
  })
}

export async function instanceDiskUsage(id: string): Promise<number> {
  if (!isDesktop) return 0
  return invoke('instance_disk_usage', { id })
}

/** Directories under `instances/` with no manifest — reclaimable leftovers. */
export async function scanOrphanInstances(): Promise<{ name: string; size: number }[]> {
  if (!isDesktop) return []
  return invoke('scan_orphan_instances', {})
}

export async function deleteOrphanInstance(name: string): Promise<void> {
  if (!isDesktop) return
  await invoke('delete_orphan_instance', { name })
}

/* ------------------------------ snapshots ----------------------------- */

export async function createInstanceSnapshot(
  id: string,
  transferId: string,
  onProgress: (event: { progress: number; bytesDone: number; bytesTotal: number }) => void,
): Promise<RemoteSnapshotInfo> {
  if (!isDesktop) throw new Error('创建快照仅在桌面端可用')
  const channel = new Channel<Parameters<typeof onProgress>[0]>()
  channel.onmessage = onProgress
  return invoke('create_instance_snapshot', { id, transferId, onProgress: channel })
}

export async function restoreInstanceSnapshot(
  id: string,
  transferId: string,
  snapshotId: string,
  onProgress: (event: { progress: number; bytesDone: number; bytesTotal: number }) => void,
): Promise<RemoteInstanceRecord> {
  if (!isDesktop) throw new Error('还原快照仅在桌面端可用')
  const channel = new Channel<Parameters<typeof onProgress>[0]>()
  channel.onmessage = onProgress
  return invoke('restore_instance_snapshot', { id, transferId, snapshotId, onProgress: channel })
}

export async function deleteInstanceSnapshot(id: string, snapshotId: string): Promise<void> {
  if (!isDesktop) return
  await invoke('delete_instance_snapshot', { id, snapshotId })
}
