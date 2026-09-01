import { useEffect, useState } from 'react'
import { desktop } from '@/lib/desktop'

/** Tracks the window's maximized state so the restore glyph stays accurate. */
export function useMaximized(): boolean {
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    if (!desktop.isDesktop) return
    let disposed = false
    let unlisten: (() => void) | undefined

    const sync = () => {
      void desktop.isMaximized().then((v) => {
        if (!disposed) setMaximized(v)
      })
    }

    sync()
    void desktop.onResized(sync).then((fn) => {
      if (disposed) fn()
      else unlisten = fn
    })

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  return maximized
}
