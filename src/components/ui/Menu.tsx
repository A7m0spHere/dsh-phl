import { useEffect, useRef, useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

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
  const ref = useRef<HTMLDivElement>(null)
  const { t, scale } = useMotion()

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  return (
    <div ref={ref} className={cn('relative', className)}>
      {trigger({ open, toggle: () => setOpen((v) => !v) })}
      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, scale: scale === 0 ? 1 : 0.95, y: scale === 0 ? 0 : side === 'top' ? 4 : -4 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: scale === 0 ? 1 : 0.97, y: scale === 0 ? 0 : side === 'top' ? 2 : -2 }}
            transition={t(0.15)}
            style={{
              width,
              transformOrigin: `${side === 'top' ? 'bottom' : 'top'} ${align === 'end' ? 'right' : 'left'}`,
            }}
            className={cn(
              'absolute z-50 overflow-hidden rounded-lg bg-surface-raised p-1 shadow-pop ring-1 ring-inset ring-line',
              side === 'top' ? 'bottom-full mb-1.5' : 'top-full mt-1.5',
              align === 'end' ? 'right-0' : 'left-0',
            )}
          >
            {items.map((item) => (
              <div key={item.id}>
                {item.separated && <div className="my-1 h-px bg-line" />}
                <button
                  disabled={item.disabled}
                  onClick={() => {
                    setOpen(false)
                    item.onSelect?.()
                  }}
                  className={cn(
                    'flex w-full items-center gap-2 rounded-sm px-2 py-[6px] text-left text-base transition-colors duration-100',
                    item.danger
                      ? 'text-danger hover:bg-danger/10'
                      : 'text-ink hover:bg-surface-hover',
                    item.disabled && 'pointer-events-none opacity-40',
                  )}
                >
                  {item.icon && <span className="shrink-0 text-ink-faint">{item.icon}</span>}
                  <span className="flex-1 truncate">{item.label}</span>
                  {item.shortcut && (
                    <span className="shrink-0 text-2xs text-ink-faint">{item.shortcut}</span>
                  )}
                </button>
              </div>
            ))}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}
