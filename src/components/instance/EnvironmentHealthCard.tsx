import { useCallback, useEffect, useState } from 'react'
import { AlertTriangle, CheckCircle2, RefreshCw, ShieldCheck, XCircle } from 'lucide-react'
import { verifyInstance, type RemoteVerifyCheck, type RemoteVerifyResult } from '@/lib/desktop'
import { Badge, Button, SectionCard } from '@/components/ui'
import { cn } from '@/lib/cn'

/**
 * 环境健康（T-101/T-103）：对实例执行一次只读的结构化体检，把 manifest、
 * DSH、配置、API、启动条件逐项展示出来。报告由 Rust 生成，这里只负责呈现
 * 与重新检查 —— 修复动作（repairAction）目前仅作标记，Repair 流程落地后
 * 才会在此出现按钮。
 */

const OVERALL_TONE = {
  healthy: { tone: 'ok' as const, label: '健康' },
  degraded: { tone: 'warn' as const, label: '降级' },
  broken: { tone: 'danger' as const, label: '异常' },
} as const

function StatusIcon({ check }: { check: RemoteVerifyCheck }) {
  const cls = 'h-[14px] w-[14px] shrink-0'
  if (check.status === 'pass') return <CheckCircle2 className={cn(cls, 'text-ok')} />
  if (check.status === 'warn') return <AlertTriangle className={cn(cls, 'text-warn')} />
  return <XCircle className={cn(cls, 'text-danger')} />
}

export function EnvironmentHealthCard({ instanceId }: { instanceId: string }) {
  const [report, setReport] = useState<RemoteVerifyResult | null>(null)
  const [running, setRunning] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const run = useCallback(async () => {
    setRunning(true)
    setError(null)
    try {
      const result = await verifyInstance(instanceId)
      setReport(result)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setRunning(false)
    }
  }, [instanceId])

  useEffect(() => {
    setReport(null)
    void run()
  }, [run])

  const overall = report ? OVERALL_TONE[report.overall as keyof typeof OVERALL_TONE] : null

  return (
    <SectionCard
      title="环境健康"
      icon={<ShieldCheck size={14} />}
      description={report ? `${report.checks.length} 项检查` : '检查中…'}
      collapsible
      defaultOpen={report?.overall !== 'healthy'}
      extra={
        <>
          {overall && report && <Badge tone={overall.tone}>{overall.label}</Badge>}
          <Button size="xs" variant="ghost" onClick={() => void run()} disabled={running}>
            <RefreshCw size={12} className={cn(running && 'animate-spin')} />
            重新检查
          </Button>
        </>
      }
    >
      {error && (
        <p className="px-4 py-3 text-sm text-danger">检查失败：{error}</p>
      )}
      {!error && !report && (
        <p className="px-4 py-3 text-sm text-ink-faint">正在读取实例环境…</p>
      )}
      {report && (
        <ul>
          {report.checks.map((check) => (
            <li
              key={check.id}
              className="flex items-start gap-2.5 px-4 py-[7px] transition-colors hover:bg-surface-hover/60"
            >
              <span className="mt-[3px]">
                <StatusIcon check={check} />
              </span>
              <span className="min-w-0 flex-1">
                <span
                  className={cn(
                    'block text-base leading-snug',
                    check.status === 'fail' ? 'text-danger' : check.status === 'warn' ? 'text-warn' : 'text-ink',
                  )}
                >
                  {check.message}
                </span>
                {check.repairable && (
                  <span className="mt-0.5 block text-sm text-ink-faint">
                    可修复{check.repairAction ? `（${check.repairAction}）` : ''}
                  </span>
                )}
              </span>
              <span className="shrink-0 text-sm text-ink-faint">{check.category}</span>
            </li>
          ))}
        </ul>
      )}
    </SectionCard>
  )
}
