import { AnimatePresence, motion } from 'motion/react'
import { Check } from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'
import { LAUNCH_PHASE_LABEL, LAUNCH_PHASES, type InstanceRuntimeState } from '@/types'
import { ProgressBar, Spinner } from '@/components/ui'

/**
 * The launch sequence, spelled out. A single spinner tells the user to wait;
 * a checklist tells them what the launcher is actually doing to their machine,
 * which is what makes a failure legible when one happens.
 */
export function LaunchTimeline({ state }: { state: InstanceRuntimeState }) {
  const { t, scale } = useMotion()
  const currentIndex = state.phase ? LAUNCH_PHASES.indexOf(state.phase) : -1

  return (
    <div className="space-y-2.5">
      <div className="flex items-center gap-3">
        <ProgressBar value={state.progress ?? 0} active className="flex-1" height={5} />
        <span className="num w-9 text-right text-sm font-medium text-accent-ink">
          {Math.round((state.progress ?? 0) * 100)}%
        </span>
      </div>

      <ol className="grid grid-cols-1 gap-x-6 gap-y-0.5 sm:grid-cols-2">
        {LAUNCH_PHASES.map((phase, index) => {
          const done = index < currentIndex
          const active = index === currentIndex
          return (
            <li
              key={phase}
              className={cn(
                'flex items-center gap-2 py-[5px] text-sm transition-colors duration-200',
                active ? 'text-ink' : done ? 'text-ink-muted' : 'text-ink-faint',
              )}
            >
              <span className="flex h-4 w-4 shrink-0 items-center justify-center">
                <AnimatePresence mode="wait" initial={false}>
                  {done ? (
                    <motion.span
                      key="done"
                      initial={{ scale: scale === 0 ? 1 : 0.4, opacity: 0 }}
                      animate={{ scale: 1, opacity: 1 }}
                      transition={t(0.2)}
                      className="flex h-[15px] w-[15px] items-center justify-center rounded-full bg-ok/15 text-ok"
                    >
                      <Check size={10} strokeWidth={3} />
                    </motion.span>
                  ) : active ? (
                    <motion.span
                      key="active"
                      initial={{ opacity: 0 }}
                      animate={{ opacity: 1 }}
                      transition={t(0.16)}
                      className="text-accent"
                    >
                      <Spinner size={13} weight={2.4} />
                    </motion.span>
                  ) : (
                    <motion.span
                      key="idle"
                      initial={{ opacity: 0 }}
                      animate={{ opacity: 1 }}
                      className="h-[6px] w-[6px] rounded-full bg-current opacity-40"
                    />
                  )}
                </AnimatePresence>
              </span>
              <span className={cn('truncate', active && 'font-medium')}>
                {LAUNCH_PHASE_LABEL[phase]}
              </span>
            </li>
          )
        })}
      </ol>
    </div>
  )
}
