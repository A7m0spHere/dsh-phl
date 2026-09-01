import type { ReactNode } from 'react'
import { cn } from '@/lib/cn'

export type BadgeTone = 'neutral' | 'accent' | 'ok' | 'warn' | 'danger' | 'outline'

const TONES: Record<BadgeTone, string> = {
  neutral: 'bg-surface-sunken text-ink-muted ring-line',
  accent: 'bg-accent-soft text-accent-ink ring-accent/20',
  ok: 'bg-ok/10 text-ok ring-ok/20',
  warn: 'bg-warn/12 text-warn ring-warn/25',
  danger: 'bg-danger/10 text-danger ring-danger/20',
  outline: 'text-ink-faint ring-line',
}

export function Badge({
  tone = 'neutral',
  children,
  className,
  icon,
}: {
  tone?: BadgeTone
  children: ReactNode
  className?: string
  icon?: ReactNode
}) {
  return (
    <span
      className={cn(
        'inline-flex h-[19px] items-center gap-1 rounded-xs px-1.5 text-2xs font-medium ring-1 ring-inset',
        TONES[tone],
        className,
      )}
    >
      {icon}
      {children}
    </span>
  )
}

/** Monospace chip for ports, pids and versions. */
export function Chip({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'num inline-flex h-[19px] items-center rounded-xs bg-surface-sunken px-1.5 font-mono text-2xs text-ink-muted ring-1 ring-inset ring-line',
        className,
      )}
    >
      {children}
    </span>
  )
}

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="inline-flex h-[18px] min-w-[18px] items-center justify-center rounded-xs bg-surface-sunken px-1 font-sans text-2xs text-ink-faint ring-1 ring-inset ring-line">
      {children}
    </kbd>
  )
}

export function Dot({ className }: { className?: string }) {
  return <span className={cn('mx-1.5 inline-block text-ink-faint/60', className)}>·</span>
}
