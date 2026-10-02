/**
 * The official DeepSeek Harness desktop app, seen by PHL as a machine
 * singleton it helps operate (planning input 2026-09-29 §8): discover,
 * show, launch, and quit — nothing more. Mirrors the Rust wire type pinned
 * by `official_desktop_wire_shape_is_stable` in `ipc_contract.rs`; optional
 * fields are the fail-closed channel (`running: undefined` reads as
 * 「无法确认」, never 「未运行」).
 */

export type OfficialDesktopStatus = 'not_installed' | 'unsupported' | 'installed' | 'unknown'

export interface OfficialDesktopInfo {
  status: OfficialDesktopStatus
  /** Install directory (Windows) or `.app` bundle (macOS). */
  root?: string
  mainExe?: string
  version?: string
  /** `undefined` = the probe could not be trusted; the UI must say so. */
  running?: boolean
  pid?: number
}

export type OfficialDesktopQuitOutcome = 'exited' | 'still_running'
