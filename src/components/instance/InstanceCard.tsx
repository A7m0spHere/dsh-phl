import { memo } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { MoreHorizontal, Play, RotateCcw, Square, Star, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { formatRelative } from '@/lib/format'
import { hueTone } from '@/lib/hue'
import { useMotion } from '@/lib/motion'
import { useCatalogStore, useInstanceStore, useIsDark, useUIStore } from '@/stores'
import { LAUNCH_PHASE_LABEL, type Instance } from '@/types'
import { Button, Chip, EdgeProgress, IconButton, Menu, Tooltip } from '@/components/ui'
import { InstanceTile } from './InstanceTile'
import { StatusPill } from './StatusPill'
import { useInstanceActions } from './useInstanceActions'

interface Props {
  instance: Instance
  layout?: 'grid' | 'list'
}

/**
 * The card is the app's primary object. Its resting state is quiet — name,
 * environment, status — and it only reveals controls once the pointer is on
 * it, so a page of ten instances reads as ten facts rather than thirty
 * buttons.
 */
export const InstanceCard = memo(function InstanceCard({ instance, layout = 'grid' }: Props) {
  const state = useInstanceStore((s) => s.states[instance.id]) ?? { status: 'stopped' as const }
  const toggle = useInstanceStore((s) => s.toggle)
  const launch = useInstanceStore((s) => s.launch)
  const dismissError = useInstanceStore((s) => s.dismissError)
  const version = useCatalogStore((s) => s.versions.find((v) => v.id === instance.versionId))
  const runtime = useCatalogStore((s) => s.runtimes.find((r) => r.id === instance.runtimeId))
  const navigate = useUIStore((s) => s.navigate)
  const dark = useIsDark()
  const { t, riseItem, scale } = useMotion()
  const { menuItems } = useInstanceActions(instance)

  const tone = hueTone(instance.hue, dark)
  const status = state.status
  const busy = status === 'starting' || status === 'stopping'
  const running = status === 'running'
  const failed = status === 'error'
  const enabledPlugins = instance.plugins.filter((p) => p.enabled).length

  const primaryLabel = running ? '停止' : busy ? '取消' : failed ? '重试' : '启动'
  const PrimaryIcon = running ? Square : busy ? X : failed ? RotateCcw : Play

  const open = () => navigate({ name: 'instance', id: instance.id })

  return (
    <motion.article
      variants={riseItem}
      exit="out"
      layout={scale === 0 ? false : 'position'}
      transition={t(0.26)}
      onClick={open}
      whileHover={scale === 0 ? undefined : { y: -2 }}
      className={cn(
        'group/card relative cursor-pointer overflow-hidden rounded-lg bg-surface ring-1 ring-inset transition-[box-shadow,background-color] duration-200 ease-out',
        'hover:shadow-lift',
        failed ? 'ring-danger/25' : 'ring-line hover:ring-line-strong/70',
        layout === 'grid' ? 'p-2.5' : 'px-2.5 py-1.5',
      )}
    >
      {/* identity edge — grows in on hover, and stays lit while running */}
      <span
        aria-hidden
        className="absolute inset-y-0 left-0 w-[2.5px] origin-center scale-y-0 rounded-r-full transition-transform duration-300 ease-out group-hover/card:scale-y-100"
        style={{ background: tone.solid, transform: running ? 'scaleY(1)' : undefined }}
      />

      <div className={cn('flex gap-2.5', layout === 'grid' ? 'items-start' : 'items-center')}>
        <InstanceTile
          name={instance.name}
          hue={instance.hue}
          status={status}
          size={layout === 'grid' ? 30 : 26}
          layoutId={`tile-${instance.id}`}
        />

        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <h3 className="truncate text-base font-medium leading-4 text-ink">{instance.name}</h3>
            {instance.favorite && (
              <Star size={10} className="shrink-0 fill-warn text-warn" aria-label="已置顶" />
            )}
          </div>

          {/* The meta line is swapped for live phase text during a launch —
              one line that changes meaning, rather than a second line that
              pushes the layout around. */}
          <div className="relative mt-0.5 h-[16px]">
            <AnimatePresence mode="wait" initial={false}>
              {busy ? (
                <motion.div
                  key="phase"
                  initial={{ opacity: 0, y: 5 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -5 }}
                  transition={t(0.16)}
                  className="absolute inset-0 flex items-center gap-1.5 text-sm text-accent-ink"
                >
                  <span className="truncate">
                    {status === 'stopping'
                      ? '正在停止进程'
                      : LAUNCH_PHASE_LABEL[state.phase ?? 'resolve-version']}
                  </span>
                  <span className="num shrink-0 text-2xs text-ink-faint">
                    {Math.round((state.progress ?? 0) * 100)}%
                  </span>
                </motion.div>
              ) : failed ? (
                <motion.div
                  key="error"
                  initial={{ opacity: 0, y: 5 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -5 }}
                  transition={t(0.16)}
                  className="absolute inset-0 truncate text-sm text-danger"
                >
                  {state.error?.title}
                </motion.div>
              ) : (
                <motion.div
                  key="meta"
                  initial={{ opacity: 0, y: 5 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -5 }}
                  transition={t(0.16)}
                  className="absolute inset-0 flex items-center gap-1.5 truncate text-sm text-ink-muted"
                >
                  <span className="truncate">DSH {version?.name ?? instance.versionId}</span>
                  <span className="text-ink-faint/50">·</span>
                  <span className="shrink-0">{runtime?.name ?? instance.runtimeId}</span>
                  <span className="text-ink-faint/50">·</span>
                  <span className="shrink-0">{enabledPlugins} 插件</span>
                </motion.div>
              )}
            </AnimatePresence>
          </div>
        </div>

        {layout === 'list' && (
          <div className="hidden items-center gap-2 md:flex">
            <Chip>:{instance.port}</Chip>
            <span className="w-[84px] text-right text-sm text-ink-faint">
              {formatRelative(instance.lastRunAt)}
            </span>
          </div>
        )}

        <div className="flex shrink-0 items-center gap-1" onClick={(e) => e.stopPropagation()}>
          <StatusPill status={status} startedAt={state.startedAt} showClock={layout === 'list'} />

          <Button
            size="sm"
            variant={running ? 'secondary' : failed ? 'secondary' : 'primary'}
            onClick={() => (failed ? (dismissError(instance.id), launch(instance.id)) : toggle(instance.id))}
            className={cn(
              'group/sheen w-[54px] transition-opacity duration-200',
              // Controls stay out of the way until the card is engaged, but a
              // running instance always keeps its stop button reachable.
              !running && !busy && !failed && 'opacity-0 group-hover/card:opacity-100 focus:opacity-100',
            )}
            sheen={!running && !busy}
          >
            <PrimaryIcon size={10} className={cn(!running && !busy && !failed && 'fill-current')} />
            {primaryLabel}
          </Button>

          <Menu
            items={menuItems}
            trigger={({ open: isOpen, toggle: t2 }) => (
              <Tooltip content="更多操作" side="top">
                <IconButton
                  label="更多操作"
                  size="sm"
                  variant="ghost"
                  onClick={t2}
                  className={cn(
                    'transition-opacity duration-200',
                    !isOpen && 'opacity-0 group-hover/card:opacity-100 focus:opacity-100',
                  )}
                >
                  <MoreHorizontal size={14} />
                </IconButton>
              </Tooltip>
            )}
          />
        </div>
      </div>

      {/* Secondary row only exists in the grid layout, where there is room. */}
      {layout === 'grid' && (
        <div className="mt-2 flex items-center gap-2 border-t border-line pt-1.5 text-sm text-ink-faint">
          <Chip>:{instance.port}</Chip>
          {instance.autoPort && <span className="text-2xs text-ink-faint">自动分配</span>}
          <span className="ml-auto truncate">{formatRelative(instance.lastRunAt)}</span>
        </div>
      )}

      <AnimatePresence>
        {busy && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={t(0.16)}
          >
            <EdgeProgress
              value={state.progress ?? 0}
              indeterminate={status === 'stopping'}
              tone={status === 'stopping' ? 'warn' : 'accent'}
            />
          </motion.div>
        )}
      </AnimatePresence>
    </motion.article>
  )
})
