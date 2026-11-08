import { memo, useState } from 'react'
import { FileText, Play, RefreshCw, RotateCw, Square } from 'lucide-react'
import { Badge, Button, IconButton, ProgressBar, Tooltip } from '@/components/ui'
import { StatusDot } from '@/components/instance/StatusPill'
import { useInstanceStore } from '@/stores/instanceStore'
import { useSettingsStore } from '@/stores/settingsStore'
import { useUIStore } from '@/stores/uiStore'
import { isDesktop, revealPath } from '@/lib/desktop'
import { formatBytes } from '@/lib/format'
import { parseThrownError } from '@/lib/errorCodes'
import { cn } from '@/lib/cn'
import { LAUNCH_PHASE_LABEL, type Instance, type InstanceRuntimeState, type ProcessSample } from '@/types'
import { STOPPED } from '@/stores/instanceLifecycleActions'
import { processReading } from './processStatus'
import { useProcessSampling, useProcessStore } from './processStore'

const PAGE_SIZE = 50

export function InstanceProcessPanel({ instances }: { instances: Instance[] }) {
  useProcessSampling()
  const samples = useProcessStore((state) => state.samples)
  const error = useProcessStore((state) => state.error)
  const refreshedAt = useProcessStore((state) => state.refreshedAt)
  const loading = useProcessStore((state) => state.loading)
  const states = useInstanceStore((state) => state.states)
  const restarting = useInstanceStore((state) => state.restarting)
  const stoppingAll = useInstanceStore((state) => state.stoppingAll)
  const allInstances = useInstanceStore((state) => state.instances)
  const [requestedPage, setPage] = useState(0)
  const pageCount = Math.max(1, Math.ceil(instances.length / PAGE_SIZE))
  const page = Math.min(requestedPage, pageCount - 1)
  const active = allInstances.filter((instance) => ['running', 'starting', 'stopping'].includes(states[instance.id]?.status ?? 'stopped'))

  const stopAll = async () => {
    const ui = useUIStore.getState()
    const ok = await ui.confirm({
      title: '停止全部实例',
      message: `将取消启动并停止全部 ${active.length} 个活跃实例，包含当前筛选范围之外的实例。`,
      detail: active.slice(0, 10).map((instance) => instance.name).join('\n'),
      confirmLabel: '全部停止', tone: 'danger',
    })
    if (!ok) return
    const failed = await useInstanceStore.getState().stopAll()
    ui.toast(failed.length
      ? { kind: 'error', title: `${failed.length} 个实例未能确认停止`, message: '请查看对应行的原因并重试停止。' }
      : { kind: 'success', title: '全部实例已停止' })
  }

  return (
    <section aria-label="实例进程总览" className="mt-3 overflow-hidden rounded-lg bg-surface ring-1 ring-inset ring-line">
      <div className="flex flex-wrap items-center gap-2 border-b border-line p-3">
        <div className="min-w-0 flex-1 text-sm text-ink-muted">
          <p>CPU 与内存按 DSH 主进程统计，CPU 100% 表示全部可用逻辑处理器。</p>
          <p className="mt-1 text-ink-faint">{isDesktop ? refreshedAt ? `最近采样 ${new Date(refreshedAt).toLocaleTimeString()}` : '正在读取真实进程数据' : '浏览器模拟模式，资源数据不可采样'}</p>
        </div>
        <Button size="sm" disabled={!isDesktop || loading} onClick={() => void useProcessStore.getState().refresh()}><RefreshCw size={12} />刷新</Button>
        <Button size="sm" variant="danger" disabled={!active.length || stoppingAll} loading={stoppingAll} onClick={() => void stopAll()}><Square size={12} />全部停止</Button>
      </div>
      {error && <p role="alert" className="border-b border-line bg-warn/10 p-3 text-sm text-warn">采样失败，可刷新重试。{error}</p>}
      <div className="overflow-x-auto">
        <table className="w-full min-w-[660px] text-left text-sm">
          <thead className="bg-surface-sunken text-xs text-ink-faint">
            <tr>{['实例', '状态', 'PID / 端口', 'CPU', '内存 RSS', '启动时间', '操作'].map((label) => (
              <th key={label} scope="col" className={cn('whitespace-nowrap px-3 py-2 font-medium', label === '操作' && 'sticky right-0 bg-surface-sunken')}>{label}</th>
            ))}</tr>
          </thead>
          <tbody>
            {instances.length === 0 && <tr><td colSpan={7} className="p-5 text-center text-ink-faint">没有匹配的实例，可清除左侧搜索与筛选。</td></tr>}
            {instances.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE).map((instance) => (
              <ProcessRow key={instance.id} instance={instance} state={states[instance.id] ?? STOPPED}
                sample={samples[instance.id]} error={error} restarting={!!restarting[instance.id]} stoppingAll={stoppingAll} />
            ))}
          </tbody>
        </table>
      </div>
      <footer className="flex items-center gap-2 border-t border-line px-3 py-2 text-sm text-ink-faint">
        <span>共 {instances.length} 个实例 · 每页 {PAGE_SIZE} 个</span>
        <span className="ml-auto">{page + 1} / {pageCount}</span>
        <Button size="sm" disabled={page === 0} onClick={() => setPage(page - 1)}>上一页</Button>
        <Button size="sm" disabled={page + 1 === pageCount} onClick={() => setPage(page + 1)}>下一页</Button>
      </footer>
    </section>
  )
}

const ProcessRow = memo(function ProcessRow({ instance, state, sample, error, restarting, stoppingAll }: {
  instance: Instance; state: InstanceRuntimeState; sample?: ProcessSample
  error: string | null; restarting: boolean; stoppingAll: boolean
}) {
  const reading = processReading(state, sample, error, Date.now())
  const busy = restarting || stoppingAll || state.status === 'stopping'
  const startedAt = reading.sample?.startedAtMs
  const hasLogs = !!instance.lastRunAt || !!state.pid || !!state.lastExit || !!state.error
  const running = state.status === 'running'
  const starting = state.status === 'starting'
  const primaryLabel = running ? '停止' : starting ? '取消启动' : '启动'
  let cpu = '—'
  if (reading.sample) cpu = reading.sample.cpuPercent === null ? '采样中' : `${reading.sample.cpuPercent.toFixed(1)}%`
  const primaryAction = () => {
    const store = useInstanceStore.getState()
    if (running) void store.stop(instance.id)
    else if (starting) store.cancelLaunch(instance.id)
    else void store.launch(instance.id)
  }
  const openLogs = async () => {
    try {
      await revealPath([useSettingsStore.getState().root, 'instances', instance.id, 'logs'].join('/'))
    } catch (failure) {
      useUIStore.getState().toast({ kind: 'error', title: '无法打开日志目录', message: parseThrownError(failure).message })
    }
  }
  return (
    <tr className="border-t border-line align-top" data-instance-id={instance.id}>
      <td className="max-w-[180px] px-3 py-3">
        <button className="max-w-full truncate font-medium text-ink hover:text-accent-ink" title={instance.name}
          onClick={() => useUIStore.getState().push({ name: 'instance', id: instance.id })}>{instance.name}</button>
        {reading.detail && <p className="mt-1 line-clamp-2 whitespace-pre-wrap text-xs text-ink-faint" title={reading.detail}>{reading.detail}</p>}
      </td>
      <td className="px-3 py-3">
        <Badge tone={reading.tone}>
          {reading.tone === 'ok' && <StatusDot status="running" size={5} />}
          {restarting ? '重启中' : reading.label}
        </Badge>
        {state.status === 'starting' && <div className="mt-1.5 min-w-[90px]">
          <p className="mb-1 text-xs text-ink-faint">{LAUNCH_PHASE_LABEL[state.phase ?? 'resolve-version']}</p>
          <ProgressBar value={state.progress ?? 0} />
        </div>}
      </td>
      <td className="whitespace-nowrap px-3 py-3 font-mono text-xs text-ink-muted">
        <div>{state.pid ?? '—'}</div><div className="mt-1">:{reading.sample?.port ?? instance.port}</div>
      </td>
      <td className="whitespace-nowrap px-3 py-3 tabular-nums text-ink-muted">{cpu}</td>
      <td className="whitespace-nowrap px-3 py-3 tabular-nums text-ink-muted">{reading.sample?.memoryBytes != null ? formatBytes(reading.sample.memoryBytes) : '—'}</td>
      <td className="whitespace-nowrap px-3 py-3 text-xs text-ink-muted">{startedAt ? <><div>{new Date(startedAt).toLocaleDateString()}</div><div className="mt-1">{new Date(startedAt).toLocaleTimeString()}</div></> : '—'}</td>
      <td className="sticky right-0 border-l border-line bg-surface px-3 py-2">
        <div className="flex gap-1">
          <Tooltip content={primaryLabel}>
            <IconButton label={`${primaryLabel} ${instance.name}`} size="sm" disabled={busy} onClick={primaryAction}>
              {running || starting ? <Square size={13} /> : <Play size={13} />}
            </IconButton>
          </Tooltip>
          {running && (
            <Tooltip content="重启实例">
              <IconButton label={`重启 ${instance.name}`} size="sm" disabled={busy}
                onClick={() => void useInstanceStore.getState().restart(instance.id)}>
                <RotateCw size={13} />
              </IconButton>
            </Tooltip>
          )}
          <Tooltip content={hasLogs ? '打开日志目录' : '启动后生成日志'}>
            <IconButton label={`日志 ${instance.name}`} size="sm" disabled={!hasLogs} onClick={() => void openLogs()}>
              <FileText size={13} />
            </IconButton>
          </Tooltip>
        </div>
      </td>
    </tr>
  )
})
