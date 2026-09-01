import type { ReactNode } from 'react'
import { motion } from 'motion/react'
import { AlertTriangle, Info, TriangleAlert } from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

/**
 * Empty states carry the same weight as populated ones: they explain what the
 * area is for and offer the one action that fills it.
 */
export function EmptyState({
  icon,
  title,
  description,
  action,
  className,
  compact,
}: {
  icon?: ReactNode
  title: string
  description?: ReactNode
  action?: ReactNode
  className?: string
  compact?: boolean
}) {
  const { t } = useMotion()
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={t(0.3)}
      className={cn(
        'flex flex-col items-center justify-center text-center',
        compact ? 'py-8' : 'py-16',
        className,
      )}
    >
      {icon && (
        <div className="mb-3.5 flex h-11 w-11 items-center justify-center rounded-xl bg-surface-sunken text-ink-faint ring-1 ring-inset ring-line">
          {icon}
        </div>
      )}
      <div className="text-md font-medium text-ink">{title}</div>
      {description && (
        <div className="mt-1.5 max-w-[380px] text-base leading-relaxed text-ink-faint">
          {description}
        </div>
      )}
      {action && <div className="mt-5">{action}</div>}
    </motion.div>
  )
}

export type NoticeTone = 'info' | 'warn' | 'danger'

const NOTICE: Record<NoticeTone, { cls: string; icon: ReactNode }> = {
  info: { cls: 'bg-info/[0.07] text-info ring-info/20', icon: <Info size={14} /> },
  warn: { cls: 'bg-warn/[0.09] text-warn ring-warn/25', icon: <TriangleAlert size={14} /> },
  danger: { cls: 'bg-danger/[0.07] text-danger ring-danger/20', icon: <AlertTriangle size={14} /> },
}

export function Notice({
  tone = 'info',
  title,
  children,
  action,
  className,
}: {
  tone?: NoticeTone
  title?: ReactNode
  children?: ReactNode
  action?: ReactNode
  className?: string
}) {
  const spec = NOTICE[tone]
  return (
    <div
      className={cn(
        'flex gap-2.5 rounded-lg px-3 py-2.5 ring-1 ring-inset',
        spec.cls,
        className,
      )}
    >
      <span className="mt-[2px] shrink-0">{spec.icon}</span>
      <div className="min-w-0 flex-1">
        {title && <div className="text-base font-medium">{title}</div>}
        {children && (
          <div className={cn('text-sm leading-relaxed text-ink-muted', title && 'mt-1')}>
            {children}
          </div>
        )}
      </div>
      {action && <div className="ml-auto shrink-0 self-center">{action}</div>}
    </div>
  )
}

/** Placeholder rows while the repository resolves. */
export function Skeleton({ className }: { className?: string }) {
  return (
    <div className={cn('relative overflow-hidden rounded bg-ink/[0.06]', className)}>
      <div className="absolute inset-0 animate-shimmer bg-gradient-to-r from-transparent via-ink/[0.05] to-transparent" />
    </div>
  )
}
