import { invoke, Channel } from '@tauri-apps/api/core'
import type { RemoteInstanceRecord } from './desktopInstances'

/**
 * Copy & trial upgrade bridge (R1 · M3). The preview is a backend-computed,
 * read-only plan; creation is cancellable through the shared transfer
 * registry, and the commit point is the staging rename — a cancel after it
 * reports "created", never a pure cancel.
 */

export type TrialScope = 'config' | 'config+sessions'
export type TrialWorkspace = 'fresh' | 'shared'

export interface TrialRequest {
  sourceId: string
  /** Bare DSH version, always explicit — the flow never falls back to latest. */
  targetVersion: string
  targetRuntimeId: string
  name: string
  scope: TrialScope
  workspace: TrialWorkspace
  /**
   * Plan identity issued by the preview. Create refuses a request without it
   * (C16/R3-01): the plan carries the source fingerprint and the copy's port,
   * and the backend re-verifies all three against the plan it issued.
   */
  planId: string
  targetId: string
  sourceFingerprint: string
}

export interface TrialPreview {
  sourceId: string
  sourceName: string
  sourceVersionId: string
  sourceRuntimeId: string
  targetVersion: string
  targetInstalled: boolean
  targetRuntimeId: string
  targetRuntimeInstalled: boolean
  suggestedName: string
  scope: string
  workspace: string
  estimatedBytes: number
  sessionCount: number | null
  sessionCwds: string[]
  pendingDownloads: string[]
  conflicts: string[]
  blocked: string[]
  /**
   * Real (non-link) profile packages whose version differs from the target
   * version tree's — the copy would carry the OLD build, and the new DSH may
   * refuse it at boot. Surfaced before the user commits.
   */
  mismatchedPackages: string[]
  /** Plan identity the create must send back verbatim. */
  planId: string
  targetId: string
  sourceFingerprint: string
  /** The copy's own port, allocated at plan time (CR-07). */
  allocatedPort: number
  autoPort: boolean
}

export interface TrialOutcome {
  record: RemoteInstanceRecord
  committed: boolean
  readiness: 'needsDependencies' | 'needsCredentials' | 'readyToLaunch'
  linkRedirects: number
  linkFailures: string[]
  /** Real profile packages carried at a version the target tree doesn't ship. */
  mismatchedPackages: string[]
  sessionsImported: number
  notes: string[]
}

export async function previewTrial(req: TrialRequest): Promise<TrialPreview> {
  return invoke('preview_trial', { req })
}

export async function createTrial(
  transferId: string,
  req: TrialRequest,
  onProgress: (p: { progress: number; bytesDone: number; bytesTotal: number }) => void,
): Promise<TrialOutcome> {
  const channel = new Channel<{ progress: number; bytesDone: number; bytesTotal: number }>()
  channel.onmessage = onProgress
  return invoke('create_trial', { transferId, req, onProgress: channel })
}
