import { motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { formatClock } from '@/lib/format'
import { useUptime } from '@/lib/hooks'
import { useMotion } from '@/lib/motion'
import type { InstanceStatus } from '@/types'
import { Spinner } from '@/components/ui'

const LABEL: Record<InstanceStatus, string> = {
  stopped: '已停止',
  starting: '启动中',
  running: '运行中',
  stopping: '停止中',
  error: '启动失败',
}

const TONE: Record<InstanceStatus, string> = {
  stopped: 'text-ink-faint',
  starting: 'text-accent-ink',
  running: 'text-ok',
  stopping: 'text-warn',
  error: 'text-danger',
}

const DOT: Record<InstanceStatus, string> = {
  stopped: 'bg-ink-faint/50',
  starting: 'bg-accent',
  running: 'bg-ok',
  stopping: 'bg-warn',
  error: 'bg-danger',
}

/**
 * A running instance gets a slow halo behind its dot. It is the only ambient
 * animation in the list, which is what makes "this one is live" readable at a
 * glance without reading any text.
 */
export function StatusDot({ status, size = 7 }: { status: InstanceStatus; size?: number }) {
  return (
    <span className="relative inline-flex shrink-0 items-center justify-center" style={{ width: size, height: size }}>
      {status === 'running' && (
        <span className="absolute inset-0 animate-halo rounded-full bg-ok" aria-hidden />
      )}
      <span className={cn('relative rounded-full', DOT[status])} style={{ width: size, height: size }} />
    </span>
  )
}

export function StatusPill({
  status,
  startedAt,
  className,
  showClock = false,
}: {
  status: InstanceStatus
  startedAt?: number
  className?: string
  showClock?: boolean
}) {
  const uptime = useUptime(status === 'running' ? startedAt : undefined)
  const { t } = useMotion()

  return (
    <motion.span
      layout="position"
      transition={t(0.2)}
      className={cn(
        'inline-flex h-[19px] items-center gap-1 rounded-full bg-surface-sunken px-1.5 text-2xs font-medium ring-1 ring-inset ring-line',
        TONE[status],
        className,
      )}
    >
      {status === 'starting' || status === 'stopping' ? (
        <Spinner size={10} weight={2.6} />
      ) : (
        <StatusDot status={status} size={5} />
      )}
      {LABEL[status]}
      {showClock && status === 'running' && startedAt && (
        <span className="num text-2xs tabular-nums text-ink-faint">{formatClock(uptime)}</span>
      )}
    </motion.span>
  )
}
