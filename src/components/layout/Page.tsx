import type { ReactNode } from 'react'
import { motion } from 'motion/react'
import { cn } from '@/lib/cn'
import { useTruncated } from '@/lib/hooks'
import { useMotion } from '@/lib/motion'
import { Tooltip } from '@/components/ui'

/**
 * Standard page frame: a header that does not scroll, a body that does.
 * Keeping the header fixed means the page title and its primary action stay
 * available no matter how long the list gets.
 */
export function PageShell({
  title,
  subtitle,
  actions,
  toolbar,
  children,
  bodyClassName,
  maxWidth = 900,
}: {
  title: ReactNode
  subtitle?: ReactNode
  actions?: ReactNode
  toolbar?: ReactNode
  children: ReactNode
  bodyClassName?: string
  maxWidth?: number
}) {
  const { t, scale } = useMotion()
  // On a scaled window the header actions would squeeze the subtitle into a
  // broken sliver. min() lets the column fall back to the full content width
  // while large screens keep the reading measure.
  const columnStyle = { maxWidth: `min(${maxWidth}px, 100%)` }
  return (
    <div className="flex h-full min-h-0 flex-col">
      <motion.header
        initial={{ opacity: 0, y: scale === 0 ? 0 : -6 }}
        animate={{ opacity: 1, y: 0 }}
        transition={t(0.24)}
        className="shrink-0 px-[var(--page-pad)] pb-2 pt-3"
      >
        <div className="mx-auto w-full" style={columnStyle}>
          {/* grow-1 + a large flex-basis act as the wrap threshold: the title
              block demands ~30rem before it will share a row, so a narrow /
              zoomed column pushes the action buttons onto their own line
              instead of squeezing the subtitle into a broken sliver. */}
          <div className="flex flex-wrap items-start gap-x-4 gap-y-2">
            <div className="min-w-0 grow basis-[30rem]">
              <h1 className="text-lg font-semibold tracking-tight text-ink">{title}</h1>
              {subtitle && <PageSubtitle subtitle={subtitle} />}
            </div>
            {actions && (
              <div className="ml-auto flex max-w-full flex-wrap items-center justify-end gap-1.5">{actions}</div>
            )}
          </div>
          {toolbar && <div className="mt-2.5 flex items-center gap-2">{toolbar}</div>}
        </div>
      </motion.header>

      <div className={cn('min-h-0 flex-1 overflow-y-auto px-[var(--page-pad)] pb-6', bodyClassName)}>
        <div className="mx-auto w-full" style={columnStyle}>
          {children}
        </div>
      </div>
    </div>
  )
}

/**
 * The page subtitle is clamped to two lines; when the column is too narrow
 * to show it all, the full text surfaces in a Tooltip. A native `title` used
 * to fire unconditionally — a bubble repeating text that was already fully
 * readable.
 */
function PageSubtitle({ subtitle }: { subtitle: ReactNode }) {
  const [ref, truncated] = useTruncated<HTMLParagraphElement>()
  return (
    <Tooltip content={truncated ? subtitle : ''}>
      <p ref={ref} className="mt-0.5 line-clamp-2 text-sm leading-relaxed text-ink-muted">
        {subtitle}
      </p>
    </Tooltip>
  )
}

/** A titled band inside a page body. */
export function PageSection({
  title,
  description,
  action,
  children,
  className,
}: {
  title?: ReactNode
  description?: ReactNode
  action?: ReactNode
  children: ReactNode
  className?: string
}) {
  return (
    <section className={cn('mb-4 last:mb-0', className)}>
      {(title || action) && (
        <div className="mb-2 flex items-baseline gap-3">
          <div className="min-w-0">
            {title && <h2 className="text-base font-medium text-ink">{title}</h2>}
            {description && <p className="mt-0.5 text-sm text-ink-faint">{description}</p>}
          </div>
          {action && <div className="ml-auto shrink-0">{action}</div>}
        </div>
      )}
      {children}
    </section>
  )
}
