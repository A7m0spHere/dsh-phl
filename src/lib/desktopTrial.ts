import { invoke, Channel } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { RemoteInstanceRecord } from './desktopInstances'
import type { PluginEngineCompat } from '@/types'

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
  /**
   * Sessions that exist in the source but cannot be read (format too new,
   * corruption, IO failure), each entry carrying its reason (#6). The count
   * above only covers what a copy could carry — these are named so the
   * preview never misstates the source's data scope.
   */
  sessionsUnreadable: string[]
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
  /**
   * Per-plugin `engines.dsh` verdicts (issue #10) of the source profile's
   * installed plugins evaluated against the TARGET version — the official
   * compatibility claim, shown before the user commits.
   */
  pluginCompat: PluginEngineCompat[]
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

/** Mirrors the Rust `SessionSummary` (issue #8 report). */
export interface TrialSessionSummary {
  count: number
  unreadable: string[]
}

/** Mirrors the Rust `TrialUpgradeReport` (issue #8): one honest page about a
 *  trial copy — what is proven, what is merely an indicator, what was never
 *  verified. `overall` is `pass | partial | fail | unverified`. */
export interface TrialUpgradeReport {
  trialId: string
  sourceId: string | null
  planId: string | null
  targetVersion: string
  targetRuntimeId: string
  generatedAt: string
  overall: 'pass' | 'partial' | 'fail' | 'unverified'
  launchedEver: boolean
  webui: 'pass' | 'partial' | 'unverified'
  webuiDetail: string
  /** Token-redacted heuristic lines — an indicator for a human, not a diagnosis. */
  errorLines: string[]
  exitNote: string
  plugins: PluginEngineCompat[]
  linkFailures: string[]
  mismatchedPackages: string[]
  sessions: TrialSessionSummary | null
  sourceNote: string
  reportPath: string | null
}

export async function trialUpgradeReport(instanceId: string): Promise<TrialUpgradeReport> {
  if (!isDesktop) {
    throw new Error('升级报告仅在桌面应用内可用（浏览器模式没有真实副本）')
  }
  return invoke<TrialUpgradeReport>('trial_upgrade_report', { instanceId })
}
