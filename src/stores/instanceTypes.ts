import type { InstanceState } from './instanceStore'
import type { WebUiOpenResult } from '@/lib/desktop'

export interface InstanceProcessControls {
  restarting: Record<string, true>
  stoppingAll: boolean
  launch: (id: string, options?: { autoOpenWebUi?: boolean }) => Promise<void>
  cancelLaunch: (id: string) => void
  stop: (id: string) => Promise<boolean>
  restart: (id: string) => Promise<boolean>
  /** Returns instance IDs whose processes could not be confirmed stopped. */
  stopAll: () => Promise<string[]>
  toggle: (id: string) => void
  openWebUi: (id: string) => Promise<WebUiOpenResult>
}

/**
 * The zustand `set`/`get` pair the instance domain-action creators compose
 * against. Lives in its own module (not `instanceStore.ts`) so a split-out
 * action file can type itself without importing the store that imports it —
 * the type-only cycle would be erased at runtime, but keeping the seam here
 * means neither direction needs a runtime edge for typing.
 */
export type InstanceSet = (
  partial: Partial<InstanceState> | ((state: InstanceState) => Partial<InstanceState>),
) => void
export type InstanceGet = () => InstanceState
