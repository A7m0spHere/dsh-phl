import { useEffect, useRef } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { AlertTriangle, CircleAlert, CircleCheck, ListTodo, Loader2, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { cancelTransfer, desktop, type TaskInfo } from '@/lib/desktop'
import { parseThrownError } from '@/lib/errorCodes'
import { useMotion } from '@/lib/motion'
import { ProgressBar } from '@/components/ui'
import { useTaskStore } from '@/stores/taskStore'
import { useUIStore } from '@/stores/uiStore'
import { kindLabel, phaseText } from './taskLabels'

function elapsed(t: TaskInfo): string {
  const end = t.finishedAt ?? Date.now()
  const s = Math.max(0, Math.round((end - t.startedAt) / 1000))
  if (s < 60) return `${s} 秒`
  if (s < 3600) return `${Math.floor(s / 60)} 分 ${s % 60 ? `${s % 60} 秒` : ''}`.trim()
  return `${Math.floor(s / 3600)} 小时`
}

/**
 * The cross-page long-task surface (roadmap O-10): everything `guarded()`
 * in Rust registers shows up here — queued work is the transfer-slot queue's
 * `queued` states on each page, while running tasks carry their backend
 * phase. Failed rows keep the stable error code's hint (O-09) visible.
 */
export function TaskCenter() {
  const tasks = useTaskStore((s) => s.tasks)
  const open = useTaskStore((s) => s.open)
  const setOpen = useTaskStore((s) => s.setOpen)
  const refresh = useTaskStore((s) => s.refresh)
  const navigate = useUIStore((s) => s.navigate)
  const { t } = useMotion()
  const rootRef = useRef<HTMLDivElement>(null)

  const running = tasks.filter((x) => x.state === 'running')
  const settled = tasks.filter((x) => x.state !== 'running').slice(0, 12)
  // One latest-failure per kind, from the store's own helper — it is the
  // retry entry point and, surfaced as a badge, the reason the centre is worth
  // opening after a toast has already faded (a failed launch/install/delete is
  // otherwise invisible once the transient toast clears).
  const failedCount = useTaskStore((s) => s.recentFailures().length)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false)
    }
    window.addEventListener('mousedown', onDown)
    return () => window.removeEventListener('mousedown', onDown)
  }, [open, setOpen])

  return (
    <div ref={rootRef} className="no-drag relative">
      <button
        aria-label="任务中心"
        aria-expanded={open}
        onClick={() => {
          setOpen(!open)
          void refresh()
        }}
        className={cn(
          'relative flex h-6 items-center gap-1.5 rounded-sm px-2 text-2xs font-medium transition-colors',
          open ? 'bg-surface-hover text-ink' : 'text-ink-faint hover:bg-surface-hover hover:text-ink',
        )}
      >
        {running.length > 0 ? <Loader2 size={12} className="animate-spin text-accent" /> : failedCount > 0 ? <AlertTriangle size={12} className="text-danger" /> : <ListTodo size={12} />}
        任务
        {running.length > 0 ? (
          <span className="flex h-[14px] min-w-[14px] items-center justify-center rounded-full bg-accent-soft px-1 font-semibold text-accent-ink">
            {running.length}
          </span>
        ) : failedCount > 0 ? (
          <span
            aria-label={`${failedCount} 个失败任务`}
            className="flex h-[14px] min-w-[14px] items-center justify-center rounded-full bg-danger/15 px-1 font-semibold text-danger"
          >
            {failedCount}
          </span>
        ) : null}
      </button>

      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, y: -4, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -4, scale: 0.98 }}
            transition={t(0.16)}
            className="absolute right-0 top-[calc(100%+6px)] z-50 w-[380px] overflow-hidden rounded-lg bg-surface shadow-pop ring-1 ring-line-strong/40"
          >
            <div className="flex items-center justify-between border-b border-line px-3.5 py-2.5">
              <span className="text-sm font-semibold text-ink">任务中心</span>
              <button
                aria-label="关闭任务中心"
                onClick={() => setOpen(false)}
                className="text-ink-faint transition-colors hover:text-ink"
              >
                <X size={13} />
              </button>
            </div>

            {running.length === 0 && settled.length === 0 ? (
              <div className="px-3.5 py-6 text-center text-sm text-ink-faint">
                {desktop.isDesktop ? '当前没有进行中的任务，最近记录会保留在这里。' : '任务中心仅在桌面端有真实数据。'}
              </div>
            ) : (
              <div className="max-h-[420px] overflow-y-auto py-1">
                {running.map((task) => (
                  <TaskRow key={task.id} task={task} onNavigate={navigate} running />
                ))}
                {running.length > 0 && settled.length > 0 && (
                  <div className="mt-1 border-t border-line px-3.5 pb-1 pt-2 text-2xs font-medium uppercase tracking-wide text-ink-faint">
                    最近完成 / 失败
                  </div>
                )}
                {settled.map((task) => (
                  <TaskRow key={task.id} task={task} onNavigate={navigate} />
                ))}
              </div>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}

function TaskRow({
  task,
  running,
  onNavigate,
}: {
  task: TaskInfo
  running?: boolean
  onNavigate: ReturnType<typeof useUIStore.getState>['navigate']
}) {
  const label = kindLabel(task.kind)
  const error = task.state === 'failed' && task.error ? parseThrownError(task.error) : null
  return (
    <div className="flex items-start gap-2.5 px-3.5 py-2">
      <span className="mt-0.5 shrink-0">
        {running ? (
          <Loader2 size={13} className="animate-spin text-accent" />
        ) : task.state === 'failed' ? (
          <CircleAlert size={13} className="text-danger" />
        ) : (
          <CircleCheck size={13} className="text-ink-faint" />
        )}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline justify-between gap-2">
          <span className="truncate text-sm text-ink">{task.label || label}</span>
          <span className="num shrink-0 text-2xs text-ink-faint">{elapsed(task)}</span>
        </div>
        <div className="mt-0.5 flex items-center gap-2 text-sm text-ink-faint">
          {running ? (
            <>
              <span>{phaseText(task)}</span>
              {/* A real ratio earns a number next to the phase word;
                  indeterminate phases stay on the word alone — inventing a
                  percent for them is the lie the old rows told by omission
                  (they showed none, because nothing ever set the phase). */}
              {task.progress != null && (
                <span className="num shrink-0">{Math.round(task.progress * 100)}%</span>
              )}
              {task.cancelRequested || task.id.startsWith('task-') ? null : (
                <button className="text-accent hover:underline" onClick={() => void cancelTransfer(task.id)}>
                  取消
                </button>
              )}
            </>
          ) : task.state === 'cancelled' ? (
            <span>已取消 · {label}</span>
          ) : error ? (
            <span className="break-all text-danger/90">
              {error.message}
              {error.hint ? `（${error.hint}）` : ''}
            </span>
          ) : (
            <span>已完成 · {label}</span>
          )}
        </div>
        {running && task.progress != null && (
          <div className="mt-1">
            <ProgressBar value={task.progress} height={3} />
          </div>
        )}
        {/* 失败的下载类任务给出直接可达的页面入口（重试入口）。 */}
        {task.state === 'failed' && (task.kind === 'version-install' || task.kind === 'runtime-install') && (
          <button
            className="mt-1 text-sm text-accent hover:underline"
            onClick={() => onNavigate({ name: task.kind === 'version-install' ? 'versions' : 'runtimes' })}
          >
            前往重试
          </button>
        )}
      </div>
    </div>
  )
}
