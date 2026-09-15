import { useEffect, useRef, useState } from 'react'

// +1: sub-pixel rounding between scrollWidth and clientWidth.
const isOverflowing = (el: HTMLElement) =>
  el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1

/**
 * True when the referenced element is actually cutting content off —
 * `truncate`'s horizontal ellipsis or `line-clamp`'s vertical one. Drives
 * "reveal on hover" affordances, which must stay quiet while the text is
 * fully visible: a bubble repeating readable text is noise (2026-09-12
 * review: the old native-`title` reveals fired unconditionally).
 *
 * Re-checks after every commit: a content swap that keeps the same box does
 * not re-fire the ResizeObserver, and the reveal must follow the text
 * (short→long, long→short, including under an open bubble) without callers
 * remembering to remount via a `key`. Resizes land outside React's commit
 * cycle, so the observer stays for those.
 */
export function useTruncated<T extends HTMLElement>() {
  const ref = useRef<T>(null)
  const [truncated, setTruncated] = useState(false)
  useEffect(() => {
    const el = ref.current
    if (el) setTruncated(isOverflowing(el))
  })
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const ro = new ResizeObserver(() => setTruncated(isOverflowing(el)))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, truncated] as const
}

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

export const isTyping = (target: EventTarget | null) => {
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
