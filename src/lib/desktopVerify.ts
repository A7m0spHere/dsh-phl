import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'

/**
 * Environment health of the desktop bridge (§M2 domain split): read-only
 * verification of an instance and execution of the local-only repairs. Re-
 * exported from `desktop.ts` so consumers are unchanged.
 */

/** Mirrors the Rust `VerifyCheck` — one item of the environment report. */
export interface RemoteVerifyCheck {
  id: string
  category: string
  severity: 'info' | 'warning' | 'error'
  status: 'pass' | 'warn' | 'fail'
  message: string
  repairable: boolean
  repairAction?: string | null
}

/** Mirrors the Rust `VerifyResult` — the instance's Environment Health. */
export interface RemoteVerifyResult {
  overall: 'healthy' | 'degraded' | 'broken'
  checks: RemoteVerifyCheck[]
}

/**
 * Read-only environment verification: inspects the manifest, the pinned DSH
 * version tree, config files, API references and launch conditions without
 * modifying anything.
 */
export async function verifyInstance(instanceId: string): Promise<RemoteVerifyResult> {
  if (!isDesktop) {
    return { overall: 'healthy', checks: [] }
  }
  return invoke<RemoteVerifyResult>('verify_instance', { instanceId })
}

/** Mirrors the Rust `RepairOutcome`. */
export interface RepairOutcome {
  applied: string[]
  /** Recognized but needing a download — routed to the install flows. */
  requiresUser: string[]
  rejected: string[]
}

/**
 * Executes the local-only repairs (recreate workspace, sweep transaction
 * leftovers) for an instance; download-needing actions come back in
 * `requiresUser`. The caller re-runs verify afterwards.
 */
export async function repairInstance(
  instanceId: string,
  actions: string[],
): Promise<RepairOutcome> {
  if (!isDesktop) return { applied: [], requiresUser: [], rejected: actions }
  return invoke<RepairOutcome>('repair_instance', { instanceId, actions })
}
