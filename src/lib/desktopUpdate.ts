import { isDesktop, isWindows } from './desktopCore'
import {
  UPDATE_MANIFEST_URL,
  isNewerVersion,
  parseUpdateManifest,
} from './updateManifest'

/**
 * In-app updates (roadmap T-208). The release pipeline publishes a signed
 * manifest; the updater verifies its minisign signature against the public key
 * in tauri.conf.json before anything is downloaded, so a compromised mirror
 * cannot push code. Desktop only - the browser build has no updater and every
 * call degrades to no update.
 *
 * The manifest is Windows-only: macOS ships a manually installed DMG and no
 * in-app updater chain yet. On those platforms PHL reads the same manifest
 * itself and returns a `manual` result — the settings panel then points at the
 * release page instead of offering an install button.
 */

export interface AppUpdateInfo {
  version: string
  currentVersion: string
  date?: string
  notes?: string
  /**
   * True when this platform has no in-app install chain: the caller must offer
   * a manual download instead of `installAppUpdate()`.
   */
  manual?: boolean
}

export type UpdateMode = 'in-app' | 'manual'

/**
 * Windows is the only platform with a published updater chain today. The
 * manifest has a single `windows-x86_64` entry; `tauri-plugin-updater` treats
 * a missing platform key as an error before it compares versions, so any other
 * platform must not call it.
 */
export function updateMode(): UpdateMode {
  return isWindows ? 'in-app' : 'manual'
}

export type UpdateProgress =
  | { kind: 'started'; total: number | null }
  | { kind: 'progress'; downloaded: number; total: number | null }
  | { kind: 'finished' }

type DownloadEvent = {
  event: 'Started' | 'Progress' | 'Finished'
  data?: { contentLength?: number; chunkLength?: number }
}

/**
 * The handle returned by check(). It must survive between the check and the
 * install call: the plugin downloads exactly the manifest it verified, so
 * re-checking in between would act on a different object.
 */
type PendingUpdate = {
  version: string
  downloadAndInstall: (onEvent: (event: DownloadEvent) => void) => Promise<void>
}

let pending: PendingUpdate | null = null

export function appVersion(): string {
  return typeof __PHL_VERSION__ === 'string' ? __PHL_VERSION__ : ''
}

/**
 * Ask the manifest for a newer version. Returns null when the app is current,
 * and throws with a readable message when the manifest is unreachable or
 * malformed.
 *
 * Windows uses the updater plugin end to end. Other platforms never call it:
 * the plugin resolves the current platform's download entry before deciding
 * whether an update exists, so a platform missing from the Windows-only
 * manifest fails even when the app is already current. They read the manifest
 * directly instead and mark the result `manual`.
 */
export async function checkAppUpdate(): Promise<AppUpdateInfo | null> {
  if (!isDesktop) return null
  if (!isWindows) return checkManifestForManualUpdate()
  const { check } = await import('@tauri-apps/plugin-updater')
  const update = await check()
  if (!update) {
    pending = null
    return null
  }
  pending = update as unknown as PendingUpdate
  return {
    version: update.version,
    currentVersion: update.currentVersion,
    date: update.date ?? undefined,
    notes: update.body ?? undefined,
  }
}

/**
 * Manual-notice path. Reads the same manifest but never downloads: only the
 * version and notes are used to tell the user a newer build exists, so no
 * signature check is needed (and none is possible — there is no artifact).
 */
async function checkManifestForManualUpdate(): Promise<AppUpdateInfo | null> {
  const currentVersion = appVersion()
  let response: Response
  try {
    response = await fetch(UPDATE_MANIFEST_URL, { cache: 'no-store' })
  } catch (err) {
    throw new Error('无法读取更新清单：' + describeError(err))
  }
  if (!response.ok) {
    throw new Error('无法读取更新清单：HTTP ' + response.status)
  }
  let payload: unknown
  try {
    payload = await response.json()
  } catch {
    throw new Error('更新清单不是有效的 JSON')
  }
  const manifest = parseUpdateManifest(payload)
  pending = null
  if (!currentVersion || !isNewerVersion(manifest.version, currentVersion)) {
    return null
  }
  return {
    version: manifest.version,
    currentVersion,
    date: manifest.date,
    notes: manifest.notes,
    manual: true,
  }
}

function describeError(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

/**
 * Download and install the update found by checkAppUpdate().
 *
 * Windows only: the other platforms have no in-app install chain, and the
 * manual result from `checkAppUpdate()` deliberately carries no update handle.
 * On Windows the installer runs and the app exits, so this promise normally
 * never resolves - the process is gone before it can. Callers must not treat a
 * missing resolution as a failure.
 */
export async function installAppUpdate(
  onProgress: (progress: UpdateProgress) => void,
): Promise<void> {
  if (!isDesktop) throw new Error('应用更新仅在桌面端可用')
  if (updateMode() === 'manual') {
    throw new Error('当前平台暂不支持应用内更新，请手动下载安装包')
  }
  if (!pending) throw new Error('没有可安装的更新，请先检查更新')
  let total: number | null = null
  let downloaded = 0
  await pending.downloadAndInstall((event) => {
    if (event.event === 'Started') {
      total = event.data?.contentLength ?? null
      onProgress({ kind: 'started', total })
    } else if (event.event === 'Progress') {
      downloaded += event.data?.chunkLength ?? 0
      onProgress({ kind: 'progress', downloaded, total })
    } else {
      onProgress({ kind: 'finished' })
    }
  })
}

export function clearPendingUpdate() {
  pending = null
}
