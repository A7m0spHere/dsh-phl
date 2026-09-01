import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'

/**
 * The seam between PHL's UI and the desktop shell.
 *
 * The app runs in a frameless Tauri window with its own title bar, so window
 * controls are the app's responsibility. Everything here degrades to a no-op
 * in a plain browser, which keeps `npm run dev` usable for pure UI work
 * without booting Rust.
 */

export const isDesktop =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in (window as object)

type Unlisten = () => void

const noop = async () => {}

function currentWindow() {
  return getCurrentWindow()
}

export const desktop = {
  isDesktop,

  /** Reveal the window once the first paint is done (it starts hidden). */
  async ready(): Promise<void> {
    if (!isDesktop) return
    try {
      await invoke('app_ready')
    } catch {
      // If the command is unavailable the Rust-side timeout still shows it.
    }
  },

  async minimize(): Promise<void> {
    if (!isDesktop) return noop()
    await currentWindow().minimize()
  },

  async toggleMaximize(): Promise<void> {
    if (!isDesktop) return noop()
    await currentWindow().toggleMaximize()
  },

  /**
   * Asks the window to close. Rust intercepts the request and hands it back
   * to the frontend through `onCloseRequested`, so the confirmation dialog is
   * the same whether the user clicks our button or the OS asks.
   */
  async requestClose(): Promise<void> {
    if (!isDesktop) return noop()
    await currentWindow().close()
  },

  /** Actually quit, after the user has confirmed. */
  async exit(): Promise<void> {
    if (!isDesktop) return noop()
    await invoke('exit_app')
  },

  async isMaximized(): Promise<boolean> {
    if (!isDesktop) return false
    try {
      return await currentWindow().isMaximized()
    } catch {
      return false
    }
  },

  /** Fires whenever the window is resized, which covers maximize/restore. */
  async onResized(handler: () => void): Promise<Unlisten> {
    if (!isDesktop) return () => {}
    return currentWindow().onResized(handler)
  },

  async onCloseRequested(handler: () => void): Promise<Unlisten> {
    if (!isDesktop) return () => {}
    return currentWindow().listen('phl://close-requested', () => handler())
  },
}

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
