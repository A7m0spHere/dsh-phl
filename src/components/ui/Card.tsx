import { forwardRef, type HTMLAttributes, type ReactNode, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { ChevronDown } from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

export interface CardProps extends HTMLAttributes<HTMLDivElement> {
  /** Lifts and brightens on hover; use for anything clickable. */
  interactive?: boolean
  /** Removes the inner padding so the caller can lay out edge-to-edge. */
  flush?: boolean
  tone?: 'default' | 'sunken' | 'accent' | 'danger'
}

const TONES = {
  default: 'bg-surface',
  sunken: 'bg-surface-sunken',
  accent: 'bg-accent-soft/60',
  danger: 'bg-danger/[0.045]',
} as const

export const Card = forwardRef<HTMLDivElement, CardProps>(function Card(
  { interactive, flush, tone = 'default', className, children, ...rest },
  ref,
) {
  return (
    <div
      ref={ref}
      className={cn(
        'relative rounded-lg ring-1 ring-inset ring-line transition-[box-shadow,background-color,transform] duration-200 ease-out',
        TONES[tone],
        tone === 'danger' && 'ring-danger/20',
        !flush && 'p-3',
        interactive && 'cursor-pointer hover:ring-line-strong/80 hover:shadow-lift',
        className,
      )}
      {...rest}
    >
      {children}
    </div>
  )
})

/* ------------------------------------------------------------------ */

interface SectionCardProps {
  title: ReactNode
  /** Right-hand slot in the header — counts, actions, badges. */
  extra?: ReactNode
  icon?: ReactNode
  description?: ReactNode
  children: ReactNode
  className?: string
  bodyClassName?: string
  /** Collapsible sections remember their state per mount. */
  collapsible?: boolean
  defaultOpen?: boolean
}

/**
 * The workhorse panel: a titled block with an optional collapse. Collapsing
 * animates real height so the surrounding layout settles rather than jumps.
 */
export function SectionCard({
  title,
  extra,
  icon,
  description,
  children,
  className,
  bodyClassName,
  collapsible = false,
  defaultOpen = true,
}: SectionCardProps) {
  const [open, setOpen] = useState(defaultOpen)
  const { t, scale } = useMotion()

  const header = (
    <div
      className={cn(
        'flex min-h-[32px] items-center gap-2 px-3 py-1.5',
        collapsible && 'cursor-pointer select-none hover:bg-surface-hover/60',
      )}
      onClick={collapsible ? () => setOpen((v) => !v) : undefined}
      role={collapsible ? 'button' : undefined}
      tabIndex={collapsible ? 0 : undefined}
      onKeyDown={
        collapsible
          ? (e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault()
                setOpen((v) => !v)
              }
            }
          : undefined
      }
    >
      {collapsible && (
        <motion.span
          animate={{ rotate: open ? 0 : -90 }}
          transition={t(0.2)}
          className="-ml-1 text-ink-faint"
        >
          <ChevronDown size={14} />
        </motion.span>
      )}
      {icon && <span className="text-ink-faint">{icon}</span>}
      <h3 className="text-base font-medium text-ink">{title}</h3>
      {description && <span className="truncate text-sm text-ink-faint">{description}</span>}
      <div className="ml-auto flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
        {extra}
      </div>
    </div>
  )

  return (
    <div className={cn('overflow-hidden rounded-lg bg-surface ring-1 ring-inset ring-line', className)}>
      {header}
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            key="body"
            initial={scale === 0 ? false : { height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={t(0.24)}
            className="overflow-hidden"
          >
            <div className={cn('border-t border-line px-3 py-2.5', bodyClassName)}>{children}</div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}

/* ------------------------------------------------------------------ */

/** A label/value row used throughout the detail views. */
export function DataRow({
  label,
  value,
  mono,
  action,
}: {
  label: ReactNode
  value: ReactNode
  mono?: boolean
  action?: ReactNode
}) {
  return (
    <div className="group/row flex min-h-[22px] items-baseline gap-3 py-[2px]">
      <span className="w-[88px] shrink-0 text-sm text-ink-faint">{label}</span>
      <span
        className={cn(
          'min-w-0 flex-1 break-all text-base text-ink',
          mono && 'font-mono text-sm text-ink-muted',
        )}
      >
        {value}
      </span>
      {action && (
        <span className="shrink-0 opacity-0 transition-opacity group-hover/row:opacity-100">
          {action}
        </span>
      )}
    </div>
  )
}
