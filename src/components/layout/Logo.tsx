import { cn } from '@/lib/cn'

/**
 * PHL's mark: three offset bars inside a rounded square — several isolated
 * environments stacked in one container. Drawn here rather than shipped as an
 * asset so it inherits the accent colour and stays crisp at any size.
 */
export function Logo({ size = 20, className }: { size?: number; className?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      className={cn('shrink-0', className)}
      aria-label="PHL"
    >
      <rect x="1.5" y="1.5" width="21" height="21" rx="6" className="fill-accent" />
      <rect x="6" y="6.5" width="12" height="3" rx="1.5" fill="hsl(var(--c-accent-on))" opacity="0.95" />
      <rect x="6" y="11" width="8.5" height="3" rx="1.5" fill="hsl(var(--c-accent-on))" opacity="0.7" />
      <rect x="6" y="15.5" width="5" height="3" rx="1.5" fill="hsl(var(--c-accent-on))" opacity="0.45" />
    </svg>
  )
}
