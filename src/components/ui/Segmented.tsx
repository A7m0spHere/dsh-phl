import { type ReactNode, useId } from 'react'
import { motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

export interface SegmentedOption<T extends string> {
  value: T
  label: ReactNode
  icon?: ReactNode
  title?: string
}

/**
 * The selected background is a single shared element that slides between
 * options, so switching filters reads as one continuous movement instead of
 * two separate state changes.
 */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  size = 'md',
  className,
  block,
}: {
  value: T
  options: SegmentedOption<T>[]
  onChange: (v: T) => void
  size?: 'sm' | 'md'
  className?: string
  block?: boolean
}) {
  const layoutId = useId()
  const { spring } = useMotion()

  return (
    <div
      role="tablist"
      className={cn(
        'inline-flex items-center gap-0.5 rounded bg-surface-sunken p-[3px] ring-1 ring-inset ring-line',
        block && 'w-full',
        className,
      )}
    >
      {options.map((opt) => {
        const active = opt.value === value
        return (
          <button
            key={opt.value}
            role="tab"
            aria-selected={active}
            title={opt.title}
            onClick={() => onChange(opt.value)}
            className={cn(
              'relative inline-flex flex-1 items-center justify-center gap-1.5 whitespace-nowrap rounded-xs font-medium transition-colors duration-150',
              size === 'sm' ? 'h-6 px-2 text-xs' : 'h-[26px] px-2.5 text-sm',
              active ? 'text-ink' : 'text-ink-faint hover:text-ink-muted',
            )}
          >
            {active && (
              <motion.span
                layoutId={layoutId}
                transition={spring}
                className="absolute inset-0 rounded-xs bg-surface shadow-rest"
              />
            )}
            <span className="relative z-10 inline-flex items-center gap-1.5">
              {opt.icon}
              {opt.label}
            </span>
          </button>
        )
      })}
    </div>
  )
}

/** Underlined tabs for page-level sections. */
export function Tabs<T extends string>({
  value,
  options,
  onChange,
  className,
}: {
  value: T
  options: { value: T; label: ReactNode; count?: number }[]
  onChange: (v: T) => void
  className?: string
}) {
  const layoutId = useId()
  const { spring } = useMotion()

  return (
    <div className={cn('flex items-center gap-1', className)}>
      {options.map((opt) => {
        const active = opt.value === value
        return (
          <button
            key={opt.value}
            onClick={() => onChange(opt.value)}
            className={cn(
              'relative px-2.5 py-2 text-base font-medium transition-colors duration-150',
              active ? 'text-ink' : 'text-ink-faint hover:text-ink-muted',
            )}
          >
            <span className="inline-flex items-center gap-1.5">
              {opt.label}
              {opt.count !== undefined && (
                <span
                  className={cn(
                    'num rounded-xs px-1 text-2xs',
                    active ? 'bg-accent-soft text-accent-ink' : 'bg-surface-sunken text-ink-faint',
                  )}
                >
                  {opt.count}
                </span>
              )}
            </span>
            {active && (
              <motion.span
                layoutId={layoutId}
                transition={spring}
                className="absolute inset-x-1.5 -bottom-px h-[2px] rounded-full bg-accent"
              />
            )}
          </button>
        )
      })}
    </div>
  )
}
