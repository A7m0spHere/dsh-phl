import { create } from 'zustand'

/**
 * Open state for the environment-compare dialog (R1 · M2). A tiny module
 * store keeps the dialog mountable once at the app root while both the
 * instance menu (card / detail / quick switcher) and the trial-result page
 * can open it — left is the instance being inspected, right is an optional
 * pre-picked counterpart (the trial flow passes the copy's id).
 */
interface EnvCompareState {
  /** Instance whose environment is the left side of the comparison. */
  leftId: string | null
  /** Optional pre-selected right side (e.g. the freshly created copy). */
  rightId: string | null
  open: (leftId: string, rightId?: string | null) => void
  close: () => void
}

export const useEnvCompareStore = create<EnvCompareState>((set) => ({
  leftId: null,
  rightId: null,
  open: (leftId, rightId = null) => set({ leftId, rightId }),
  close: () => set({ leftId: null, rightId: null }),
}))
