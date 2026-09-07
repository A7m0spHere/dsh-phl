import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'

/**
 * The diagnostics half of the desktop bridge (§M2 domain split). Moved out of
 * `desktop.ts` one cohesive domain at a time (Tasks / Pack / Sessions / Adoption /
 * Runtimes precede it); `desktop.ts` re-exports everything here so all
 * `from '@/lib/desktop'` imports are unchanged.
 */

/** Mirrors the Rust `DiagnosticItem`; `level` is `ok` | `warn` | `fail`. */
export interface DiagnosticItem {
  id: string
  level: 'ok' | 'warn' | 'fail'
  label: string
  detail: string
}

export interface DiagnosticReport {
  root: string
  items: DiagnosticItem[]
  cacheBytes: number
  cacheFiles: number
  generatedAt: string
}

/** In the browser there is nothing to inspect; callers show a notice. */
export async function runDiagnostics(): Promise<DiagnosticReport | null> {
  if (!isDesktop) return null
  return invoke('run_diagnostics', {})
}

/** Frees the download cache (`.part` 残留与保留的压缩包), returns bytes. */
export async function clearDownloadCache(): Promise<number> {
  if (!isDesktop) return 0
  return invoke('clear_download_cache', {})
}
