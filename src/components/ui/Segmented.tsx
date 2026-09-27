import { type ReactNode, useId } from 'react'
import { motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'
import { Tooltip } from './Tooltip'

export interface SegmentedOption<T extends string> {
  value: T
  label: ReactNode
  icon?: ReactNode
  /**
   * Hint for an icon-only option (empty `label`): it becomes the accessible
   * name AND the hover bubble — the same unified Tooltip every other hint in
   * the app uses. A native `title` used to serve this and doubled up with a
   * styled bubble on the same control (2026-09-13 review: the instances
   * view toggle had a `title` while everything else had a bubble).
   */
  hint?: string
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
  disabled,
}: {
  value: T
  options: SegmentedOption<T>[]
  onChange: (v: T) => void
  size?: 'sm' | 'md'
  className?: string
  block?: boolean
  disabled?: boolean
}) {
  const layoutId = useId()
  const { spring } = useMotion()

  return (
    <div
      role="tablist"
      className={cn(
        'inline-flex items-center gap-0.5 rounded bg-surface-sunken p-[3px] ring-1 ring-inset ring-line',
        block && 'w-full',
        disabled && 'pointer-events-none opacity-45',
        className,
      )}
    >
      {options.map((opt) => {
        const active = opt.value === value
        const button = (
          <button
            role="tab"
            aria-selected={active}
            aria-label={opt.hint}
            disabled={disabled}
            onClick={() => onChange(opt.value)}
            className={cn(
              'relative inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-xs font-medium transition-colors duration-150',
              // A hinted option is wrapped in the Tooltip anchor, which takes
              // the flex slot — the button then fills it instead of growing.
              opt.hint ? 'w-full' : 'flex-1',
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
        return opt.hint ? (
          <Tooltip key={opt.value} content={opt.hint} className="flex-1">
            {button}
          </Tooltip>
        ) : (
          <span key={opt.value} className="contents">
            {button}
          </span>
        )
      })}
    </div>
  )
}
