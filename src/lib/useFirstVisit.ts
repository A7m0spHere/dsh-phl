import { useRef } from 'react'

/**
 * Session-scoped "have I shown this before?" flag.
 *
 * Entrance choreography is an introduction, not a loop: a page that staggers
 * ten sections in on every visit is charming once and a queue the rest of the
 * day. Keying off a module-level set (not state) means the answer survives the
 * component unmounting when you navigate away and back, and it resets with the
 * app — exactly the lifetime a first-look reveal should have.
 *
 * Returns `true` on the first mount for a key and `false` afterwards; the value
 * is stable for the life of the mount, so it can drive `animate` directly.
 */
const seen = new Set<string>()

export function useFirstVisit(key: string): boolean {
  const first = useRef<boolean | null>(null)
  if (first.current === null) {
    first.current = !seen.has(key)
    seen.add(key)
  }
  return first.current
}
