import { forwardRef, useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { AlertCircle, CheckCircle2, Info, TriangleAlert, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'
import { useUIStore, type Toast as ToastModel } from '@/stores/uiStore'
import { Spinner } from './Spinner'

const ICONS = {
  info: <Info size={15} />,
  success: <CheckCircle2 size={15} />,
  warn: <TriangleAlert size={15} />,
  error: <AlertCircle size={15} />,
  progress: <Spinner size={15} />,
}

const TONES = {
  info: 'text-info',
  success: 'text-ok',
  warn: 'text-warn',
  error: 'text-danger',
  progress: 'text-accent',
}

const ToastRow = forwardRef<HTMLDivElement, { toast: ToastModel }>(function ToastRow({ toast }, ref) {
  const dismiss = useUIStore((s) => s.dismissToast)
  const { t } = useMotion()
  const [paused, setPaused] = useState(false)
  const remaining = useRef(toast.duration)
  const startedAt = useRef(Date.now())

  useEffect(() => {
    if (!toast.duration || paused) return
    startedAt.current = Date.now()
    const id = window.setTimeout(() => dismiss(toast.id), remaining.current)
    return () => {
      window.clearTimeout(id)
      remaining.current = Math.max(600, remaining.current - (Date.now() - startedAt.current))
    }
  }, [toast.duration, toast.id, dismiss, paused])

  return (
    <motion.div
      ref={ref}
      layout
      initial={{ opacity: 0, y: -14, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: -8, scale: 0.98, transition: t(0.14) }}
      transition={t(0.26)}
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      className="pointer-events-auto relative w-[312px] overflow-hidden rounded-lg bg-surface-raised shadow-pop ring-1 ring-inset ring-line"
    >
      <div className="flex gap-2 px-2.5 py-2">
        <span className={cn('mt-[1px] shrink-0', TONES[toast.kind])}>{ICONS[toast.kind]}</span>
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium leading-[17px] text-ink">{toast.title}</div>
          {toast.message && (
            <div className="mt-0.5 text-xs leading-[15px] text-ink-muted">{toast.message}</div>
          )}
        </div>
        <div className="flex shrink-0 items-start gap-1">
          {toast.action && (
            <button
              onClick={() => {
                toast.action!.run()
                dismiss(toast.id)
              }}
              className="rounded-xs px-1.5 py-0.5 text-sm font-medium text-accent-ink transition-colors hover:bg-accent-soft"
            >
              {toast.action.label}
            </button>
          )}
          <button
            onClick={() => dismiss(toast.id)}
            aria-label="关闭"
            className="rounded-xs p-1 text-ink-faint transition-colors hover:bg-surface-hover hover:text-ink"
          >
            <X size={13} />
          </button>
        </div>
      </div>
      {toast.duration > 0 && (
        <motion.div
          className={cn('absolute inset-x-0 bottom-0 h-[2px] origin-left', {
            'bg-info': toast.kind === 'info',
            'bg-ok': toast.kind === 'success',
            'bg-warn': toast.kind === 'warn',
            'bg-danger': toast.kind === 'error',
            'bg-accent': toast.kind === 'progress',
          })}
          initial={{ scaleX: 1 }}
          animate={{ scaleX: paused ? undefined : 0 }}
          transition={{ duration: remaining.current / 1000, ease: 'linear' }}
        />
      )}
    </motion.div>
  )
})

/**
 * Toasts land under the title bar, centred — the same place the app's own
 * chrome lives, so status never competes with the content area for attention.
 */
export function Toaster() {
  const toasts = useUIStore((s) => s.toasts)
  return (
    <div className="pointer-events-none fixed left-1/2 top-[calc(var(--titlebar-h)+10px)] z-[70] flex -translate-x-1/2 flex-col items-center gap-2">
      <AnimatePresence initial={false} mode="popLayout">
        {toasts.map((toast) => (
          <ToastRow key={toast.id} toast={toast} />
        ))}
      </AnimatePresence>
    </div>
  )
}
