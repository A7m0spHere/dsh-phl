import { useEffect, useRef, type KeyboardEvent } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { ArrowRight, Check, Download, RefreshCw, X } from 'lucide-react'
import { Badge, Button, Field, Input, ProgressBar, Select, Spinner } from '@/components/ui'
import { Logo } from '@/components/layout/Logo'
import { cn } from '@/lib/cn'
import { isDesktop } from '@/lib/desktop'
import { formatBytes } from '@/lib/format'
import { MODAL_SCRIM, MODAL_Z, useMotion } from '@/lib/motion'
import { isRuntimeCompatible, pickDefaultRuntime, pickSetupVersion } from '@/lib/wizardDefaults'
import { useCatalogStore } from '@/stores/catalogStore'
import { useInstanceStore } from '@/stores/instanceStore'
import { useUIStore } from '@/stores/uiStore'
import { LAUNCH_PHASE_LABEL } from '@/types'
import { setupIsBusy, useSetupStore, type SetupStep } from './setupStore'

const STEPS: { id: SetupStep; label: string }[] = [
  { id: 'runtime', label: '准备 Node' },
  { id: 'version', label: '准备 DSH' },
  { id: 'create', label: '创建实例' },
  { id: 'launch', label: '启动 DSH' },
  { id: 'open', label: '打开 WebUI' },
]

const TRANSFER_LABEL: Record<string, string> = {
  queued: '等待下载', downloading: '正在下载', verifying: '校验完整性',
  extracting: '正在解压', 'installing-deps': '安装 DSH 依赖',
}

export function SetupDialog() {
  const setup = useSetupStore()
  const versions = useCatalogStore((state) => state.versions)
  const runtimes = useCatalogStore((state) => state.runtimes)
  const loading = useCatalogStore((state) => state.loading)
  const instanceStates = useInstanceStore((state) => state.states)
  const panelRef = useRef<HTMLDivElement>(null)
  const { pop, overlay } = useMotion()
  const busy = setupIsBusy(setup.status)
  const locked = busy || !!setup.instanceId
  const version = versions.find((candidate) => candidate.id === setup.versionId)
  const runtime = runtimes.find((candidate) => candidate.id === setup.runtimeId)
  const runtimeOptions = runtimes.filter((item) => !item.legacy && item.state.kind !== 'removing' && !!version && isRuntimeCompatible(item, version))

  useEffect(() => {
    if (!setup.open) return
    const previous = document.activeElement as HTMLElement | null
    useUIStore.getState().pushOverlay()
    const timer = window.setTimeout(() => panelRef.current?.focus(), 0)
    return () => {
      window.clearTimeout(timer)
      useUIStore.getState().popOverlay()
      if (previous?.isConnected) previous.focus()
    }
  }, [setup.open])

  useEffect(() => {
    if (!setup.open || busy || setup.instanceId || (setup.versionId && setup.runtimeId)) return
    const nextVersion = version ?? pickSetupVersion(versions, runtimes)
    const nextRuntime = nextVersion && pickDefaultRuntime(runtimes, nextVersion)
    if (nextVersion && nextRuntime) {
      setup.configure({ versionId: nextVersion.id, runtimeId: nextRuntime.id })
    }
  }, [setup.open, busy, setup.instanceId, setup.versionId, setup.runtimeId, setup.configure, version, versions, runtimes])

  const handleKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.stopPropagation()
      if (busy) setup.cancel()
      else setup.close()
    }
    if (event.key !== 'Tab') return
    const controls = Array.from(panelRef.current?.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex="0"]',
    ) ?? []).filter((element) => element.offsetParent !== null)
    const first = controls[0]
    const last = controls[controls.length - 1]
    if (!first) {
      event.preventDefault()
      return
    }
    if (event.shiftKey && (document.activeElement === first || document.activeElement === panelRef.current)) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && (document.activeElement === last || document.activeElement === panelRef.current)) {
      event.preventDefault()
      first.focus()
    }
  }

  const goToInstance = () => {
    if (!setup.instanceId) return
    setup.close()
    useUIStore.getState().navigate({ name: 'instance', id: setup.instanceId })
  }

  const transfer = setup.step === 'runtime' ? runtime?.state : version?.state
  const launch = setup.instanceId ? instanceStates[setup.instanceId] : undefined
  let progress = setup.progress
  let detail = setup.detail
  if (setup.step === 'runtime' || setup.step === 'version') {
    detail = transfer ? TRANSFER_LABEL[transfer.kind] ?? '' : ''
    progress = transfer && 'progress' in transfer ? transfer.progress : null
    if (transfer?.kind === 'downloading') {
      detail += ` · ${formatBytes(transfer.bytesDone)} · ${formatBytes(transfer.bytesPerSec)}/s`
    }
  }
  if (setup.step === 'launch') {
    detail = launch?.phase ? LAUNCH_PHASE_LABEL[launch.phase] : '等待服务就绪'
    progress = launch?.progress ?? null
  }
  const stepIndex = STEPS.findIndex((step) => step.id === setup.step)
  const canRun = !!version && !!runtime && !!setup.name.trim() && isRuntimeCompatible(runtime, version)

  return (
    <AnimatePresence>
      {setup.open && (
        <motion.div className={`fixed inset-0 ${MODAL_Z} flex items-center justify-center p-4 sm:p-6`}>
          <motion.div variants={overlay} initial="hidden" animate="show" exit="out" className={MODAL_SCRIM} />
          <motion.div
            ref={panelRef} variants={pop} initial="hidden" animate="show" exit="out"
            role="dialog" aria-modal="true" aria-labelledby="setup-title" aria-describedby="setup-description"
            tabIndex={-1} onKeyDown={handleKey}
            className="relative flex max-h-full w-full max-w-[620px] flex-col overflow-hidden rounded-lg bg-surface-raised shadow-pop ring-1 ring-inset ring-line outline-none"
          >
            <header className="flex items-center gap-3 border-b border-line px-5 py-4">
              <Logo size={28} />
              <div className="min-w-0 flex-1">
                <h2 id="setup-title" className="text-lg font-semibold text-ink">一键装好，开始使用 DSH</h2>
                <p id="setup-description" className="mt-1 text-sm text-ink-muted">确认版本和名称，PHL 会准备环境并打开首个实例。</p>
              </div>
              <Button variant="ghost" aria-label={busy ? '取消一键装好' : '关闭一键装好'} disabled={setup.status === 'cancelling'} onClick={busy ? setup.cancel : setup.close}>
                <X size={16} />
              </Button>
            </header>

            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-5">
              {!isDesktop && <Badge tone="warn">浏览器预览 · 下载与启动使用模拟数据</Badge>}
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="DSH 版本" hint={version?.requiresNode.length === 0 ? '此版本未声明 Node 要求，请在启动后确认兼容性。' : '优先复用已安装版本，支持改选。'}>
                  <Select value={setup.versionId} disabled={locked} onChange={(event) => setup.configure({ versionId: event.target.value })}>
                    <option value="">请选择 DSH 版本</option>
                    {versions.filter((item) => !item.pendingPublish && !item.legacy && item.state.kind !== 'removing').map((item) => (
                      <option key={item.id} value={item.id}>{item.name} · {item.state.kind === 'installed' ? '已安装' : '需准备'}</option>
                    ))}
                  </Select>
                </Field>
                <Field label="Node 运行时" hint={version && !runtimeOptions.length ? '此版本没有可用的匹配 Node，请改选 DSH 版本或手动配置。' : '自动匹配 DSH 要求，已安装的 Node 可直接复用。'}>
                  <Select value={setup.runtimeId} disabled={locked} onChange={(event) => setup.configure({ runtimeId: event.target.value })}>
                    <option value="">请选择 Node</option>
                    {runtimeOptions.map((item) => (
                      <option key={item.id} value={item.id}>{item.system ? '系统 ' : ''}Node {item.version}{item.lts ? ' LTS' : ''} · {item.state.kind === 'installed' ? '已安装' : '需准备'}</option>
                    ))}
                  </Select>
                </Field>
                <Field label="实例名称" className="sm:col-span-2">
                  <Input value={setup.name} disabled={locked} onChange={(event) => setup.configure({ name: event.target.value })} />
                </Field>
              </div>

              <div className="rounded-lg bg-surface-sunken p-3 text-sm leading-relaxed text-ink-muted ring-1 ring-inset ring-line">
                <p className="break-all">数据目录 {setup.root}</p>
                <p className="mt-1">实例使用独立目录和自动端口，模型配置继承全局 API 设置。首次打开 WebUI 后可继续配置模型。</p>
              </div>

              {!locked && (
                <div className="flex items-center justify-between gap-3 text-sm text-ink-faint">
                  <span>{loading ? '正在加载版本与运行时目录' : !canRun ? '选择可用组件后即可开始' : '下载完成后创建实例，已装好的组件会保留供重试使用'}</span>
                  <Button size="sm" disabled={loading} onClick={() => void useCatalogStore.getState().load()}>
                    <RefreshCw size={12} />重新加载
                  </Button>
                </div>
              )}

              <ol aria-label="准备步骤" className="space-y-2">
                {STEPS.map((step, index) => {
                  const done = setup.status === 'complete' || (setup.status !== 'idle' && index < stepIndex)
                  const active = busy && index === stepIndex
                  return (
                    <li key={step.id} className={cn('flex items-center gap-3 rounded px-3 py-2 text-sm', active ? 'bg-accent-soft text-accent-ink' : 'text-ink-muted')}>
                      <span className={cn('flex h-5 w-5 shrink-0 items-center justify-center rounded-full', done ? 'bg-ok/10 text-ok' : 'bg-surface-sunken text-ink-faint')}>
                        {done ? <Check size={12} /> : active ? <Spinner size={12} /> : index + 1}
                      </span>
                      <span className="font-medium">{step.label}</span>
                      {active && <span className="ml-auto min-w-0 truncate text-xs">{detail}</span>}
                    </li>
                  )
                })}
              </ol>
              {busy && <ProgressBar value={progress ?? 0} indeterminate={progress === null} />}

              <div aria-live="polite" aria-atomic="true">
                {setup.error && <p role="alert" className="whitespace-pre-wrap rounded bg-danger/10 p-3 text-sm leading-relaxed text-danger">{setup.error}</p>}
                {setup.status === 'cancelled' && <p className="text-sm text-ink-muted">已取消。{setup.instanceId ? '已创建的实例已停止并保留，可继续启动同一个实例。' : '已安装的共享组件已保留，可继续准备。'}</p>}
                {setup.status === 'cancelling' && <p className="text-sm text-ink-muted">正在取消当前操作并确认进程退出…</p>}
                {setup.status === 'complete' && <p className="rounded bg-ok/10 p-3 text-sm text-ok">{isDesktop ? '实例已就绪，WebUI 已打开。' : '模拟流程已完成，可在桌面版中准备真实环境。'}</p>}
              </div>
            </div>

            <footer className="flex flex-wrap items-center justify-end gap-2 border-t border-line bg-surface-sunken/60 px-5 py-3">
              {!busy && !setup.instanceId && (
                <Button variant="ghost" onClick={() => { setup.close(); useUIStore.getState().push({ name: 'create' }) }}>跳过，手动配置</Button>
              )}
              {!busy && setup.instanceId && <Button onClick={goToInstance}>查看实例<ArrowRight size={12} /></Button>}
              {busy ? (
                <Button disabled={setup.status === 'cancelling'} onClick={setup.cancel}>取消准备</Button>
              ) : setup.status !== 'complete' ? (
                <Button variant="primary" disabled={!canRun || loading} onClick={() => void setup.run()}>
                  <Download size={13} />{setup.status === 'idle' ? '一键装好并打开' : '继续 / 重试'}
                </Button>
              ) : <Button variant="primary" onClick={setup.close}>完成</Button>}
            </footer>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
