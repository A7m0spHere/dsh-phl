import { motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

export type ProgressTone = 'accent' | 'ok' | 'warn' | 'danger'

const TONES: Record<ProgressTone, string> = {
  accent: 'bg-accent',
  ok: 'bg-ok',
  warn: 'bg-warn',
  danger: 'bg-danger',
}

interface ProgressBarProps {
  /** 0..1. Ignored when `indeterminate`. */
  value?: number
  indeterminate?: boolean
  tone?: ProgressTone
  /** Track height in px. */
  height?: number
  className?: string
  /** Adds a travelling highlight — use only while work is actually moving. */
  active?: boolean
}

export function ProgressBar({
  value = 0,
  indeterminate = false,
  tone = 'accent',
  height = 4,
  className,
  active = false,
}: ProgressBarProps) {
  const { t } = useMotion()
  const pct = Math.max(0, Math.min(1, value)) * 100

  return (
    <div
      className={cn('relative w-full overflow-hidden rounded-full bg-ink/[0.08]', className)}
      style={{ height }}
      role="progressbar"
      aria-valuenow={indeterminate ? undefined : Math.round(pct)}
    >
      {indeterminate ? (
        <div className={cn('absolute inset-y-0 w-1/3 animate-shimmer rounded-full', TONES[tone])} />
      ) : (
        <motion.div
          className={cn('h-full rounded-full', TONES[tone])}
          initial={false}
          animate={{ width: `${pct}%` }}
          transition={t(0.32)}
        />
      )}
      {active && !indeterminate && (
        <div
          className="pointer-events-none absolute inset-y-0 left-0 overflow-hidden rounded-full"
          style={{ width: `${pct}%` }}
        >
          <div className="absolute inset-y-0 w-1/3 animate-shimmer bg-gradient-to-r from-transparent via-white/45 to-transparent" />
        </div>
      )}
    </div>
  )
}

/** A one-pixel bar that sits flush against a card edge while it works. */
export function EdgeProgress({
  value,
  indeterminate,
  tone = 'accent',
}: {
  value: number
  indeterminate?: boolean
  tone?: ProgressTone
}) {
  const { t } = useMotion()
  return (
    <div className="absolute inset-x-0 bottom-0 h-[2px] overflow-hidden rounded-b-lg bg-ink/[0.06]">
      {indeterminate ? (
        <div className={cn('h-full w-1/3 animate-shimmer', TONES[tone])} />
      ) : (
        <motion.div
          className={cn('h-full', TONES[tone])}
          initial={{ width: 0 }}
          animate={{ width: `${Math.min(100, value * 100)}%` }}
          transition={t(0.3)}
        />
      )}
    </div>
  )
}

/** Circular progress used by the title-bar transfer indicator. */
export function ProgressRing({
  value,
  size = 16,
  width = 2,
  className,
}: {
  value: number
  size?: number
  width?: number
  className?: string
}) {
  const r = (size - width) / 2
  const c = 2 * Math.PI * r
  return (
    <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className={cn(className)}>
      <circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        stroke="currentColor"
        strokeOpacity={0.18}
        strokeWidth={width}
      />
      <motion.circle
        cx={size / 2}
        cy={size / 2}
        r={r}
        fill="none"
        stroke="currentColor"
        strokeWidth={width}
        strokeLinecap="round"
        strokeDasharray={c}
        initial={false}
        animate={{ strokeDashoffset: c * (1 - Math.max(0, Math.min(1, value))) }}
        transition={{ duration: 0.3, ease: [0.22, 0.61, 0.36, 1] }}
        style={{ transformOrigin: 'center', transform: 'rotate(-90deg)' }}
      />
    </svg>
  )
}
