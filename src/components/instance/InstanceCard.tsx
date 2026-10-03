import { memo } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { ArrowUpRight, MoreHorizontal, Play, RotateCcw, Square, Star, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { formatRelative } from '@/lib/format'
import { hueTone } from '@/lib/hue'
import { useMotion } from '@/lib/motion'
import { useCatalogStore, useInstanceStore, useIsDark, useUIStore } from '@/stores'
import { LAUNCH_PHASE_LABEL, type Instance } from '@/types'
import { Button, Chip, EdgeProgress, IconButton, Menu, Spinner, Tooltip } from '@/components/ui'
import { InstanceTile } from './InstanceTile'
import { STATUS_LABEL, StatusPill } from './StatusPill'
import { useInstanceActions } from './useInstanceActions'

interface Props {
  instance: Instance
  layout?: 'grid' | 'list'
}

/**
 * The card is the app's primary object. Its resting state is quiet — name,
 * environment, status — with visible controls so actions are discoverable
 * without hovering. Grid cards give identity and controls their own rows.
 *
 * Click semantics are the launcher's: single click *selects the launch
 * target* (the dock below always follows this focus), double click starts
 * (or stops) the instance, and the detail page is reached through the
 * explicit 详情 affordance — never as a side effect of aiming.
 *
 * Keyboard is the mouse's equal, not an afterthought: the card itself takes
 * focus (roving into it also moves the launch target), Enter opens the detail
 * page and Space runs the primary action, so the whole page is usable without
 * a pointer. The card is a `group` rather than a `button` because it contains
 * real buttons — nesting them inside a button role would misdescribe it.
 */
export const InstanceCard = memo(function InstanceCard({ instance, layout = 'grid' }: Props) {
  const state = useInstanceStore((s) => s.states[instance.id]) ?? { status: 'stopped' as const }
  const toggle = useInstanceStore((s) => s.toggle)
  const launch = useInstanceStore((s) => s.launch)
  const dismissError = useInstanceStore((s) => s.dismissError)
  const focused = useInstanceStore((s) => s.focusId === instance.id)
  const setFocus = useInstanceStore((s) => s.setFocus)
  // The backend is walking this instance's tree for deletion: the row must
  // say so (and stop offering actions) instead of sitting frozen until the
  // remove command finally lands.
  const deleting = useInstanceStore((s) => s.deleting[instance.id] === true)
  // The clone copies this row's tree (the new instance appears on success);
  // the source row shows the same work-in-progress treatment as a delete.
  const cloning = useInstanceStore((s) => s.cloning[instance.id] === true)
  const version = useCatalogStore((s) => s.versions.find((v) => v.id === instance.versionId))
  const runtime = useCatalogStore((s) => s.runtimes.find((r) => r.id === instance.runtimeId))
  const push = useUIStore((s) => s.push)
  const dark = useIsDark()
  const { t, riseItem, swap, scale } = useMotion()
  const { menuItems } = useInstanceActions(instance)

  const tone = hueTone(instance.hue, dark)
  const status = state.status
  const busy = status === 'starting' || status === 'stopping'
  const running = status === 'running'
  const failed = status === 'error'
  const enabledPlugins = instance.plugins.filter((p) => p.enabled).length

  const primaryLabel = running ? '停止' : busy ? '取消' : failed ? '重试' : '启动'
  const PrimaryIcon = running ? Square : busy ? X : failed ? RotateCcw : Play

  const openDetail = () => push({ name: 'instance', id: instance.id })
  const primaryAction = () => (failed ? (dismissError(instance.id), launch(instance.id)) : toggle(instance.id))

  return (
    <motion.article
      variants={riseItem}
      exit="out"
      layout={scale === 0 ? false : 'position'}
      transition={t(0.26)}
      tabIndex={0}
      aria-label={`${instance.name}，${STATUS_LABEL[status]}。回车查看详情，空格${primaryLabel}。`}
      data-card
      onClick={() => setFocus(instance.id)}
      onFocus={() => setFocus(instance.id)}
      onDoubleClick={primaryAction}
      onKeyDown={(e) => {
        // Let the revealed controls keep their own keys; only act when the
        // card itself holds focus.
        if (e.target !== e.currentTarget) return
        if (e.key === 'Enter') {
          e.preventDefault()
          openDetail()
        } else if (e.key === ' ') {
          e.preventDefault()
          primaryAction()
        }
      }}
      whileHover={scale === 0 ? undefined : { y: -2 }}
      className={cn(
        'group/card relative cursor-pointer overflow-hidden rounded-lg bg-surface ring-1 ring-inset transition-[box-shadow,background-color] duration-200 ease-out',
        'hover:shadow-lift',
        failed
          ? 'ring-danger/25'
          : focused
            ? 'ring-accent/45'
            : 'ring-line hover:ring-line-strong/70',
        layout === 'grid' ? 'p-2.5' : 'px-2.5 py-1.5',
      )}
    >
      {/* identity edge — grows in on hover, and stays lit while running or focused */}
      <span
        aria-hidden
        className="absolute inset-y-0 left-0 w-[2.5px] origin-center scale-y-0 rounded-r-full transition-transform duration-300 ease-out group-hover/card:scale-y-100"
        style={{ background: tone.solid, transform: running || focused ? 'scaleY(1)' : undefined }}
      />

      <div className={cn('flex gap-2.5', layout === 'grid' ? 'flex-wrap items-start' : 'items-center')}>
        <InstanceTile
          name={instance.name}
          hue={instance.hue}
          status={status}
          size={layout === 'grid' ? 30 : 26}
          layoutId={`tile-${instance.id}`}
        />

        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <h3 className="truncate text-md font-medium leading-4 text-ink">{instance.name}</h3>
            {instance.favorite && (
              <Star size={10} className="shrink-0 fill-warn text-warn" aria-label="已置顶" />
            )}
          </div>

          {/* The meta line is swapped for live phase text during a launch —
              one line that changes meaning, rather than a second line that
              pushes the layout around. */}
          <div className="relative mt-0.5 h-[16px]">
            <AnimatePresence mode="wait" initial={false}>
              {deleting || cloning ? (
                <motion.div
                  key="deleting"
                  variants={swap}
                  initial="hidden"
                  animate="show"
                  exit="out"
                  className="absolute inset-0 flex items-center gap-1.5 text-sm text-warn"
                >
                  <Spinner size={11} weight={2.6} />
                  <span className="truncate">{deleting ? '正在删除实例目录…' : '正在克隆到新实例…'}</span>
                </motion.div>
              ) : busy ? (
                <motion.div
                  key="phase"
                  variants={swap}
                  initial="hidden"
                  animate="show"
                  exit="out"
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
                  variants={swap}
                  initial="hidden"
                  animate="show"
                  exit="out"
                  className="absolute inset-0 truncate text-sm text-danger"
                >
                  {state.error?.title}
                </motion.div>
              ) : (
                <motion.div
                  key="meta"
                  variants={swap}
                  initial="hidden"
                  animate="show"
                  exit="out"
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
          <div className="hidden items-center gap-2 lg:flex">
            <Chip>:{instance.port}</Chip>
            <span className="w-[84px] text-right text-sm text-ink-faint">
              {formatRelative(instance.lastRunAt)}
            </span>
          </div>
        )}

        {deleting ? (
          <span className="flex shrink-0 items-center gap-1.5 text-sm text-ink-faint">
            <Spinner size={12} weight={2.6} />
            删除中…
          </span>
        ) : (
        <div
          className={cn(
            'flex shrink-0 items-center gap-1',
            // flex-wrap：窄卡片叠加错误徽章时让图标换行，而不是被 overflow-hidden 裁掉。
            layout === 'grid' && 'w-full flex-wrap border-t border-line pt-2',
          )}
          onClick={(e) => e.stopPropagation()}
          onDoubleClick={(e) => e.stopPropagation()}
        >
          <StatusPill status={status} startedAt={state.startedAt} showClock />

          {/* A crash's toast is gone in seconds; the fact should outlive it
              until the next launch replaces the runtime state. */}
          {state.lastExit && (
            <Tooltip
              content={`进程异常退出（退出码 ${state.lastExit.code ?? '未知'}，运行 ${state.lastExit.ranFor}s）；日志在实例目录 logs/ 下，下次启动后消失`}
            >
              <span className="num shrink-0 rounded-xs border border-danger/40 px-1.5 py-0.5 text-2xs text-danger">
                上次退出 {state.lastExit.code ?? '?'}
              </span>
            </Tooltip>
          )}

          {/* Same lifetime as `lastExit`, different failure: the process is
              alive but its WebUI page did not boot (a plugin never
              activated). Without this the pill would read 运行中 over a UI
              that shows only a failure card. */}
          {state.lastWebuiError && (
            <Tooltip
              content={
                <div className="max-w-[320px] space-y-1 text-left">
                  <div className="font-medium">{state.lastWebuiError.headline}（进程仍在运行）</div>
                  {state.lastWebuiError.causes.map((c, i) => (
                    <div key={i} className="text-ink-muted">
                      {c}
                    </div>
                  ))}
                  <div className="text-ink-faint">{state.lastWebuiError.hint}</div>
                </div>
              }
            >
              <span className="num shrink-0 rounded-xs border border-danger/40 px-1.5 py-0.5 text-2xs text-danger">
                页面未加载
              </span>
            </Tooltip>
          )}

          <Button
            size="sm"
            variant={running ? 'secondary' : failed ? 'secondary' : 'primary'}
            onClick={primaryAction}
            className={cn(
              'group/sheen w-[54px]',
              layout === 'grid' && 'ml-auto',
            )}
            sheen={!running && !busy}
          >
            <PrimaryIcon size={10} className={cn(!running && !busy && !failed && 'fill-current')} />
            {primaryLabel}
          </Button>

          <Tooltip content="查看详情" side="top">
            <IconButton
              label="查看详情"
              size="sm"
              variant="ghost"
              onClick={openDetail}
            >
              <ArrowUpRight size={14} />
            </IconButton>
          </Tooltip>

          <Menu
            items={menuItems}
            trigger={({ toggle: t2, menuProps }) => (
              <Tooltip content="更多操作" side="top">
                <IconButton
                  label="更多操作"
                  size="sm"
                  variant="ghost"
                  onClick={t2}
                  {...menuProps}
                >
                  <MoreHorizontal size={14} />
                </IconButton>
              </Tooltip>
            )}
          />
        </div>
        )}
      </div>

      {/* Secondary row only exists in the grid layout, where there is room. */}
      {layout === 'grid' && (
        <div className="mt-2 flex items-center gap-2 text-sm text-ink-faint">
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
