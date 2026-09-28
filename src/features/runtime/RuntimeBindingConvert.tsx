import { useEffect, useState } from 'react'
import { ArrowRightLeft } from 'lucide-react'
import { Badge, Button, Notice, ProgressBar, Spinner } from '@/components/ui'
import { useInstanceStore, useUIStore } from '@/stores'
import { Cancelled, repository } from '@/services'
import { parseThrownError } from '@/lib/errorCodes'
import type { RuntimeConversionPreview } from '@/lib/desktop'

/**
 * Legacy → precise runtime binding conversion (R1 · M1).
 *
 * An instance bound to a legacy `node-<major>` directory keeps launching, but
 * precise bindings are what make same-major patch versions coexist and what
 * environment comparisons can honestly verify. This card offers the explicit
 * per-instance conversion: prepare and verify the precise object first, flip
 * the manifest atomically, never guess the version (the target is the legacy
 * directory's recorded marker version, and a marker/binary mismatch blocks).
 */
export function RuntimeBindingConvert({ instanceId }: { instanceId: string }) {
  const [preview, setPreview] = useState<RuntimeConversionPreview | null>(null)
  const [loadFailed, setLoadFailed] = useState(false)
  const [running, setRunning] = useState(false)
  const [progress, setProgress] = useState<{ stage: string; progress?: number } | null>(null)
  const reloadInstances = useInstanceStore((s) => s.reload)

  useEffect(() => {
    let alive = true
    repository
      .previewRuntimeConversion(instanceId)
      .then((p) => {
        if (alive) setPreview(p)
      })
      .catch(() => {
        if (alive) setLoadFailed(true)
      })
    return () => {
      alive = false
    }
  }, [instanceId])

  if (loadFailed || !preview) return null

  const {
    legacyId,
    recordedVersion,
    binaryVersion,
    targetId,
    targetInstalled,
    blockedReason,
  } = preview

  const onConvert = async () => {
    setRunning(true)
    setProgress({ stage: 'preparing' })
    try {
      await repository.convertRuntimeBinding(instanceId, (p) => setProgress(p), new AbortController().signal)
      useUIStore.getState().toast({
        kind: 'success',
        title: '已转为精确 Runtime 绑定',
        message: `实例现在绑定 ${targetId}`,
      })
      await reloadInstances()
    } catch (err) {
      if (!(err instanceof Cancelled)) {
        useUIStore.getState().toast({
          kind: 'error',
          title: '转换失败',
          message: parseThrownError(err).message || '旧绑定保持不变。',
          duration: 6000,
        })
      }
    } finally {
      setRunning(false)
      setProgress(null)
    }
  }

  return (
    <div className="rounded-lg bg-surface-sunken/60 p-3 ring-1 ring-inset ring-line">
      <div className="flex items-center gap-2">
        <ArrowRightLeft size={14} className="text-ink-faint" />
        <span className="text-sm font-medium text-ink">旧式 Runtime 绑定</span>
        <Badge tone="warn">node-{preview.legacyMajor}</Badge>
        <div className="ml-auto">
          <Button size="sm" variant="secondary" disabled={running || !!blockedReason} onClick={() => void onConvert()}>
            {running ? <Spinner size={12} weight={2.6} /> : null}
            转为精确绑定
          </Button>
        </div>
      </div>
      <p className="mt-1.5 text-sm leading-relaxed text-ink-faint">
        当前绑定 {legacyId}
        {recordedVersion ? `（标记 v${recordedVersion}` : ''}
        {binaryVersion ? `，实际 v${binaryVersion}）` : recordedVersion ? '）' : ''}
        。转换会安装并验证精确对象 <span className="font-mono">{targetId || '—'}</span>
        {targetInstalled ? '（已安装，无需下载）' : ''}，成功后原子更新本实例绑定；旧目录保留，可稍后清理。
      </p>
      {blockedReason && !running && (
        <Notice tone="warn" className="mt-2">
          {blockedReason}
        </Notice>
      )}
      {running && progress && (
        <div className="mt-2">
          <ProgressBar
            value={progress.progress ?? 0}
            indeterminate={progress.stage === 'verifying' || progress.stage === 'preparing' || progress.stage === 'committing'}
            active={progress.stage === 'downloading'}
            height={4}
          />
          <p className="mt-1 text-sm text-ink-faint">
            {progress.stage === 'downloading'
              ? '正在下载精确版本…'
              : progress.stage === 'verifying'
                ? '正在校验二进制与 SHASUMS…'
                : progress.stage === 'committing'
                  ? '正在更新实例绑定…'
                  : '准备中…'}
          </p>
        </div>
      )}
    </div>
  )
}
