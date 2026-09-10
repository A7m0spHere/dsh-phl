import { isDesktop } from './desktopCore'

/**
 * In-app updates (roadmap T-208). The release pipeline publishes a signed
 * manifest; the updater verifies its minisign signature against the public key
 * in tauri.conf.json before anything is downloaded, so a compromised mirror
 * cannot push code. Desktop only - the browser build has no updater and every
 * call degrades to no update.
 */

export interface AppUpdateInfo {
  version: string
  currentVersion: string
  date?: string
  notes?: string
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
 * and throws with the backend's message when the manifest is unreachable or
 * its signature does not verify.
 */
export async function checkAppUpdate(): Promise<AppUpdateInfo | null> {
  if (!isDesktop) return null
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
 * Download and install the update found by checkAppUpdate().
 *
 * On Windows the installer runs and the app exits, so this promise normally
 * never resolves - the process is gone before it can. Callers must not treat a
 * missing resolution as a failure.
 */
export async function installAppUpdate(
  onProgress: (progress: UpdateProgress) => void,
): Promise<void> {
  if (!isDesktop) throw new Error('应用更新仅在桌面端可用')
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
