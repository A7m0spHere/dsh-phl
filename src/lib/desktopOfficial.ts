import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { OfficialDesktopInfo, OfficialDesktopQuitOutcome } from '@/types'

export type { OfficialDesktopInfo, OfficialDesktopQuitOutcome }

/**
 * Desktop bridge for the official DeepSeek Harness desktop singleton
 * (`src-tauri/src/official/`): inspect / launch / quit. Every call re-probes
 * the machine live — the official app self-updates on the nightly channel,
 * so the backend caches nothing and neither may the frontend.
 *
 * Browser mode degrades honestly: inspect returns `null` (the card shows a
 * desktop-only notice) and the actions reject instead of pretending. Backend
 * failures propagate to the store's error state, which shows 「无法确认」 —
 * a failed probe must never read as "not installed".
 */

export async function inspectOfficialDesktop(): Promise<OfficialDesktopInfo | null> {
  if (!isDesktop) return null
  return invoke<OfficialDesktopInfo>('inspect_official_desktop')
}

export async function launchOfficialDesktop(): Promise<void> {
  if (!isDesktop) throw new Error('官方桌面端管理仅在桌面应用内可用')
  await invoke('launch_official_desktop')
}

export async function quitOfficialDesktop(force: boolean): Promise<OfficialDesktopQuitOutcome> {
  if (!isDesktop) throw new Error('官方桌面端管理仅在桌面应用内可用')
  return invoke<OfficialDesktopQuitOutcome>('quit_official_desktop', { force })
}
