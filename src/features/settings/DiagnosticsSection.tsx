import { useEffect, useState } from 'react'
import { cn } from '@/lib/cn'
import {
  clearDownloadCache,
  desktop,
  runDiagnostics,
  type DiagnosticReport,
} from '@/lib/desktop'
import { formatBytes, formatDateTime } from '@/lib/format'
import { PageSection } from '@/components/layout/Page'
import { Button, Notice } from '@/components/ui'
import { useSettingsStore, useUIStore } from '@/stores'

/**
 * The diagnostics interaction as one coherent unit, split out of the 1000-line
 * settings page (roadmap O-13 — migrate one complete interaction at a time,
 * behaviour preserved). The report is generated on mount (the section only
 * mounts when open) and re-generated when the data root changes underneath it;
 * every check is a cheap existence probe, so nothing here belongs at app start.
 */
export function DiagnosticsSection() {
  const root = useSettingsStore((s) => s.root)
  const ui = useUIStore()
  const [report, setReport] = useState<DiagnosticReport | null>(null)
  const [diagLoading, setDiagLoading] = useState(false)
  const [cacheBusy, setCacheBusy] = useState(false)

  useEffect(() => {
    if (!desktop.isDesktop) return
    setDiagLoading(true)
    void runDiagnostics()
      .then(setReport, () => setReport(null))
      .finally(() => setDiagLoading(false))
  }, [root])

  const rerunDiagnostics = () => {
    if (!desktop.isDesktop || diagLoading) return
    setDiagLoading(true)
    void runDiagnostics()
      .then(setReport, (err) => {
        setReport(null)
        ui.toast({
          kind: 'error',
          title: '诊断失败',
          message: err instanceof Error ? err.message : String(err),
        })
      })
      .finally(() => setDiagLoading(false))
  }

  /**
   * Copy the diagnostic report as plain text (roadmap O-09: an export that
   * can be pasted into a bug report). The report is built entirely from
   * existence/size checks — it carries no credential values and no instance
   * env — which is the desensitisation boundary SECURITY.md states.
   */
  const copyDiagnostics = async () => {
    if (!report) return
    const lines = [
      `PHL 诊断报告 · ${formatDateTime(report.generatedAt)}`,
      `数据目录: ${report.root}`,
      ...report.items.map(
        (item) => `[${item.level.toUpperCase()}] ${item.label}${item.detail ? ` — ${item.detail}` : ''}`,
      ),
    ]
    try {
      await navigator.clipboard.writeText(lines.join('\n'))
      ui.toast({ kind: 'success', title: '诊断报告已复制', message: '内容不含任何密钥或环境变量值。' })
    } catch {
      ui.toast({
        kind: 'error',
        title: '复制失败',
        message: '系统剪贴板不可用；请截图或逐项转述检查结果。',
      })
    }
  }

  const clearCache = async () => {
    if (!report) return
    const ok = await ui.confirm({
      title: '清理下载缓存',
      message: '删除下载残留（.part）与保留的压缩包。它们只用于加速重装，不影响任何已安装的版本、Runtime 与实例。',
      detail: `将释放 ${formatBytes(report.cacheBytes)}`,
      tone: 'danger',
      confirmLabel: '清理',
    })
    if (!ok) return
    setCacheBusy(true)
    try {
      const freed = await clearDownloadCache()
      ui.toast({ kind: 'success', title: '缓存已清理', message: `释放 ${formatBytes(freed)}` })
      rerunDiagnostics()
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '清理缓存失败',
        message: err instanceof Error ? err.message : String(err),
      })
    } finally {
      setCacheBusy(false)
    }
  }

  return (
    <>
      <PageSection>
        <div className="flex items-center justify-between gap-3 rounded-lg bg-surface p-4 ring-1 ring-inset ring-line">
          <div className="min-w-0">
            <div className="text-md font-medium text-ink">环境检查</div>
            <div className="mt-0.5 truncate text-sm text-ink-faint">
              {report
                ? `生成于 ${formatDateTime(report.generatedAt)} · ${report.root}`
                : '检查数据目录、版本与 Runtime 完整性、实例引用与下载缓存。'}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {report && (
              <Button variant="ghost" size="sm" onClick={() => void copyDiagnostics()}>
                复制报告
              </Button>
            )}
            <Button variant="secondary" size="sm" disabled={diagLoading} onClick={rerunDiagnostics}>
              {diagLoading ? '检查中…' : '重新检查'}
            </Button>
          </div>
        </div>
      </PageSection>

      <PageSection>
        {!desktop.isDesktop ? (
          <Notice tone="info">诊断仅在桌面端可用；浏览器预览没有真实的文件系统可检查。</Notice>
        ) : !report ? (
          <div className="rounded-lg bg-surface p-4 text-sm text-ink-faint ring-1 ring-inset ring-line">
            {diagLoading ? '正在检查…' : '尚无检查结果，点上方「重新检查」生成。'}
          </div>
        ) : (
          <div className="overflow-hidden rounded-lg bg-surface ring-1 ring-inset ring-line">
            {report.items.map((item, idx) => (
              <div
                key={item.id}
                className={cn(
                  'flex items-start gap-3 px-4 py-3',
                  idx > 0 && 'border-t border-line',
                )}
              >
                <span
                  className={cn(
                    'mt-1.5 h-2 w-2 shrink-0 rounded-full',
                    item.level === 'ok' && 'bg-ok',
                    item.level === 'warn' && 'bg-warn',
                    item.level === 'fail' && 'bg-danger',
                  )}
                />
                <div className="min-w-0 flex-1">
                  <div className="text-base text-ink">{item.label}</div>
                  {item.detail && (
                    <div className="mt-0.5 break-all text-sm text-ink-faint">
                      {item.detail}
                    </div>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </PageSection>

      {report && report.cacheBytes > 0 && (
        <PageSection title="清理">
          <Notice tone="info" className="mb-3">
            下载缓存只用于加速重装；删除它不影响任何已安装的版本、Runtime 与实例。
          </Notice>
          <Button variant="secondary" disabled={cacheBusy} onClick={() => void clearCache()}>
            {cacheBusy ? '清理中…' : `清理下载缓存（${formatBytes(report.cacheBytes)}）`}
          </Button>
        </PageSection>
      )}
    </>
  )
}
