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
 *
 * The window primitives live here for the same reason: the `desktop` window
 * controls (in `desktop.ts`) and the launch domain (which listens for window
 * events) both need `currentWindow`/`Unlisten`, so hoisting them out of the
 * parent keeps `desktopLaunch.ts` importable without a cycle back into
 * `desktop.ts`.
 */
import { getCurrentWindow } from '@tauri-apps/api/window'

export const isDesktop =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in (window as object)

/** A teardown handle returned by a Tauri event listener. */
export type Unlisten = () => void

/** No-op async — the browser-mode return for commands that do nothing headless. */
export const noop = async () => {}

/** The current app window (frameless; the app draws its own title bar). */
export function currentWindow() {
  return getCurrentWindow()
}
