import { motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { hueTone, initials } from '@/lib/hue'
import { useMotion } from '@/lib/motion'
import { useIsDark } from '@/stores/uiStore'
import type { InstanceStatus } from '@/types'
import { Spinner } from '@/components/ui'

/**
 * The instance's identity mark. It is the element that persists across the
 * list, the detail header and the launch dock, so the eye can follow one
 * instance through a navigation instead of re-finding it.
 */
export function InstanceTile({
  name,
  hue,
  status = 'stopped',
  size = 38,
  className,
  layoutId,
}: {
  name: string
  hue: number
  status?: InstanceStatus
  size?: number
  className?: string
  layoutId?: string
}) {
  const dark = useIsDark()
  const tone = hueTone(hue, dark)
  const { t, springSoft } = useMotion()
  const busy = status === 'starting' || status === 'stopping'

  return (
    <motion.div
      layoutId={layoutId}
      // A shared element is a spring everywhere else in the app (segmented
      // pills, nav marker, accent ring); the tile — the one element the eye
      // follows from list → detail → dock — must agree, or its flight reads
      // as a different interaction. springSoft is the gentler end so a tile
      // this size settles without overshooting, and it snaps at motion=off.
      transition={springSoft}
      className={cn('relative shrink-0 select-none', className)}
      style={{ width: size, height: size }}
    >
      <div
        className="flex h-full w-full items-center justify-center rounded-lg font-medium tracking-tight"
        style={{
          background: tone.soft,
          color: tone.text,
          boxShadow: `inset 0 0 0 1px ${tone.ring}`,
          fontSize: size * 0.36,
        }}
      >
        {busy ? <Spinner size={size * 0.44} weight={2.2} /> : initials(name)}
      </div>

      {status === 'running' && (
        <motion.span
          initial={{ scale: 0.6, opacity: 0 }}
          animate={{ scale: 1, opacity: 1 }}
          transition={t(0.24)}
          className="absolute -bottom-[2px] -right-[2px] flex h-[11px] w-[11px] items-center justify-center rounded-full bg-surface"
        >
          <span className="h-[7px] w-[7px] rounded-full bg-ok" />
          <span className="absolute h-[7px] w-[7px] animate-halo rounded-full bg-ok" />
        </motion.span>
      )}
      {status === 'error' && (
        <span className="absolute -bottom-[2px] -right-[2px] flex h-[11px] w-[11px] items-center justify-center rounded-full bg-surface">
          <span className="h-[7px] w-[7px] rounded-full bg-danger" />
        </span>
      )}
    </motion.div>
  )
}
