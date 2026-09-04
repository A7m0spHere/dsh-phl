import { useEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { AnimatePresence, motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

/**
 * Hover help with an intentional delay: instant tooltips fire while the
 * pointer is merely passing through and turn into visual noise.
 *
 * `allowOverflow` pins the bubble to the viewport instead of the trigger:
 * anything inside a SectionCard or a collapsible row (both `overflow-hidden`
 * for their height animation) would otherwise clip a wide tooltip against
 * the card edge. The body-level portal is the only place a tooltip can be
 * guaranteed to fit; `fixed` keeps it attached even when an ancestor
 * establishes its own stacking context (e.g. `transform: none` pages).
 */
export function Tooltip({
  content,
  children,
  side = 'top',
  delay = 420,
  className,
  allowOverflow = false,
}: {
  content: ReactNode
  children: ReactNode
  side?: 'top' | 'bottom' | 'left' | 'right'
  delay?: number
  className?: string
  allowOverflow?: boolean
}) {
  const [open, setOpen] = useState(false)
  const [tip, setTip] = useState<{ x: number; y: number; flip: boolean } | null>(null)
  const anchorRef = useRef<HTMLSpanElement>(null)
  const timer = useRef<number>()
  const { t, scale } = useMotion()

  // `tip` is deliberately *not* cleared on hide: the exit frame of the
  // portal bubble still renders at the last anchor position, and nulling
  // the position mid-exit would teleport it to the viewport corner.
  const hide = () => {
    window.clearTimeout(timer.current)
    setOpen(false)
  }

  const show = () => {
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => {
      if (allowOverflow) {
        const r = anchorRef.current?.getBoundingClientRect()
        if (!r) return
        const ax = r.left + r.width / 2
        // Near the right viewport edge the centered bubble would spill off
        // screen: right-align it onto the trigger instead.
        const flip = ax > window.innerWidth - 190
        setTip({
          x: flip ? r.right : ax,
          y: side === 'bottom' ? r.bottom + 6 : r.top - 6,
          flip,
        })
      }
      setOpen(true)
    }, delay)
  }

  // Scrolling or clicking out detaches a fixed-position bubble from its
  // trigger — hiding on scroll beats trying to follow it.
  useEffect(() => {
    if (!open || !allowOverflow) return
    const close = () => setOpen(false)
    window.addEventListener('scroll', close, true)
    return () => window.removeEventListener('scroll', close, true)
  }, [open, allowOverflow])

  const pos = {
    top: 'bottom-full left-1/2 -translate-x-1/2 mb-1.5',
    bottom: 'top-full left-1/2 -translate-x-1/2 mt-1.5',
    left: 'right-full top-1/2 -translate-y-1/2 mr-1.5',
    right: 'left-full top-1/2 -translate-y-1/2 ml-1.5',
  }[side]

  const offset = { top: { y: 4 }, bottom: { y: -4 }, left: { x: 4 }, right: { x: -4 } }[side]

  const bubble = (
    <motion.span
      initial={{ opacity: 0, ...(scale === 0 ? {} : offset) }}
      animate={{ opacity: 1, x: 0, y: 0 }}
      exit={{ opacity: 0 }}
      transition={t(0.14)}
      className={cn(
        'pointer-events-none z-[80] rounded-sm bg-ink px-2 py-1 text-xs font-medium text-canvas shadow-pop',
        // Portal bubbles can run wider than the viewport (joined env-var
        // lists); wrap them instead of scrolling the page sideways. Inline
        // bubbles keep the classic single-line behavior.
        allowOverflow
          ? 'fixed max-w-[min(440px,86vw)] whitespace-normal leading-relaxed'
          : `absolute whitespace-nowrap ${pos}`,
      )}
      style={
        allowOverflow && tip
          ? {
              left: tip.x,
              top: tip.y,
              transform: `translate(${tip.flip ? '-100%' : '-50%'}, ${side === 'bottom' ? '0' : '-100%'})`,
            }
          : undefined
      }
    >
      {content}
    </motion.span>
  )

  return (
    <span
      ref={anchorRef}
      className={cn('relative inline-flex', className)}
      onMouseEnter={show}
      onMouseLeave={hide}
      onFocus={show}
      onBlur={hide}
    >
      {children}
      {allowOverflow
        ? // The portal host stays mounted so AnimatePresence outlives
          // `open` and can play the bubble's exit.
          createPortal(
            <AnimatePresence>{open ? bubble : null}</AnimatePresence>,
            document.body,
          )
        : <AnimatePresence>{open ? bubble : null}</AnimatePresence>}
    </span>
  )
}
