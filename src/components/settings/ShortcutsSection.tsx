import { useEffect, useMemo, useRef, useState } from 'react'
import { motion } from 'motion/react'
import { Search } from 'lucide-react'
import { isMacPlatform, isTyping, matchesShortcut, shortcutChips, SHORTCUT_GROUPS } from '@/lib/shortcuts'
import { useMotion, D } from '@/lib/motion'
import { Kbd } from '@/components/ui'
import { PageSection } from '@/components/layout/Page'

/** How long a pressed shortcut stays highlighted. */
const FLASH_MS = 1000

/**
 * The shortcuts reference, as one settings section.
 *
 * Two ideas from open-source keybinding UIs, re-skinned in this app's design
 * language: a grouped list with `<kbd>` chips (GitHub / GitLab's press-`?`
 * help) and VS Code's live capture — while this section is on screen,
 * pressing any registered shortcut highlights the matching row, so you can
 * feel what each binding actually is.
 */
export function ShortcutsSection() {
  const { t, scale, stagger, riseItem } = useMotion()
  const mac = useMemo(isMacPlatform, [])
  const [query, setQuery] = useState('')
  const [flash, setFlash] = useState<{ ids: Set<string>; nonce: number }>({
    ids: new Set(),
    nonce: 0,
  })
  const flashTimer = useRef<number>()

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const ids = SHORTCUT_GROUPS.flatMap((g) => g.items)
        // Same typing-field rule as the real bindings: non-global hotkeys are
        // ignored while a text field has focus, so the demo must not light up
        // rows whose keys would not actually fire (typing in the search box).
        .filter((s) => (s.global || !isTyping(e.target)) && matchesShortcut(s, e))
        .map((s) => s.id)
      if (!ids.length) return
      window.clearTimeout(flashTimer.current)
      setFlash((f) => ({ ids: new Set(ids), nonce: f.nonce + 1 }))
      flashTimer.current = window.setTimeout(() => setFlash({ ids: new Set(), nonce: 0 }), FLASH_MS)
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
      window.clearTimeout(flashTimer.current)
    }
  }, [])

  const q = query.trim().toLowerCase()
  const groups = useMemo(() => {
    if (!q) return SHORTCUT_GROUPS
    return SHORTCUT_GROUPS.map((g) => ({
      ...g,
      items: g.items.filter((s) => {
        const haystack = [s.description, s.scope ?? '', ...shortcutChips(s, mac)]
          .join(' ')
          .toLowerCase()
        return haystack.includes(q)
      }),
    })).filter((g) => g.items.length > 0)
  }, [q, mac])

  return (
    <div>
      <div className="mb-4 flex items-center gap-2 rounded-lg bg-surface px-3.5 py-2.5 ring-1 ring-inset ring-line transition-shadow focus-within:ring-accent-ink/40">
        <Search size={14} className="shrink-0 text-ink-faint" />
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索快捷键或功能"
          aria-label="搜索快捷键"
          className="min-w-0 flex-1 bg-transparent text-base text-ink outline-none placeholder:text-ink-faint/70"
        />
        <span className="flex shrink-0 items-center gap-1.5 text-2xs text-ink-faint">
          <span className="relative flex h-1.5 w-1.5">
            {scale > 0 && (
              <motion.span
                className="absolute inset-0 rounded-full bg-accent"
                animate={{ scale: [1, 2.1], opacity: [0.5, 0] }}
                transition={{ ...t(1.4), repeat: Infinity }}
              />
            )}
            <span className="relative h-1.5 w-1.5 rounded-full bg-accent" />
          </span>
          试试按下快捷键
        </span>
      </div>

      {groups.length === 0 && (
        <p className="py-10 text-center text-base text-ink-faint">没有匹配的快捷键</p>
      )}

      {groups.map((g) => (
        <PageSection key={g.id} title={g.title} description={g.description}>
          <motion.div
            variants={stagger(0.03)}
            initial="hidden"
            animate="show"
            role="list"
            className="divide-y divide-line rounded-lg bg-surface px-4 ring-1 ring-inset ring-line"
          >
            {g.items.map((s) => {
              const active = flash.ids.has(s.id)
              return (
                <motion.div
                  key={`${g.id}:${s.id}`}
                  variants={riseItem}
                  role="listitem"
                  className="relative -mx-4 flex items-center gap-4 px-4 py-2.5"
                >
                  {active && (
                    <motion.span
                      initial={{ opacity: 0.8 }}
                      animate={{ opacity: 0 }}
                      transition={t(D.slow, D.fast)}
                      className="pointer-events-none absolute inset-y-0.5 inset-x-1 rounded-md bg-accent-soft"
                    />
                  )}
                  <div className="relative min-w-0 flex-1">
                    <div className="text-base text-ink">{s.description}</div>
                    {s.scope && <div className="mt-0.5 text-2xs text-ink-faint">{s.scope}</div>}
                  </div>
                  <div className="relative flex shrink-0 items-center gap-1">
                    {shortcutChips(s, mac).map((chip) => (
                      <motion.span
                        key={`${chip}:${active ? flash.nonce : 0}`}
                        initial={active ? { scale: 1 } : false}
                        animate={active ? { scale: [1, 1.18, 1] } : { scale: 1 }}
                        transition={t(D.slow)}
                        className="inline-flex"
                      >
                        <Kbd active={active}>{chip}</Kbd>
                      </motion.span>
                    ))}
                  </div>
                </motion.div>
              )
            })}
          </motion.div>
        </PageSection>
      ))}
    </div>
  )
}
