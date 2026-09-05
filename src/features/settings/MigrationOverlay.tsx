/** Full-screen progress for a data-root migration. */
import { AnimatePresence, motion } from 'motion/react'
import { HardDrive } from 'lucide-react'
import { type MoveProgress } from '@/lib/desktop'
import { formatBytes } from '@/lib/format'
import { useMotion } from '@/lib/motion'
import { MOVE_KIND_LABELS } from './consts'
import { Button, ProgressBar, Spinner } from '@/components/ui'

export function MigrationOverlay({
  info,
  progress,
  onCancel,
}: {
  info: { from: string; to: string } | null
  progress: MoveProgress | null
  onCancel: () => void
}) {
  const { overlay, pop } = useMotion()
  const kindLabel = progress ? (MOVE_KIND_LABELS[progress.kind] ?? progress.kind) : '准备中…'

  return (
    <AnimatePresence>
      {info && (
        <motion.div key="migration" className="absolute inset-0 z-40 flex items-center justify-center">
          <motion.div
            variants={overlay}
            initial="hidden"
            animate="show"
            exit="out"
            className="absolute inset-0 bg-canvas/70 backdrop-blur-sm"
          />
          <motion.div
            variants={pop}
            initial="hidden"
            animate="show"
            exit="out"
            className="relative w-[420px] rounded-xl bg-surface-raised p-5 shadow-pop ring-1 ring-inset ring-line"
          >
            <div className="flex items-center gap-2.5">
              <HardDrive size={15} className="text-accent" />
              <span className="text-md font-medium text-ink">正在迁移数据</span>
              <span className="num ml-auto text-sm text-ink-faint">
                {progress ? Math.round(progress.progress * 100) : 0}%
              </span>
            </div>

            <ProgressBar value={progress?.progress ?? 0} active className="mt-3.5" height={5} />

            <div className="mt-3.5 space-y-1.5 text-sm">
              <div className="flex items-center gap-2 text-ink">
                <Spinner size={12} />
                正在移动：{kindLabel}
                {progress && (
                  <span className="num ml-auto text-ink-faint">
                    {formatBytes(progress.bytesDone)} / {formatBytes(progress.bytesTotal)}
                  </span>
                )}
              </div>
              <div className="break-all font-mono text-2xs leading-relaxed text-ink-faint">
                {info.from}
                <br />→ {info.to}
              </div>
            </div>

            <div className="mt-4 flex justify-end">
              <Button size="sm" variant="ghost" onClick={onCancel}>
                取消
              </Button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
