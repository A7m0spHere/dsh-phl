import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'

/**
 * Operation records bridge (R1 · M5). The journal only covers the R1 flows
 * and their launches; exported reports are scrubbed on the Rust side before
 * any credential-shaped value can reach the file.
 */

export interface OperationSummary {
  operationId: string
  kind: string
  sourceId: string
  targetId: string
  label: string
  status: 'running' | 'committed' | 'failed' | 'cancelled' | 'interrupted'
  startedAt: string
  finishedAt: string | null
  error: string | null
  detail: Record<string, unknown>
}

export interface ReportResult {
  jsonPath: string
  markdownPath: string
  sha256: string
}

export async function listOperations(
  instanceId: string | null,
  limit = 50,
): Promise<OperationSummary[] | null> {
  if (!isDesktop) return null
  return invoke('list_operations', { instanceId, limit })
}

export async function exportOperationReport(
  operationId: string,
  destination: string,
): Promise<ReportResult | null> {
  if (!isDesktop) return null
  return invoke('export_operation_report', { operationId, destination })
}
