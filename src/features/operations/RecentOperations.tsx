import { useCallback, useEffect, useState } from 'react'
import { ClipboardList, FileDown } from 'lucide-react'
import { Badge, Button, SectionCard } from '@/components/ui'
import { useUIStore } from '@/stores'
import { formatDateTime } from '@/lib/format'
import { parseThrownError } from '@/lib/errorCodes'
import { chooseDirectory } from '@/lib/desktop'
import { exportOperationReport, listOperations, type OperationSummary } from '@/lib/desktopOperations'

const STATUS_BADGE: Record<OperationSummary['status'], { tone: 'ok' | 'warn' | 'danger' | 'neutral'; label: string }> = {
  committed: { tone: 'ok', label: '已完成' },
  failed: { tone: 'danger', label: '失败' },
  cancelled: { tone: 'neutral', label: '已取消' },
  interrupted: { tone: 'warn', label: '中断（应用退出）' },
  running: { tone: 'warn', label: '进行中' },
}

const KIND_LABEL: Record<string, string> = {
  'trial-create': '复制并试用新版',
  'community-pack-install': '安装社区整合包',
  'pack-dependencies': '安装整合包依赖',
  'launch-verify': '启动验证',
}

/**
 * 最近操作 (R1 · M5): the per-instance slice of the operation journal.
 * Exports a scrubbed JSON + Markdown report through a save dialog; the
 * local launch log itself is never shared verbatim.
 */
export function RecentOperations({ instanceId }: { instanceId: string }) {
  const [operations, setOperations] = useState<OperationSummary[] | null>(null)
  const [exporting, setExporting] = useState<string | null>(null)

  const reload = useCallback(() => {
    listOperations(instanceId, 20)
      .then((rows) => setOperations(rows ?? []))
      .catch(() => setOperations([]))
  }, [instanceId])

  useEffect(() => {
    reload()
    const timer = window.setInterval(reload, 8000)
    return () => window.clearInterval(timer)
  }, [reload])

  const onExport = async (operation: OperationSummary) => {
    setExporting(operation.operationId)
    try {
      const dest = await chooseDirectory()
      if (!dest) return
      const result = await exportOperationReport(operation.operationId, `${dest}\\phl-操作报告`)
      if (!result) return
      useUIStore.getState().toast({
        kind: 'success',
        title: '报告已导出',
        message: result.markdownPath,
        duration: 6000,
      })
    } catch (err) {
      useUIStore.getState().toast({
        kind: 'error',
        title: '导出失败',
        message: parseThrownError(err).message,
      })
    } finally {
      setExporting(null)
    }
  }

  const rows = operations ?? []
  return (
    <SectionCard
      title="最近操作"
      description="试升级、社区包导入、依赖准备与启动验证的记录；崩溃后按中断呈现，不假装成功。"
    >
      {rows.length === 0 ? (
        <p className="flex items-center gap-2 px-1.5 py-2 text-sm text-ink-faint">
          <ClipboardList size={14} />
          暂无操作记录。
        </p>
      ) : (
        <ul className="flex flex-col gap-1.5">
          {rows.map((op) => {
            const badge = STATUS_BADGE[op.status]
            return (
              <li
                key={op.operationId}
                className="flex items-center gap-2.5 rounded-md bg-surface px-2.5 py-2 ring-1 ring-inset ring-line"
              >
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-sm text-ink">{KIND_LABEL[op.kind] ?? op.kind}</span>
                    <Badge tone={badge.tone}>{badge.label}</Badge>
                  </div>
                  <p className="mt-0.5 truncate text-xs text-ink-faint">
                    {formatDateTime(op.startedAt)}
                    {op.finishedAt ? ` → ${formatDateTime(op.finishedAt)}` : ''}
                    {op.error ? ` · ${op.error}` : ''}
                  </p>
                </div>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={exporting === op.operationId}
                  onClick={() => void onExport(op)}
                >
                  <FileDown size={12} />
                  导出报告
                </Button>
              </li>
            )
          })}
        </ul>
      )}
    </SectionCard>
  )
}
