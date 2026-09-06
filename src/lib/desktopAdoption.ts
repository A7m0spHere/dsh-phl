/**
 * Local DSH discovery + adoption bridge (desktop domain module, roadmap O-13).
 *
 * Two halves of one product story. `discovery/*` surfaces DSH environments
 * already on the machine; `preview_adoption`/`adopt_instance` fold one into a
 * PHL instance (development spec parts 1–2). All destructive decisions — what
 * is a DSH home, whether a home is already managed, what a copy moves — are
 * made in Rust; the WebView only ever names an instance id.
 *
 * In a plain browser every call degrades: discovery returns an empty list,
 * adoption throws. The wizard uses that to show a "desktop only" state rather
 * than pretending to scan.
 */
import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { RemoteAdoptedFrom, RemoteInstanceManifest, RemoteInstanceRecord } from './desktop'

/* ------------------------------ discovery ----------------------------- */

export type DiscoverySource = 'env' | 'defaultHome' | 'path' | 'manual'
export type DiscoveryConfidence = 'high' | 'medium' | 'low' | 'invalid'

/** Mirrors the Rust `ManagedInstance`. */
export interface RemoteManagedInstance {
  id: string
  name: string
}

/** Mirrors the Rust `DshCandidate` — one discovered DSH environment. */
export interface RemoteDshCandidate {
  id: string
  displayName: string
  dshHome: string
  executablePath: string | null
  detectedVersion: string | null
  nodePath: string | null
  nodeVersion: string | null
  profile: string
  pluginCount: number
  sessionCount: number
  sizeBytes: number
  source: DiscoverySource
  confidence: DiscoveryConfidence
  warnings: string[]
  alreadyManaged: boolean
  managedInstance: RemoteManagedInstance | null
}

/** Rescan the machine. Read-only backend-side; safe to call on every entry. */
export async function discoverDsh(): Promise<RemoteDshCandidate[]> {
  if (!isDesktop) return []
  return invoke<RemoteDshCandidate[]>('discover_dsh')
}

/** Inspect a manually chosen DSH_HOME. Throws when the pick is not a home. */
export async function inspectDshHome(path: string): Promise<RemoteDshCandidate> {
  if (!isDesktop) throw new Error('接入本机 DSH 仅在桌面端可用')
  return invoke<RemoteDshCandidate>('inspect_dsh_home', { path })
}

/** Inspect a manually chosen DSH executable; its home is inferred. */
export async function inspectDshExecutable(path: string): Promise<RemoteDshCandidate> {
  if (!isDesktop) throw new Error('接入本机 DSH 仅在桌面端可用')
  return invoke<RemoteDshCandidate>('inspect_dsh_executable', { path })
}

/* ------------------------------- adoption ----------------------------- */

export type AdoptionMode = 'managed-copy' | 'external'
/** `selected` migrates exactly the conversations named in `sessionDirs`. */
export type AdoptionSessionStrategy = 'all' | 'none' | 'selected'

/** Mirrors the Rust `AdoptionRequest`. */
export interface AdoptionRequest {
  sourceHome: string
  mode: AdoptionMode
  sessionStrategy: AdoptionSessionStrategy
  /** Encoded session dir names (`session-…`); only read when `selected`. */
  sessionDirs: string[]
  /** Identity + environment; Rust owns the management fields. */
  manifest: RemoteInstanceManifest
}

/** Mirrors the Rust `AdoptionPreview` — what the wizard shows before commit. */
export interface RemoteAdoptionPreview {
  sourceHome: string
  mode: AdoptionMode
  sessionStrategy: AdoptionSessionStrategy
  profile: string
  detectedVersion: string | null
  pluginCount: number
  sessionCount: number
  copyBytes: number
  symlinkEntries: number
  warnings: string[]
  keepsExistingSessions: boolean
  /** How many conversations would migrate under `selected` (else 0). */
  selectedSessionCount: number
}

export async function previewAdoption(req: AdoptionRequest): Promise<RemoteAdoptionPreview> {
  if (!isDesktop) throw new Error('接入本机 DSH 仅在桌面端可用')
  return invoke<RemoteAdoptionPreview>('preview_adoption', { req })
}

/** Mirrors the Rust `AdoptionOutcome`. */
export interface RemoteAdoptionOutcome {
  record: RemoteInstanceRecord
  adoptedFrom: RemoteAdoptedFrom
}

/**
 * Commit an adoption, streaming copy progress. `onProgress` receives bytes,
 * so the wizard can render a bar during the (potentially GB-scale) copy.
 */
export async function adoptInstance(
  req: AdoptionRequest,
  onProgress?: (p: { progress: number; bytesDone: number; bytesTotal: number }) => void,
): Promise<RemoteAdoptionOutcome> {
  if (!isDesktop) throw new Error('接入本机 DSH 仅在桌面端可用')
  const channel = new Channel<{ progress: number; bytesDone: number; bytesTotal: number }>()
  if (onProgress) channel.onmessage = onProgress
  return invoke<RemoteAdoptionOutcome>('adopt_instance', { req, onProgress: channel })
}

/** Count the DSH sessions an instance's home holds (data section, spec §27). */
export async function instanceSessionCount(id: string): Promise<number> {
  if (!isDesktop) return 0
  return invoke<number>('instance_session_count', { id })
}

/* -------------------------------- dialogs ----------------------------- */

/** Native directory picker (choosing a DSH_HOME). */
export async function chooseDshHome(): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const picked = await open({ title: '选择 DSH_HOME 目录', directory: true, multiple: false })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}

/** Native file picker (choosing a DSH executable). */
export async function chooseDshExecutable(): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const picked = await open({
      title: '选择 DSH 可执行文件',
      directory: false,
      multiple: false,
    })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}
