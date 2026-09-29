import { useEffect, useState } from 'react'
import { desktop, isMac } from '@/lib/desktop'

/**
 * Tracks the window's fullscreen state and binds the toggle shortcut.
 *
 * The header's 退出全屏 button is the only guaranteed way out on macOS: the
 * native lights auto-hide in fullscreen and the Overlay titlebar does not
 * reveal them on hover. Tauri's `isFullscreen` reads tao's window state, which
 * syncs on the native green-button transitions too (`windowWillEnterFullScreen`
 * updates it), so one probe covers programmatic and native entry alike.
 *
 * The keybinding mirrors the system convention — ⌃⌘F on macOS (a plain ⌃F is
 * "move a word left" in text fields, and a bare system ⌃F would collide with
 * nothing here anyway), Ctrl+Alt+F elsewhere — and only fires in the desktop
 * shell.
 */
export function useFullscreen(): boolean {
  const [fullscreen, setFullscreen] = useState(false)

  useEffect(() => {
    if (!desktop.isDesktop) return
    let disposed = false

    const sync = () => {
      void desktop.isFullscreen().then((v) => {
        if (!disposed) setFullscreen(v)
      })
    }
    sync()
    let unlisten: (() => void) | undefined
    void desktop.onResized(sync).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })

    const onKey = (e: KeyboardEvent) => {
      const mod = isMac ? e.metaKey && e.ctrlKey : e.ctrlKey && e.altKey
      if (e.code === 'KeyF' && mod && !e.shiftKey && (isMac ? !e.altKey : !e.metaKey)) {
        e.preventDefault()
        // A rejection here would surface as an unhandled promise rejection with
        // nothing on screen; the header button is where the failure is told.
        void desktop.toggleFullscreen().catch(() => {})
      }
    }
    window.addEventListener('keydown', onKey)

    return () => {
      disposed = true
      unlisten?.()
      window.removeEventListener('keydown', onKey)
    }
  }, [])

  return fullscreen
}
