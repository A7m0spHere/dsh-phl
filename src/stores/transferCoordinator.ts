import { useSettingsStore } from './settingsStore'

/**
 * The single global transfer-concurrency budget (§M3, roadmap O-10).
 *
 * The 「同时下载数」 setting is enforced for real across *all* long-running
 * downloads/installs — DSH versions, runtimes and plugin installs compete for
 * one shared set of slots rather than each page self-limiting (the spec's
 * "共用全局并发预算"). It lives as a module singleton, not per-store, precisely
 * so there is ONE queue and ONE budget; the catalog store composes it, it does
 * not duplicate it.
 *
 * A new transfer must own a slot before touching the network; the rest sit in
 * `queued` state and start as a predecessor finishes or is cancelled. Cancelling
 * a still-queued transfer is what `acquireTransferSlot` returning `false` means:
 * the operation never started.
 */

const activeSlots = new Set<string>()
const slotWaiters: Array<{ key: string; start: () => void }> = []

function slotLimit(): number {
  const raw = useSettingsStore.getState().concurrency
  return Math.max(1, Math.floor(Number.isFinite(raw) ? raw : 2) || 1)
}

function pumpSlots(): void {
  while (activeSlots.size < slotLimit() && slotWaiters.length > 0) {
    slotWaiters.shift()!.start()
  }
}

/**
 * Wait for a slot. `false` means the caller must treat the operation as
 * cancelled — the user aborted while queued (the transfer itself never started,
 * which is exactly why cancel-before-start works).
 */
export async function acquireTransferSlot(key: string, signal: AbortSignal): Promise<boolean> {
  if (signal.aborted) return Promise.resolve(false)
  if (activeSlots.size < slotLimit() && !activeSlots.has(key)) {
    activeSlots.add(key)
    return Promise.resolve(true)
  }
  return new Promise<boolean>((resolve) => {
    const dequeue = () => {
      const i = slotWaiters.findIndex((w) => w.key === key && w.start === waiter.start)
      if (i >= 0) slotWaiters.splice(i, 1)
      resolve(false)
    }
    const waiter = {
      key,
      start: () => {
        signal.removeEventListener('abort', dequeue)
        if (signal.aborted) {
          resolve(false)
          return
        }
        activeSlots.add(key)
        resolve(true)
      },
    }
    signal.addEventListener('abort', dequeue, { once: true })
    slotWaiters.push(waiter)
  })
}

export function releaseTransferSlot(key: string): void {
  activeSlots.delete(key)
  pumpSlots()
}
