import { useId, type ReactNode } from 'react'
import { motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'

/**
 * The left column. It never scrolls the page — it holds the controls that
 * decide what the page shows, so it stays put while content moves.
 */
export function PanelShell({ children }: { children: ReactNode }) {
  return (
    <aside
      className="hairline-r flex shrink-0 flex-col overflow-y-auto bg-chrome/60"
      style={{ width: 'var(--panel-w)' }}
    >
      {children}
    </aside>
  )
}

export function PanelGroup({
  title,
  action,
  children,
  className,
}: {
  title?: ReactNode
  action?: ReactNode
  children: ReactNode
  className?: string
}) {
  return (
    <section className={cn('px-2 py-2', className)}>
      {title && (
        <div className="mb-1 flex items-center gap-1 px-1.5">
          <h4 className="text-2xs font-semibold uppercase tracking-wider text-ink-faint">{title}</h4>
          {action && <div className="ml-auto">{action}</div>}
        </div>
      )}
      {children}
    </section>
  )
}

interface PanelItemProps {
  icon?: ReactNode
  label: ReactNode
  count?: ReactNode
  active?: boolean
  onClick?: () => void
  /** Shared between siblings so the highlight slides between them. */
  groupId?: string
  trailing?: ReactNode
  className?: string
}

export function PanelItem({
  icon,
  label,
  count,
  active,
  onClick,
  groupId,
  trailing,
  className,
}: PanelItemProps) {
  const fallback = useId()
  const { spring, scale } = useMotion()
  return (
    <button
      onClick={onClick}
      className={cn(
        'relative flex w-full items-center gap-2 rounded-sm px-1.5 text-left text-sm transition-colors duration-150',
        active ? 'text-ink' : 'text-ink-muted hover:text-ink',
        className,
      )}
      style={{ height: 'var(--row-h)' }}
    >
      {active && (
        <motion.span
          layoutId={groupId ?? fallback}
          transition={scale === 0 ? { duration: 0 } : spring}
          className="absolute inset-0 rounded-sm bg-accent-soft"
        />
      )}
      {icon && (
        <span className={cn('relative z-10 shrink-0', active ? 'text-accent-ink' : 'text-ink-faint')}>
          {icon}
        </span>
      )}
      <span className={cn('relative z-10 min-w-0 flex-1 truncate', active && 'font-medium text-accent-ink')}>
        {label}
      </span>
      {count !== undefined && (
        <span
          className={cn(
            'num relative z-10 shrink-0 text-2xs',
            active ? 'text-accent-ink/80' : 'text-ink-faint',
          )}
        >
          {count}
        </span>
      )}
      {trailing && <span className="relative z-10 shrink-0">{trailing}</span>}
    </button>
  )
}

/** A compact figure used in the panel's summary blocks. */
export function PanelStat({
  label,
  value,
  tone = 'default',
}: {
  label: ReactNode
  value: ReactNode
  tone?: 'default' | 'ok' | 'accent'
}) {
  return (
    <div className="flex items-baseline justify-between px-1.5 py-[3px]">
      <span className="text-sm text-ink-faint">{label}</span>
      <span
        className={cn(
          'num text-sm font-medium',
          tone === 'ok' ? 'text-ok' : tone === 'accent' ? 'text-accent-ink' : 'text-ink',
        )}
      >
        {value}
      </span>
    </div>
  )
}

export function PanelDivider() {
  return <div className="mx-3 h-px bg-line" />
}
