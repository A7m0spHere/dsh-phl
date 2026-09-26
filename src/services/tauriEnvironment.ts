import * as desktop from '@/lib/desktop'
import type { EnvironmentDiff, EnvironmentFacts } from '@/lib/desktop'
import type { TrialOutcome, TrialPreview, TrialRequest } from '@/lib/desktop'
import { Cancelled, newTransferId, type TransferProgress } from './repository'
import type { PhlRepository } from './repository'

/**
 * Desktop overrides for the **environment facts** module (R1 · M2). Both
 * calls are backend-side reads; the browser mock returns null so callers can
 * show an explicit desktop-only notice instead of pretending to compare.
 */

async function inspectEnvironment(instanceId: string): Promise<EnvironmentFacts | null> {
  return desktop.inspectEnvironment(instanceId)
}

async function compareEnvironments(leftId: string, rightId: string): Promise<EnvironmentDiff | null> {
  return desktop.compareEnvironments(leftId, rightId)
}

async function previewTrial(req: TrialRequest): Promise<TrialPreview | null> {
  return desktop.previewTrial(req)
}

async function createTrial(
  req: TrialRequest,
  onProgress: (p: { progress: number; bytesDone: number; bytesTotal: number }) => void,
  signal: AbortSignal,
): Promise<TrialOutcome | null> {
  const transferId = newTransferId(`trial:${req.sourceId}`)
  if (signal.aborted) throw new Cancelled()
  const onAbort = () => void desktop.cancelTransfer(transferId)
  signal.addEventListener('abort', onAbort, { once: true })
  try {
    return await desktop.createTrial(transferId, req, onProgress)
  } catch (err) {
    if (signal.aborted) throw new Cancelled()
    throw err instanceof Error ? err : new Error(String(err))
  } finally {
    signal.removeEventListener('abort', onAbort)
  }
}

async function preparePackDependencies(
  instanceId: string,
  onProgress: (p: TransferProgress) => void,
  signal: AbortSignal,
): Promise<{ readiness: string; dependenciesInstalled: number; dependencyFailures: string[] } | null> {
  const outcome = await desktop.preparePackDependencies(
    instanceId,
    (event) =>
      onProgress({
        stage: 'installingDeps',
        progress: event.progress,
        bytesDone: event.bytesDone,
      }),
    signal,
  )
  if (!outcome) return null
  return {
    readiness: outcome.readiness,
    dependenciesInstalled: outcome.dependenciesInstalled,
    dependencyFailures: outcome.dependencyFailures,
  }
}

export const tauriEnvironmentOverrides: Pick<
  PhlRepository,
  | 'inspectEnvironment'
  | 'compareEnvironments'
  | 'previewTrial'
  | 'createTrial'
  | 'preparePackDependencies'
> = {
  inspectEnvironment,
  compareEnvironments,
  previewTrial,
  createTrial,
  preparePackDependencies,
}
