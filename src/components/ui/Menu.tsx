import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { AnimatePresence, motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'
import { computePlacement, estimateMenuHeight } from './menuPlacement'

export interface MenuItem {
  id: string
  label: ReactNode
  icon?: ReactNode
  shortcut?: string
  danger?: boolean
  disabled?: boolean
  onSelect?: () => void
  /** Renders a hairline above this item. */
  separated?: boolean
}


/**
 * A small anchored menu. Opens from the corner nearest its trigger so it
 * appears to grow out of the button rather than drop on top of it.
 *
 * The panel is portalled to `document.body` and positioned `fixed` from the
 * trigger's live rect — page headers carry a transform animation and scroll
 * bodies set `overflow`, and an in-tree `absolute` panel gets clipped the
 * moment it grows past those containers (a 10-item menu showed ~2). `side`
 * states the preferred direction; the placement flips when there is no room
 * that way, and resize/scroll re-run it while open.
 */
export function Menu({
  trigger,
  items,
  align = 'end',
  side = 'bottom',
  className,
  width = 184,
}: {
  trigger: (props: { open: boolean; toggle: () => void }) => ReactNode
  items: MenuItem[]
  align?: 'start' | 'end'
  side?: 'top' | 'bottom'
  className?: string
  width?: number
}) {
  const [open, setOpen] = useState(false)
  const [dropUp, setDropUp] = useState(side === 'top')
  const [pos, setPos] = useState<CSSProperties>({ visibility: 'hidden' })
  const anchorRef = useRef<HTMLDivElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const { t, scale } = useMotion()

  useLayoutEffect(() => {
    if (!open) return
    const place = () => {
      const rect = anchorRef.current?.getBoundingClientRect()
      if (!rect) return
      // Once mounted, the real panel height beats the estimate; before that
      // (first layout pass) the estimate decides the side.
      const height = panelRef.current?.offsetHeight || estimateMenuHeight(items)
      const { top, left, dropUp: up } = computePlacement({
        rect,
        height,
        width,
        align,
        side,
        viewportW: window.innerWidth,
        viewportH: window.innerHeight,
      })
      setDropUp(up)
      setPos({ position: 'fixed', top, left, width, visibility: 'visible' })
    }
    place()
    // Re-run after the panel mounts so its true height refines the spot
    // (before paint — layout effects run synchronously against the DOM).
    const raf = requestAnimationFrame(place)
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    document.addEventListener('mousedown', onOutside)
    document.addEventListener('keydown', onKey)
    return () => {
      cancelAnimationFrame(raf)
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
      document.removeEventListener('mousedown', onOutside)
      document.removeEventListener('keydown', onKey)
    }

    function onOutside(e: MouseEvent) {
      const target = e.target as Node
      if (!anchorRef.current?.contains(target) && !panelRef.current?.contains(target)) {
        setOpen(false)
      }
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') setOpen(false)
    }
  }, [open, items, width, align, side])

  return (
    <>
      <div ref={anchorRef} className={cn('inline-flex', className)}>
        {trigger({ open, toggle: () => setOpen((v) => !v) })}
      </div>
      {createPortal(
        <AnimatePresence>
          {open && (
            <motion.div
              ref={panelRef}
              role="menu"
              data-menu-panel
              initial={{ opacity: 0, scale: scale === 0 ? 1 : 0.95, y: scale === 0 ? 0 : dropUp ? 4 : -4 }}
              animate={{ opacity: 1, scale: 1, y: 0 }}
              exit={{ opacity: 0, scale: scale === 0 ? 1 : 0.97, y: scale === 0 ? 0 : dropUp ? 2 : -2 }}
              transition={t(0.15)}
              style={{
                ...pos,
                transformOrigin: `${dropUp ? 'bottom' : 'top'} ${align === 'end' ? 'right' : 'left'}`,
              }}
              className="z-[60] overflow-hidden rounded-lg bg-surface-raised p-1 shadow-pop ring-1 ring-inset ring-line"
            >
              {items.map((item) => (
                <div key={item.id}>
                  {item.separated && <div className="my-1 h-px bg-line" />}
                  <button
                    role="menuitem"
                    disabled={item.disabled}
                    onClick={() => {
                      setOpen(false)
                      item.onSelect?.()
                    }}
                    className={cn(
                      'flex w-full items-center gap-2 whitespace-nowrap rounded-sm px-2 py-[6px] text-left text-base transition-colors duration-100',
                      item.danger
                        ? 'text-danger hover:bg-danger/10'
                        : 'text-ink hover:bg-surface-hover',
                      item.disabled && 'pointer-events-none opacity-40',
                    )}
                  >
                    {item.icon && <span className="shrink-0 text-ink-faint">{item.icon}</span>}
                    <span className="flex-1 truncate">{item.label}</span>
                    {item.shortcut && (
                      <span className="num shrink-0 text-2xs text-ink-faint">{item.shortcut}</span>
                    )}
                  </button>
                </div>
              ))}
            </motion.div>
          )}
        </AnimatePresence>,
        document.body,
      )}
    </>
  )
}
