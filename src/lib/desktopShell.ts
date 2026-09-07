import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'

/**
 * Generic shell helpers of the desktop bridge (§M2 domain split): the folder
 * picker, revealing a path in the file manager, and opening a URL in the system
 * browser. In the browser these degrade to no-op / `window.open`. Re-exported
 * from `desktop.ts` so consumers are unchanged.
 */

/**
 * Native folder picker. Returns `null` in the browser (and when the user
 * cancels), so callers must handle "no choice was made" either way.
 */
export async function chooseDirectory(defaultPath?: string): Promise<string | null> {
  if (!isDesktop) return null
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const picked = await open({
      directory: true,
      multiple: false,
      title: '选择 PHL 数据目录',
      defaultPath,
    })
    return typeof picked === 'string' ? picked : null
  } catch {
    return null
  }
}

/**
 * Reveals a folder in the system file manager. `null`/cancel is not a thing
 * here — a missing directory is the caller's data problem and rejects.
 */
export async function revealPath(path: string): Promise<void> {
  if (!isDesktop) return
  await invoke('reveal_path', { path })
}

/**
 * Opens an http(s) URL in the system browser. `<a target="_blank">` is a
 * no-op inside a Tauri window, so every external link must go through the
 * `open_external` command; in the browser it degrades to window.open.
 */
export async function openExternal(url: string): Promise<void> {
  if (!isDesktop) {
    window.open(url, '_blank', 'noopener')
    return
  }
  try {
    await invoke('open_external', { url })
  } catch (err) {
    console.warn('[phl] open_external failed:', err)
  }
}
