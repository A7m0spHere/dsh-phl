import { useEffect, useRef, useState } from 'react'

/** Ticks once per second while `startedAt` is set; returns elapsed seconds. */
export function useUptime(startedAt?: number): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!startedAt) return
    setNow(Date.now())
    const id = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(id)
  }, [startedAt])
  return startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : 0
}

export function useInterval(fn: () => void, ms: number | null) {
  const saved = useRef(fn)
  saved.current = fn
  useEffect(() => {
    if (ms === null) return
    const id = window.setInterval(() => saved.current(), ms)
    return () => window.clearInterval(id)
  }, [ms])
}

export interface Hotkey {
  /** Lower-case key, e.g. `k`, `n`, `escape`. */
  key: string
  ctrl?: boolean
  shift?: boolean
  alt?: boolean
  run: (e: KeyboardEvent) => void
  /** Fire even while a text field has focus. */
  global?: boolean
}

const isTyping = (target: EventTarget | null) => {
  const el = target as HTMLElement | null
  if (!el) return false
  return (
    el.tagName === 'INPUT' ||
    el.tagName === 'TEXTAREA' ||
    el.tagName === 'SELECT' ||
    el.isContentEditable
  )
}

export function useHotkeys(hotkeys: Hotkey[]) {
  const saved = useRef(hotkeys)
  saved.current = hotkeys
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      for (const hk of saved.current) {
        if (e.key.toLowerCase() !== hk.key) continue
        if (!!hk.ctrl !== (e.ctrlKey || e.metaKey)) continue
        if (!!hk.shift !== e.shiftKey) continue
        if (!!hk.alt !== e.altKey) continue
        if (!hk.global && isTyping(e.target)) continue
        e.preventDefault()
        hk.run(e)
        return
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [])
}

/** `true` after the first paint — lets a mount animation run only once. */
export function useMounted(): boolean {
  const [mounted, setMounted] = useState(false)
  useEffect(() => setMounted(true), [])
  return mounted
}
