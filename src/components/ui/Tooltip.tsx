import { useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { AnimatePresence, motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { TOOLTIP_Z, useMotion } from '@/lib/motion'
import {
  resolveTooltipPlacement,
  type TooltipAnchor,
  type TooltipPlacement,
} from './tooltipPlacement'

/**
 * Hover help with an intentional delay: instant tooltips fire while the
 * pointer is merely passing through and turn into visual noise.
 *
 * Every bubble portals to `document.body` and pins to the viewport with
 * `fixed` — an in-tree `absolute` bubble is clipped to a sliver by the first
 * `overflow-hidden` ancestor (SectionCard, collapsible rows, the instance
 * card) or by the page scroll body, which is exactly the recurring "black
 * bar" bug; there is no inline mode anymore to forget opting out of.
 *
 * Placement is a two-pass affair: the trigger rect at arm time picks the
 * anchor, then — still before paint — the mounted bubble is measured and the
 * final spot clamped against the viewport edges (`resolveTooltipPlacement`).
 * `w-max` matters as much as the measuring: without a set width the
 * shrink-to-fit rules would squeeze the bubble into whatever room is left
 * between its anchor and the viewport edge.
 */
/** Marker consumed by the focus path; exported for callers that must undo it. */
export const SUPPRESS_FOCUS_ATTR = 'data-tooltip-suppress-focus'

/**
 * Mark a programmatic focus so the Tooltip wrapping this element skips its
 * next focus-driven arm. Menu hands focus back to its trigger on close — a
 * pure focus restoration, not a new hint request — and without this marker
 * the armed delay re-opened the bubble 420ms later with the pointer far
 * away, where no mouseleave would ever dismiss it. One-shot: consumed by
 * that next focus event and cleared by any real hover, so keyboard focusing
 * the trigger afterwards still shows the hint.
 */
export function suppressTooltipOnFocus(el: HTMLElement) {
  el.setAttribute(SUPPRESS_FOCUS_ATTR, '')
}

export function Tooltip({
  content,
  children,
  side = 'top',
  delay = 420,
  className,
}: {
  content: ReactNode
  children: ReactNode
  side?: 'top' | 'bottom' | 'left' | 'right'
  delay?: number
  className?: string
}) {
  const [open, setOpen] = useState(false)
  const [anchor, setAnchor] = useState<TooltipAnchor | null>(null)
  const [place, setPlace] = useState<TooltipPlacement | null>(null)
  const anchorRef = useRef<HTMLSpanElement>(null)
  const bubbleRef = useRef<HTMLSpanElement>(null)
  const timer = useRef<number>()
  // Whether the pointer currently rests on the trigger. While it does, the
  // hover path owns the hint: a focus event under the pointer (clicking the
  // trigger, including the one that closes its menu) must not re-arm the
  // bubble, or a dismissed hint resurfaces 420ms after the click with no
  // pointer movement to dismiss it again.
  const hovering = useRef(false)
  const { t, scale } = useMotion()
  const tipId = useId()

  // `anchor`/`place` are deliberately *not* cleared on hide: the exit frame
  // of the portal bubble still renders at the last anchor position, and
  // nulling them mid-exit would teleport it to the viewport corner.
  const hide = () => {
    window.clearTimeout(timer.current)
    setOpen(false)
  }

  // A click or Enter on the trigger re-focuses it, which re-arms `show()`
  // through `onFocus` — so a tooltip opened by hover would survive after the
  // pointer leaves, stuck above whatever the click revealed (the card's
  // overflow-hidden then used to clip it into a black bar). Dismiss on
  // press, like the scroll listener below dismisses on scroll.
  const onPointerDown = () => {
    if (open) hide()
  }

  // Blur alone is not enough to dismiss: when an element opens its panel in a
  // body portal (the Menu), focus leaves the anchor span, and the browser can
  // report the blur without a window focus to land on — native focusout on
  // the span covers every real focus move (portal panel included) and the
  // contains-check keeps focus shuffling inside the trigger from closing it.
  useEffect(() => {
    const anchorEl = anchorRef.current
    if (!anchorEl) return
    const onFocusOut = (e: FocusEvent) => {
      if (!anchorEl.contains(e.relatedTarget as Node | null)) hide()
    }
    anchorEl.addEventListener('focusout', onFocusOut)
    return () => anchorEl.removeEventListener('focusout', onFocusOut)
  }, [])

  const show = () => {
    // An empty tooltip would still render a bare dark pill on hover (the
    // bubble is styled, not gated on content) — callers legitimately pass an
    // empty string when the hint only applies in some state (e.g. a snapshot
    // button's "stop the instance first" note). Nothing to show: don't arm.
    if (content == null || content === '') return
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => {
      const r = anchorRef.current?.getBoundingClientRect()
      if (!r) return
      setAnchor({
        top: r.top,
        right: r.right,
        bottom: r.bottom,
        left: r.left,
        width: r.width,
        height: r.height,
      })
      setOpen(true)
    }, delay)
  }

  // The bubble mounts unpositioned; one synchronous pass measures it and
  // resolves the final spot before the first paint. A content swap while the
  // bubble is open re-runs this too — the new text can be a different size,
  // and the viewport clamp has to be re-evaluated for it. Scrolling or
  // resizing detaches a fixed-position bubble from its trigger — hiding
  // beats trying to follow it.
  useLayoutEffect(() => {
    if (!open || !anchor) return
    const el = bubbleRef.current
    if (!el) return
    const next = resolveTooltipPlacement(
      side,
      anchor,
      el.offsetWidth,
      el.offsetHeight,
      window.innerWidth,
      window.innerHeight,
    )
    setPlace((prev) =>
      prev &&
      prev.x === next.x &&
      prev.y === next.y &&
      prev.tx === next.tx &&
      prev.ty === next.ty &&
      prev.side === next.side
        ? prev
        : next,
    )
  }, [open, anchor, side, content])

  // `show()` refuses empty content at arm time, but a swap can drain the
  // bubble while it is open — e.g. the trigger text got short enough to fit,
  // so the consumer's `truncated ? text : ''` passes '' again. An empty
  // styled pill must not linger where a hint used to be.
  useEffect(() => {
    if (open && (content == null || content === '')) setOpen(false)
  }, [open, content])

  useEffect(() => {
    if (!open) return
    const close = () => setOpen(false)
    // Only a scroll that actually MOVES the trigger detaches the bubble:
    // pages re-clamp their scroll position while async content settles (the
    // create wizard's readiness banner, list refills) and that fires a scroll
    // event with the pointer untouched — it used to kill a bubble the user
    // was still reading. Resize still closes outright.
    const anchorTop = anchorRef.current?.getBoundingClientRect().top ?? 0
    const anchorLeft = anchorRef.current?.getBoundingClientRect().left ?? 0
    const onScroll = () => {
      const r = anchorRef.current?.getBoundingClientRect()
      if (!r || Math.abs(r.top - anchorTop) > 1 || Math.abs(r.left - anchorLeft) > 1) close()
    }
    window.addEventListener('scroll', onScroll, true)
    window.addEventListener('resize', close)
    return () => {
      window.removeEventListener('scroll', onScroll, true)
      window.removeEventListener('resize', close)
    }
  }, [open])

  // The bubble is the trigger's *description*, not its label: the bubble
  // carries `role="tooltip"` and the trigger points at it with
  // `aria-describedby`, so a screen reader reads the hint while the trigger
  // keeps its own name (2026-09-14 desktop review).
  //
  // The id goes onto the trigger's DOM node rather than down as a prop:
  // `Badge` and `Switch` build their own elements and drop unknown props, so
  // the real `CompatBadge` rendered a bubble that nothing in the DOM pointed
  // at (2026-09-15 review, N04-R1) — and teaching today's wrappers to forward
  // the prop would leave the next one to regress the same way. Reading the
  // attribute back is also what makes the merge right: the trigger may already
  // describe itself, React has committed that by now, and the only id this
  // component owns is its own. Re-deriving each commit keeps that true when
  // the node or its description swaps under an open bubble; the equality guard
  // skips the frames where nothing changed.
  useLayoutEffect(() => {
    const el = anchorRef.current?.firstElementChild as HTMLElement | null
    if (!el) return
    const current = el.getAttribute('aria-describedby')
    const kept = (current ?? '').split(/\s+/).filter((id) => id && id !== tipId)
    const next = (open ? [...kept, tipId] : kept).join(' ')
    if (current === (next || null)) return
    if (next) el.setAttribute('aria-describedby', next)
    else el.removeAttribute('aria-describedby')
  }, [open, children, tipId])

  // The bubble's centering offset rides the CSS translate property, NOT
  // transform: this is a motion.span animating x/y, and motion writes its
  // own transform (resolving to none once settled), which clobbers any
  // transform-based centering here. translate composes with transform;
  // WebView2's Chromium supports it since 104.
  const enterOffset = { top: { y: 4 }, bottom: { y: -4 }, left: { x: 4 }, right: { x: -4 } }[
    place?.side ?? side
  ]

  return (
    <span
      ref={anchorRef}
      className={cn('relative inline-flex', className)}
      onMouseEnter={() => {
        // A real pointer arrival is user intent and outranks a pending
        // suppression from some earlier programmatic focus.
        hovering.current = true
        anchorRef.current
          ?.querySelectorAll(`[${SUPPRESS_FOCUS_ATTR}]`)
          .forEach((el) => el.removeAttribute(SUPPRESS_FOCUS_ATTR))
        show()
      }}
      onMouseLeave={() => {
        hovering.current = false
        hide()
      }}
      onPointerDown={onPointerDown}
      onFocus={(e) => {
        // Consume a programmatic focus-restore marker BEFORE any early return:
        // the restore can arrive while the pointer rests on the trigger (menu
        // closed with Esc without moving the mouse), and skipping the marker
        // check then would leave it behind to swallow the next real keyboard
        // focus instead.
        const flagged = (e.target as Element | null)?.closest?.(`[${SUPPRESS_FOCUS_ATTR}]`)
        if (flagged) {
          flagged.removeAttribute(SUPPRESS_FOCUS_ATTR)
          return
        }
        if (hovering.current) return
        show()
      }}
      onBlur={hide}
    >
      {/* The trigger renders untouched: its description link is applied to the
          committed node by the effect above. */}
      {children}
      {/* The portal host stays mounted so AnimatePresence outlives `open`
          and can play the bubble's exit. */}
      {createPortal(
        <AnimatePresence>
          {open && (
            <motion.span
              ref={bubbleRef}
              id={tipId}
              role="tooltip"
              initial={{ opacity: 0, ...(scale === 0 ? {} : enterOffset) }}
              animate={{ opacity: 1, x: 0, y: 0 }}
              exit={{ opacity: 0 }}
              transition={t(0.14)}
              className={cn(
                TOOLTIP_Z,
                'pointer-events-none fixed w-max max-w-[min(440px,86vw)] whitespace-normal rounded-sm bg-ink px-2 py-1 text-xs font-medium leading-relaxed text-canvas shadow-pop',
              )}
              style={
                place
                  ? { left: place.x, top: place.y, translate: `${place.tx} ${place.ty}` }
                  : { visibility: 'hidden' }
              }
            >
              {content}
            </motion.span>
          )}
        </AnimatePresence>,
        document.body,
      )}
    </span>
  )
}
