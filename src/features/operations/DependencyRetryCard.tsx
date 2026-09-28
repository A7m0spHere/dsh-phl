import { useState } from 'react'
import { PackageCheck, PackagePlus } from 'lucide-react'
import { Button, Notice, Spinner } from '@/components/ui'
import { useInstanceStore, useUIStore } from '@/stores'
import { Cancelled, repository } from '@/services'
import { parseThrownError } from '@/lib/errorCodes'

/**
 * Community-import retry entry (CR-06/D10, R3-03, R4-01): a
 * `needsDependencies` instance shows a real, in-place retry — the same
 * instance, only the missing dependencies. A `corruptImport` instance uses the
 * same entry, but its copy says what is actually true: the record that held
 * the dependency plan is gone, so no local retry can rebuild it, and the way
 * forward is re-importing the original pack (the launch gate refuses either
 * way).
 *
 * The outcome contract is the point of this component's failure path: a
 * dependency install that FAILED is a normal `Ok(outcome)` from the backend
 * with `readiness: needsDependencies` and `dependencyFailures` filled in. Only
 * an explicitly-ready outcome with no failures may be announced as success —
 * anything else reports the real reasons and leaves the state alone (round-4
 * counterexample: 「安装完成、实例已就绪」 on a failed install).
 */
export function DependencyRetryCard({
  instanceId,
  variant = 'needsDependencies',
  failures = [],
}: {
  instanceId: string
  variant?: 'needsDependencies' | 'corruptImport'
  /** The reasons the import record already holds, from the instance record. */
  failures?: string[]
}) {
  const [running, setRunning] = useState(false)
  const reload = useInstanceStore((s) => s.reload)
  const corrupt = variant === 'corruptImport'

  const onRetry = async () => {
    setRunning(true)
    try {
      const outcome = await repository.preparePackDependencies(
        instanceId,
        () => {},
        new AbortController().signal,
      )
      // Report the outcome FIRST, refresh the record after: the message is
      // about this attempt, and it must not wait on a disk round-trip (the
      // refresh only feeds the reasons back into the card).
      if (!outcome) {
        useUIStore.getState().toast({
          kind: 'warn',
          title: '无法重试依赖安装',
          message: '桌面端不可用：浏览器模式不会真正安装依赖。',
        })
      } else if (outcome.readiness === 'readyToLaunch' && outcome.dependencyFailures.length === 0) {
        useUIStore.getState().toast({
          kind: 'success',
          title: '依赖安装完成',
          message: '实例已就绪',
        })
      } else {
        // A failed attempt is a normal `Ok(outcome)` with reasons, never an
        // exception (round-4 counterexample: 「安装完成、实例已就绪」 after a
        // failed install). Say exactly what the backend said and keep the
        // instance pending.
        const reasons = outcome.dependencyFailures.join('；')
        useUIStore.getState().toast({
          kind: 'error',
          title: '依赖安装未完成',
          message: reasons || '实例仍处于待补依赖状态，可再次重试。',
          duration: 8000,
        })
      }
      await reload()
    } catch (err) {
      if (err instanceof Cancelled) {
        useUIStore.getState().toast({ kind: 'info', title: '已取消', message: '实例状态未改变' })
      } else {
        useUIStore.getState().toast({
          kind: 'error',
          title: '依赖安装失败',
          message: parseThrownError(err).message || '实例保持待补依赖状态，可再次重试。',
          duration: 8000,
        })
      }
    } finally {
      setRunning(false)
    }
  }

  const onReimport = () => {
    useUIStore.getState().push({ name: 'installPack' })
  }

  return (
    <Notice tone="warn">
      <div className="space-y-2">
        <div className="flex items-start gap-2">
          <PackageCheck size={14} className="mt-0.5 shrink-0" />
          <span className="flex-1">
            {corrupt
              ? '整合包导入记录缺失或已损坏，实例无法确认依赖状态，启动被拒绝。该记录无法在本地重建（依赖计划就存在其中），请重新导入原整合包。'
              : '整合包依赖尚未安装完成，实例无法启动。'}
          </span>
        </div>
        {failures.length > 0 && (
          <ul className="list-disc space-y-0.5 pl-5 text-sm text-ink-muted">
            {failures.slice(0, 5).map((reason) => (
              <li key={reason} className="break-all">
                {reason}
              </li>
            ))}
          </ul>
        )}
        {/* Actions keep their intrinsic width and may wrap onto their own
            line: the card sits in a narrow detail column, and a squeezed
            button clips its label (round-5 P2 — the label, not the click, is
            what a user reads). The explanation is a separate line for the same
            reason. */}
        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant="secondary"
            disabled={running}
            className="shrink-0 whitespace-nowrap"
            onClick={() => void onRetry()}
          >
            {running ? <Spinner size={12} weight={2.6} /> : null}
            重试依赖安装
          </Button>
          {corrupt && (
            <Button
              size="sm"
              variant="secondary"
              className="shrink-0 whitespace-nowrap"
              onClick={onReimport}
            >
              <PackagePlus size={12} />
              重新导入整合包
            </Button>
          )}
        </div>
        {corrupt && (
          <p className="text-xs leading-relaxed text-ink-faint">
            「重试」对损坏记录只会确认状态并报出原因；要恢复可用实例，请重新导入原包。
          </p>
        )}
      </div>
    </Notice>
  )
}
