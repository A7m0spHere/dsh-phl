import { useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { AnimatePresence, motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { MENU_Z, useMotion } from '@/lib/motion'
import { computePlacement, estimateMenuHeight } from './menuPlacement'
import { suppressTooltipOnFocus } from './Tooltip'

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
  trigger: (props: {
    open: boolean
    toggle: () => void
    /** Spread onto the trigger button: the menu's state, announced. */
    menuProps: { 'aria-haspopup': 'menu'; 'aria-expanded': boolean }
  }) => ReactNode
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

  /**
   * Keyboard contract: opening moves focus to the first item, arrows cycle,
   * Escape (handled below) and outside clicks close, and closing hands focus
   * back to the trigger. Without this the menu was mouse-only.
   *
   * Restoring focus must only happen on a genuine open→closed transition:
   * running it on mount (open starts false) would steal focus to the trigger
   * on every page that renders a Menu — which also pinned a Tooltip open on
   * the freshly-focused trigger, a bubble that never dismissed.
   */
  const wasOpen = useRef(false)
  /**
   * Focus the first item once the panel can actually take focus.
   *
   * The panel mounts `visibility: hidden` until the placement pass resolves its
   * spot, and `focus()` silently no-ops on a hidden element. Gating on the
   * placement *state* is not enough: the panel is a `motion.div`, and Motion
   * applies the style it was handed in a later frame, so the state can already
   * say "visible" while the item still computes `hidden` — measured on a
   * 1000×660 window at 100% DPI, where a single `requestAnimationFrame` (and a
   * placement-state gate) both ran too early and left focus on the trigger, so
   * Enter opened a menu the keyboard could not enter. Verifying that the focus
   * actually took, with a bounded retry, is timing-independent.
   */
  useLayoutEffect(() => {
    if (!open) return
    let raf = 0
    let tries = 0
    const attempt = () => {
      const item = panelRef.current?.querySelector<HTMLElement>('[role="menuitem"]:not([disabled])')
      if (!item) return
      item.focus()
      if (document.activeElement !== item && ++tries < 10) raf = requestAnimationFrame(attempt)
    }
    attempt()
    return () => cancelAnimationFrame(raf)
  }, [open])
  useLayoutEffect(() => {
    if (open) {
      wasOpen.current = true
      return
    }
    if (wasOpen.current) {
      wasOpen.current = false
      // Handing focus back is a restoration, not a new hint request: mark the
      // trigger so the wrapping Tooltip skips the arm this focus would
      // otherwise start (it used to re-open the bubble 420ms after close,
      // with the pointer far away and nothing left to dismiss it). When the
      // trigger already holds focus (closed by clicking the trigger again)
      // refocusing is a no-op that fires no events — no marker needed.
      const trigger = anchorRef.current?.querySelector<HTMLElement>('button, [role="button"]')
      if (trigger && document.activeElement !== trigger) {
        suppressTooltipOnFocus(trigger)
        trigger.focus()
      }
    }
  }, [open])

  const onPanelKeyDown = (event: React.KeyboardEvent) => {
    const keys = ['ArrowDown', 'ArrowUp', 'Home', 'End']
    if (!keys.includes(event.key)) return
    event.preventDefault()
    const items = Array.from(
      panelRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])') ?? [],
    )
    if (items.length === 0) return
    const current = items.indexOf(document.activeElement as HTMLElement)
    const next =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? items.length - 1
          : event.key === 'ArrowDown'
            ? (current + 1) % items.length
            : (current - 1 + items.length) % items.length
    items[next].focus()
  }

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
        {trigger({
          open,
          toggle: () => setOpen((v) => !v),
          menuProps: { 'aria-haspopup': 'menu', 'aria-expanded': open },
        })}
      </div>
      {createPortal(
        <AnimatePresence>
          {open && (
            <motion.div
              ref={panelRef}
              role="menu"
              data-menu-panel
              onKeyDown={onPanelKeyDown}
              initial={{ opacity: 0, scale: scale === 0 ? 1 : 0.95, y: scale === 0 ? 0 : dropUp ? 4 : -4 }}
              animate={{ opacity: 1, scale: 1, y: 0 }}
              exit={{ opacity: 0, scale: scale === 0 ? 1 : 0.97, y: scale === 0 ? 0 : dropUp ? 2 : -2 }}
              transition={t(0.15)}
              style={{
                ...pos,
                transformOrigin: `${dropUp ? 'bottom' : 'top'} ${align === 'end' ? 'right' : 'left'}`,
              }}
              className={cn(MENU_Z, 'overflow-hidden rounded-lg bg-surface-raised p-1 shadow-pop ring-1 ring-inset ring-line')}
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
