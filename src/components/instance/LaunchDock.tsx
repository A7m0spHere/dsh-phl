import { useMemo } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { ChevronUp, ExternalLink, Play, RotateCcw, Square, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { formatClock } from '@/lib/format'
import { useUptime } from '@/lib/hooks'
import { useMotion } from '@/lib/motion'
import { useCatalogStore, useInstanceStore, useUIStore } from '@/stores'
import { LAUNCH_PHASE_LABEL, LAUNCH_PHASES } from '@/types'
import { Button, Chip, IconButton, Menu, ProgressBar, Tooltip } from '@/components/ui'
import { InstanceTile } from './InstanceTile'
import { useInstanceActions } from './useInstanceActions'

/**
 * A launcher earns its name at the bottom of the window: one target, one
 * button, always in the same place. Everything else on the page is a way of
 * choosing what this button points at.
 */
export function LaunchDock() {
  const instances = useInstanceStore((s) => s.instances)
  const states = useInstanceStore((s) => s.states)
  const focusId = useInstanceStore((s) => s.focusId)
  const setFocus = useInstanceStore((s) => s.setFocus)
  const toggle = useInstanceStore((s) => s.toggle)
  const launch = useInstanceStore((s) => s.launch)
  const dismissError = useInstanceStore((s) => s.dismissError)
  const versions = useCatalogStore((s) => s.versions)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const push = useUIStore((s) => s.push)
  const { t, scale } = useMotion()

  // Prefer whatever is running; otherwise the most recently used instance.
  const target = useMemo(() => {
    if (focusId) {
      const pinned = instances.find((i) => i.id === focusId)
      if (pinned) return pinned
    }
    const running = instances.find((i) => states[i.id]?.status === 'running')
    if (running) return running
    return (
      [...instances].sort(
        (a, b) => new Date(b.lastRunAt ?? 0).getTime() - new Date(a.lastRunAt ?? 0).getTime(),
      )[0] ?? null
    )
  }, [instances, states, focusId])

  const { openWebUI } = useInstanceActions(target ?? undefined)
  const state = target ? (states[target.id] ?? { status: 'stopped' as const }) : null
  const uptime = useUptime(state?.status === 'running' ? state.startedAt : undefined)

  if (!target || !state) return null

  const version = versions.find((v) => v.id === target.versionId)
  const runtime = runtimes.find((r) => r.id === target.runtimeId)
  const status = state.status
  const busy = status === 'starting' || status === 'stopping'
  const running = status === 'running'
  const failed = status === 'error'

  const phaseIndex = state.phase ? LAUNCH_PHASES.indexOf(state.phase) : -1

  return (
    <motion.div
      initial={{ y: scale === 0 ? 0 : 24, opacity: 0 }}
      animate={{ y: 0, opacity: 1 }}
      transition={t(0.34)}
      className="relative z-20 shrink-0 border-t border-line bg-chrome/85 shadow-dock backdrop-blur-xl"
    >
      {/* launch progress runs along the very top edge of the dock */}
      <AnimatePresence>
        {busy && (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="absolute inset-x-0 -top-px"
          >
            <ProgressBar
              value={state.progress ?? 0}
              indeterminate={status === 'stopping'}
              tone={status === 'stopping' ? 'warn' : 'accent'}
              height={2}
              active
              className="rounded-none bg-transparent"
            />
          </motion.div>
        )}
      </AnimatePresence>

      <div className="flex items-center gap-2.5 px-[var(--page-pad)] py-2">
        <button
          onClick={() => push({ name: 'instance', id: target.id })}
          className="group flex min-w-0 flex-1 items-center gap-2.5 rounded-lg py-0.5 pr-2 text-left"
        >
          <InstanceTile name={target.name} hue={target.hue} status={status} size={32} />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate text-base font-medium text-ink transition-colors group-hover:text-accent-ink">
                {target.name}
              </span>
              {running && (
                <span className="num text-2xs text-ink-faint">{formatClock(uptime)}</span>
              )}
            </div>

            <div className="relative mt-0.5 h-[16px]">
              <AnimatePresence mode="wait" initial={false}>
                <motion.div
                  key={busy ? `phase-${state.phase}` : failed ? 'error' : 'meta'}
                  initial={{ opacity: 0, y: 4 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -4 }}
                  transition={t(0.15)}
                  className="absolute inset-0 flex items-center gap-1.5 text-sm"
                >
                  {busy ? (
                    <span className="flex items-center gap-1.5 text-accent-ink">
                      <span className="num text-2xs text-ink-faint">
                        {phaseIndex >= 0 ? `${phaseIndex + 1}/${LAUNCH_PHASES.length}` : ''}
                      </span>
                      {status === 'stopping'
                        ? '正在停止进程'
                        : LAUNCH_PHASE_LABEL[state.phase ?? 'resolve-version']}
                    </span>
                  ) : failed ? (
                    <span className="truncate text-danger">{state.error?.title}</span>
                  ) : (
                    <span className="flex items-center gap-1.5 truncate text-ink-muted">
                      DSH {version?.name}
                      <span className="text-ink-faint/50">·</span>
                      {runtime?.name}
                      <span className="text-ink-faint/50">·</span>
                      <Chip>:{target.port}</Chip>
                    </span>
                  )}
                </motion.div>
              </AnimatePresence>
            </div>
          </div>
        </button>

        {running && (
          <Tooltip content={`localhost:${target.port}`}>
            <Button variant="secondary" size="md" onClick={openWebUI}>
              <ExternalLink size={12} />
              打开 WebUI
            </Button>
          </Tooltip>
        )}

        <div className="group/sheen flex items-stretch">
          <Button
            size="hero"
            variant={running ? 'secondary' : 'primary'}
            sheen={!running && !busy}
            onClick={() => {
              if (failed) {
                dismissError(target.id)
                void launch(target.id)
              } else toggle(target.id)
            }}
            className={cn('min-w-[112px] rounded-r-none', running && 'min-w-[92px]')}
          >
            {running ? (
              <>
                <Square size={12} /> 停止
              </>
            ) : busy ? (
              <>
                <X size={12} /> 取消启动
              </>
            ) : failed ? (
              <>
                <RotateCcw size={12} /> 重试
              </>
            ) : (
              <>
                <Play size={12} className="fill-current" /> 启动
              </>
            )}
          </Button>

          <Menu
            width={252}
            side="top"
            items={instances.map((i) => ({
              id: i.id,
              label: (
                <span className="flex items-center gap-2">
                  <span className="truncate">{i.name}</span>
                  {states[i.id]?.status === 'running' && (
                    <span className="ml-auto h-[6px] w-[6px] shrink-0 rounded-full bg-ok" />
                  )}
                </span>
              ),
              shortcut: `:${i.port}`,
              onSelect: () => setFocus(i.id),
            }))}
            trigger={({ toggle: openMenu, menuProps }) => (
              <IconButton
                label="切换启动目标"
                size="hero"
                variant={running ? 'secondary' : 'primary'}
                onClick={openMenu}
                {...menuProps}
                className="h-9 w-7 rounded-l-none border-l border-black/10 dark:border-white/10"
              >
                <ChevronUp size={13} />
              </IconButton>
            )}
          />
        </div>
      </div>
    </motion.div>
  )
}
