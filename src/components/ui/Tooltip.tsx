import { useRef, useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

/**
 * Hover help with an intentional delay: instant tooltips fire while the
 * pointer is merely passing through and turn into visual noise.
 */
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
  const timer = useRef<number>()
  const { t, scale } = useMotion()

  const show = () => {
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setOpen(true), delay)
  }
  const hide = () => {
    window.clearTimeout(timer.current)
    setOpen(false)
  }

  const pos = {
    top: 'bottom-full left-1/2 -translate-x-1/2 mb-1.5',
    bottom: 'top-full left-1/2 -translate-x-1/2 mt-1.5',
    left: 'right-full top-1/2 -translate-y-1/2 mr-1.5',
    right: 'left-full top-1/2 -translate-y-1/2 ml-1.5',
  }[side]

  const offset = { top: { y: 4 }, bottom: { y: -4 }, left: { x: 4 }, right: { x: -4 } }[side]

  return (
    <span
      className={cn('relative inline-flex', className)}
      onMouseEnter={show}
      onMouseLeave={hide}
      onFocus={show}
      onBlur={hide}
    >
      {children}
      <AnimatePresence>
        {open && (
          <motion.span
            initial={{ opacity: 0, ...(scale === 0 ? {} : offset) }}
            animate={{ opacity: 1, x: 0, y: 0 }}
            exit={{ opacity: 0 }}
            transition={t(0.14)}
            className={cn(
              'pointer-events-none absolute z-[60] whitespace-nowrap rounded-sm bg-ink px-2 py-1 text-xs font-medium text-canvas shadow-pop',
              pos,
            )}
          >
            {content}
          </motion.span>
        )}
      </AnimatePresence>
    </span>
  )
}
