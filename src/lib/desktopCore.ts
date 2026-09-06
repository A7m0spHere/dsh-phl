/**
 * Shared core of the Tauri platform bridges.
 *
 * `desktop.ts` grew into one file for every IPC domain (roadmap O-13). New
 * domain modules — starting with `desktopTasks.ts` — import the environment
 * probe from here instead of importing `desktop.ts` itself, which would
 * cycle once the parent re-exports the domain. The rule for the split:
 * one user-facing interaction per move, behaviour and error paths preserved,
 * consumers re-exported from `desktop.ts` so nothing else has to learn the
 * new file names yet.
 */
export const isDesktop =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in (window as object)
