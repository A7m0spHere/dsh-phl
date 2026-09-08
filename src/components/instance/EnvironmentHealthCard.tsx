import { parseThrownError } from '@/lib/errorCodes'
import { useCallback, useEffect, useState } from 'react'
import { AlertTriangle, CheckCircle2, RefreshCw, ShieldCheck, Wrench, XCircle } from 'lucide-react'
import {
  repairInstance,
  verifyInstance,
  type RemoteVerifyCheck,
  type RemoteVerifyResult,
} from '@/lib/desktop'
import { Badge, Button, SectionCard } from '@/components/ui'
import { cn } from '@/lib/cn'
import { useUIStore } from '@/stores'

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

/** 缺失的版本 / Runtime 要通过各自页面的安装流程补齐（T-204 的下载一半）。 */
const NAV_ROUTES: Record<string, { label: string; route: 'versions' | 'runtimes' }> = {
  'install-version': { label: '去安装版本', route: 'versions' },
  'install-runtime': { label: '去安装 Runtime', route: 'runtimes' },
}

export function EnvironmentHealthCard({ instanceId }: { instanceId: string }) {
  const navigate = useUIStore((s) => s.navigate)
  const [report, setReport] = useState<RemoteVerifyResult | null>(null)
  const [running, setRunning] = useState(false)
  const [repairing, setRepairing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [repairNote, setRepairNote] = useState<string | null>(null)

  const run = useCallback(async () => {
    setRunning(true)
    setError(null)
    try {
      const result = await verifyInstance(instanceId)
      setReport(result)
    } catch (err) {
      setError(parseThrownError(err).message)
    } finally {
      setRunning(false)
    }
  }, [instanceId])

  useEffect(() => {
    setReport(null)
    setRepairNote(null)
    void run()
  }, [run])

  /**
   * 修复可修复问题：本地可推导的修复直接执行（重建 workspace、清理残留），
   * 需要下载的动作提示用户走安装流程；结束后立刻重新体检，让徽章说真话。
   */
  const repair = useCallback(async () => {
    if (!report) return
    const actions = [...new Set(report.checks.filter((c) => c.repairable).map((c) => c.repairAction ?? ''))]
      .filter((a) => a !== '')
      .concat(report.checks.some((c) => c.repairable && !c.repairAction) ? ['cleanup-txn'] : [])
    if (actions.length === 0) return
    setRepairing(true)
    setRepairNote(null)
    try {
      const outcome = await repairInstance(instanceId, actions)
      const parts: string[] = []
      if (outcome.applied.length > 0) parts.push(`已修复：${outcome.applied.join('、')}`)
      if (outcome.requiresUser.length > 0)
        parts.push(`需要重新安装（请在版本/Runtime 页操作）：${outcome.requiresUser.join('、')}`)
      setRepairNote(parts.join('；') || '没有可执行的修复动作')
      await run()
    } catch (err) {
      setRepairNote(parseThrownError(err).message)
    } finally {
      setRepairing(false)
    }
  }, [report, instanceId, run])

  const repairable = report?.checks.some((c) => c.repairable) ?? false
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
          {repairable && (
            <Button size="xs" variant="secondary" onClick={() => void repair()} disabled={repairing || running}>
              <Wrench size={12} />
              修复可修复问题
            </Button>
          )}
          <Button size="xs" variant="ghost" onClick={() => void run()} disabled={running || repairing}>
            <RefreshCw size={12} className={cn(running && 'animate-spin')} />
            重新检查
          </Button>
        </>
      }
    >
      {error && (
        <p className="px-4 py-3 text-sm text-danger">检查失败：{error}</p>
      )}
      {repairNote && (
        <p className="border-b border-line/60 px-4 py-2 text-sm text-ink-muted">{repairNote}</p>
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
                  <span className="mt-0.5 flex items-center gap-2 text-sm text-ink-faint">
                    <span>
                      可修复
                      {check.repairAction && NAV_ROUTES[check.repairAction] ? '：需要重新安装' : ''}
                    </span>
                    {check.repairAction && NAV_ROUTES[check.repairAction] && (
                      <Button
                        size="xs"
                        variant="secondary"
                        onClick={() => navigate({ name: NAV_ROUTES[check.repairAction!].route })}
                      >
                        {NAV_ROUTES[check.repairAction!].label}
                      </Button>
                    )}
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
