import { create } from 'zustand'

/**
 * Open state for the trial-upgrade wizard (R1 · M3), same pattern as the
 * environment-compare store: one global mount, opened from the instance
 * menu, the detail page or a quick action.
 */
interface TrialWizardState {
  /** The instance whose environment is being copied. */
  sourceId: string | null
  /** Optional pre-picked target DSH version (bare semver). */
  presetVersion: string | null
  open: (sourceId: string, presetVersion?: string | null) => void
  close: () => void
}

export const useTrialWizardStore = create<TrialWizardState>((set) => ({
  sourceId: null,
  presetVersion: null,
  open: (sourceId, presetVersion = null) => set({ sourceId, presetVersion }),
  close: () => set({ sourceId: null, presetVersion: null }),
}))
