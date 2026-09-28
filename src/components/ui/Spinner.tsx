import { cn } from '@/lib/cn'

interface SpinnerProps {
  size?: number
  className?: string
  /** Stroke width relative to the box; defaults to a hairline that stays crisp. */
  weight?: number
}

/**
 * An arc that sweeps and re-coils as it spins. A fixed-length arc reads as a
 * timer; a breathing one reads as work in progress, which is what a launcher
 * is almost always doing.
 */
export function Spinner({ size = 14, className, weight = 2 }: SpinnerProps) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      fill="none"
      className={cn('shrink-0', className)}
      aria-hidden
    >
      <circle cx="10" cy="10" r="8" stroke="currentColor" strokeOpacity="0.16" strokeWidth={weight} />
      <circle
        cx="10"
        cy="10"
        r="8"
        stroke="currentColor"
        strokeWidth={weight}
        strokeLinecap="round"
        strokeDasharray="52"
        className="animate-arc"
        style={{ transformOrigin: '10px 10px' }}
      />
    </svg>
  )
}
