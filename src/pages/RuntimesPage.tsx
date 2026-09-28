import { useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { ChevronDown, Cpu, Download, Server, Trash2, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { formatBytes, formatSpeed } from '@/lib/format'
import { useMotion } from '@/lib/motion'
import { useCatalogStore, useInstanceStore, useUIStore } from '@/stores'
import { Badge, Button, Notice, ProgressBar, SectionCard, Skeleton, Spinner, Tooltip } from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelDivider, PanelGroup, PanelShell, PanelStat } from '@/components/layout/Panel'
import { isRuntimeBusy, isRuntimeBusyKind, type Runtime } from '@/types/runtime'
import { Cancelled, repository } from '@/services'
import { parseThrownError } from '@/lib/errorCodes'

export function RuntimesPanel() {
  const runtimes = useCatalogStore((s) => s.runtimes)
  const instances = useInstanceStore((s) => s.instances)
  const installed = runtimes.filter((r) => r.state.kind === 'installed')

  return (
    <PanelShell>
      <PanelGroup title="概览">
        <PanelStat label="已安装" value={`${installed.length} 个`} />
        <PanelStat
          label="占用"
          value={formatBytes(installed.reduce((sum, r) => sum + r.size, 0))}
        />
        <PanelStat label="使用中" value={new Set(instances.map((i) => i.runtimeId)).size} />
      </PanelGroup>

      <PanelDivider />

      <PanelGroup title="说明">
        <p className="px-1.5 py-1 text-sm leading-relaxed text-ink-faint">
          Runtime 与 DSH 版本相互独立。同一个 DSH 版本可以跑在不同 Node 上，实例分别固定自己的组合。
        </p>
      </PanelGroup>

      <div className="mt-auto p-3">
        <div className="rounded-lg bg-surface-sunken p-3 text-sm leading-relaxed text-ink-faint ring-1 ring-inset ring-line">
          「系统 Node」直接使用 PATH 上的可执行文件，不由 PHL 管理，升级系统 Node 会影响引用它的实例。
        </div>
      </div>
    </PanelShell>
  )
}

export function RuntimesPage() {
  const runtimes = useCatalogStore((s) => s.runtimes)
  const loaded = useCatalogStore((s) => s.loaded)
  const install = useCatalogStore((s) => s.installRuntime)
  const cancel = useCatalogStore((s) => s.cancelRuntime)
  const remove = useCatalogStore((s) => s.removeRuntime)
  const instances = useInstanceStore((s) => s.instances)
  const confirm = useUIStore((s) => s.confirm)
  const { t, stagger, riseItem } = useMotion()

  const usedBy = useMemo(() => {
    const map = new Map<string, string[]>()
    for (const i of instances) map.set(i.runtimeId, [...(map.get(i.runtimeId) ?? []), i.name])
    return map
  }, [instances])

  /* ---- patch expander: install an older build of the same major (M1) ---- */
  const [extras, setExtras] = useState<Record<number, Runtime[]>>({})
  const [expanded, setExpanded] = useState<Record<number, boolean>>({})
  const [loadingMajor, setLoadingMajor] = useState<number | null>(null)
  /** Per-extra-row install states, keyed by the precise runtime id. */
  const [extraStates, setExtraStates] = useState<Record<string, Runtime['state']>>({})
  const load = useCatalogStore((s) => s.load)

  const toggleMajor = async (major: number) => {
    if (expanded[major]) {
      setExpanded((cur) => ({ ...cur, [major]: false }))
      return
    }
    if (!extras[major]) {
      setLoadingMajor(major)
      try {
        const rows = await repository.listRuntimeVersions(major)
        setExtras((cur) => ({ ...cur, [major]: rows }))
      } catch (err) {
        useUIStore.getState().toast({
          kind: 'error',
          title: '补丁版本加载失败',
          message: parseThrownError(err).message || '无法连接 Node 发布源。',
        })
      } finally {
        setLoadingMajor(null)
      }
    }
    setExpanded((cur) => ({ ...cur, [major]: true }))
  }

  const installExtra = async (runtime: Runtime) => {
    if (isRuntimeBusyKind(extraStates[runtime.id]?.kind)) return
    const controller = new AbortController()
    setExtraStates((cur) => ({ ...cur, [runtime.id]: { kind: 'queued' } }))
    try {
      await repository.installRuntime(runtime, (progress) => {
        setExtraStates((cur) => ({
          ...cur,
          [runtime.id]:
            progress.stage === 'downloading'
              ? {
                  kind: 'downloading',
                  progress: progress.progress ?? 0,
                  bytesDone: progress.bytesDone ?? 0,
                  bytesPerSec: progress.bytesPerSec ?? 0,
                }
              : progress.stage === 'verifying'
                ? { kind: 'verifying' }
                : { kind: 'extracting', progress: progress.progress ?? 0 },
        }))
      }, controller.signal)
      setExtraStates((cur) => ({
        ...cur,
        [runtime.id]: { kind: 'installed', installedAt: new Date().toISOString() },
      }))
      useUIStore.getState().toast({ kind: 'success', title: `${runtime.name} ${runtime.version} 安装完成` })
      // 精确对象落盘后刷新列表，让已安装行与占用统计同步。
      void load()
    } catch (err) {
      setExtraStates((cur) => ({
        ...cur,
        [runtime.id]:
          err instanceof Cancelled
            ? { kind: 'available' }
            : { kind: 'failed', reason: '下载失败' },
      }))
      if (!(err instanceof Cancelled)) {
        useUIStore.getState().toast({
          kind: 'error',
          title: `${runtime.name} ${runtime.version} 安装失败`,
        })
      }
    }
  }

  const extraStateOf = (runtime: Runtime): Runtime['state'] =>
    extraStates[runtime.id] ??
    // 已安装的补丁版本刷新后会成为正式行；展开列表里的旧数据保持安静。
    (useCatalogStore.getState().runtimes.find((r) => r.id === runtime.id)?.state ?? {
      kind: 'available',
    })

  const onRemove = async (id: string, name: string) => {
    const users = usedBy.get(id) ?? []
    const ok = await confirm({
      title: `删除 ${name}`,
      message: users.length
        ? `有 ${users.length} 个实例绑定了这个 Runtime，删除后它们将无法启动。`
        : 'Runtime 会从本机移除。',
      detail: users.join('、') || undefined,
      tone: users.length ? 'danger' : 'default',
      confirmLabel: '删除',
    })
    if (ok) await remove(id)
  }

  return (
    <PageShell
      title="Node Runtime"
      subtitle="每个实例绑定自己的 Node 版本，与 DSH 版本解耦，互不影响。"
    >
      <Notice tone="info" className="mb-4">
        DSH 的不同版本对 Node 的要求可能不同。PHL 会在启动前检查实例的组合，不匹配时直接给出原因。
      </Notice>

      {!loaded ? (
        <div className="space-y-2">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-[72px]" />
          ))}
        </div>
      ) : (
        <motion.ul variants={stagger()} initial="hidden" animate="show" className="space-y-2">
          {runtimes.map((r) => {
            const state = r.state
            const installed = state.kind === 'installed'
            const busy = isRuntimeBusy(r)
            const users = usedBy.get(r.id) ?? []
            return (
              <motion.li key={r.id} variants={riseItem}>
                <div className="group/row overflow-hidden rounded-lg bg-surface px-3.5 py-3 ring-1 ring-inset ring-line transition-shadow duration-200 hover:ring-line-strong/70">
                  <div className="flex items-center gap-3">
                    <span
                      className={cn(
                        'flex h-9 w-9 shrink-0 items-center justify-center rounded-lg',
                        installed ? 'bg-ok/10 text-ok' : 'bg-surface-sunken text-ink-faint',
                      )}
                    >
                      {r.system ? <Cpu size={16} /> : <Server size={16} />}
                    </span>

                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-md font-medium text-ink">{r.name}</span>
                        <span className="font-mono text-sm text-ink-faint">v{r.version}</span>
                        {r.codename && <Badge tone="neutral">{r.codename}</Badge>}
                        {r.lts && <Badge tone="ok">LTS</Badge>}
                        {r.system && <Badge tone="outline">系统 PATH</Badge>}
                        {r.legacy && (
                          <Tooltip content="旧式安装目录（仅主版本）。新安装均为精确版本；绑定它的实例可转为精确绑定。">
                            <span>
                              <Badge tone="warn">旧式绑定</Badge>
                            </span>
                          </Tooltip>
                        )}
                      </div>
                      <div className="mt-1 flex items-center gap-2 text-sm text-ink-faint">
                        {/* 真实目录没有安装前体积，未知时不显示「0 B」。 */}
                        {installed
                          ? r.system
                            ? '由系统提供'
                            : `已安装${r.size > 0 ? ` · ${formatBytes(r.size)}` : ''}`
                          : r.size > 0
                            ? `未安装 · ${formatBytes(r.size)}`
                            : '未安装'}
                        {users.length > 0 && (
                          <>
                            <span className="text-ink-faint/50">·</span>
                            <Tooltip content={users.join('、')}>
                              <span className="text-accent-ink">{users.length} 个实例使用</span>
                            </Tooltip>
                          </>
                        )}
                      </div>
                      {r.system && r.path && (
                        <div className="mt-1 truncate font-mono text-xs text-ink-faint" title={r.path}>
                          {r.path}
                        </div>
                      )}
                    </div>

                    <div className="flex shrink-0 items-center gap-1.5">
                      {!r.system && (
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => void toggleMajor(r.major)}
                          disabled={loadingMajor === r.major}
                          className="opacity-0 transition-opacity group-hover/row:opacity-100 focus:opacity-100 data-[state=open]:opacity-100"
                          data-state={expanded[r.major] ? 'open' : 'closed'}
                        >
                          {loadingMajor === r.major ? (
                            <Spinner size={12} weight={2.6} />
                          ) : (
                            <ChevronDown
                              size={12}
                              className={cn('transition-transform', expanded[r.major] && 'rotate-180')}
                            />
                          )}
                          补丁版本
                        </Button>
                      )}
                      {busy ? (
                        <Button size="sm" variant="secondary" onClick={() => cancel(r.id)}>
                          <X size={12} />
                          取消
                        </Button>
                      ) : installed ? (
                        !r.system && (
                          <Button
                            size="sm"
                            variant="ghost"
                            onClick={() => void onRemove(r.id, r.name)}
                            className="opacity-0 transition-opacity group-hover/row:opacity-100 focus:opacity-100"
                          >
                            <Trash2 size={12} />
                            删除
                          </Button>
                        )
                      ) : (
                        <Button size="sm" variant="primary" onClick={() => void install(r.id)}>
                          <Download size={12} />
                          安装
                        </Button>
                      )}
                    </div>
                  </div>

                  <AnimatePresence initial={false}>
                    {busy && (
                      <motion.div
                        initial={{ height: 0, opacity: 0 }}
                        animate={{ height: 'auto', opacity: 1 }}
                        exit={{ height: 0, opacity: 0 }}
                        transition={t(0.24)}
                        className="overflow-hidden"
                      >
                        <div className="pt-3">
                          <ProgressBar
                            value={'progress' in state ? state.progress : 0}
                            indeterminate={state.kind === 'verifying' || state.kind === 'removing'}
                            active={state.kind === 'downloading'}
                            height={4}
                          />
                          <div className="mt-1.5 flex justify-between text-sm text-ink-faint">
                            <span>
                              {state.kind === 'removing'
                                ? '正在删除 Runtime 目录…'
                                : state.kind === 'downloading'
                                ? // 目录不发布体积；Rust 拿到的 content-length 体现在 progress 里，
                                  // 分母由 已完成字节 ÷ 进度 反推，比显示「0 B」诚实。
                                  state.progress > 0
                                  ? `${formatBytes(state.bytesDone)} / ${formatBytes(state.bytesDone / state.progress)}`
                                  : `${formatBytes(state.bytesDone)}（总大小未知）`
                                : state.kind === 'queued'
                                  ? '排队中，等待传输槽位…'
                                  : state.kind === 'verifying'
                                    ? '正在校验官方 SHASUMS…'
                                    : '正在解压…'}
                            </span>
                            <span className="num">
                              {state.kind === 'downloading' ? formatSpeed(state.bytesPerSec) : ''}
                            </span>
                          </div>
                        </div>
                      </motion.div>
                    )}
                  </AnimatePresence>

                  <AnimatePresence initial={false}>
                    {expanded[r.major] && extras[r.major] && (
                      <motion.div
                        initial={{ height: 0, opacity: 0 }}
                        animate={{ height: 'auto', opacity: 1 }}
                        exit={{ height: 0, opacity: 0 }}
                        transition={t(0.24)}
                        className="overflow-hidden"
                      >
                        <ul className="mt-2 space-y-1 border-t border-line pt-2">
                          {extras[r.major].map((extra) => {
                            const st = extraStateOf(extra)
                            const stInstalled = st.kind === 'installed'
                            const stBusy = isRuntimeBusyKind(st.kind)
                            return (
                              <li
                                key={extra.id}
                                className="flex items-center gap-3 rounded-md px-2 py-1.5 hover:bg-surface-sunken/60"
                              >
                                <span className="font-mono text-sm text-ink-muted">
                                  v{extra.version}
                                </span>
                                {extra.lts && <Badge tone="ok">LTS</Badge>}
                                <span className="flex-1 text-sm text-ink-faint">
                                  {stInstalled
                                    ? '已安装'
                                    : stBusy
                                      ? st.kind === 'downloading'
                                        ? `下载中 ${Math.round((st.progress ?? 0) * 100)}%`
                                        : st.kind === 'verifying'
                                          ? '校验中…'
                                          : '解压中…'
                                      : '与已安装版本并存，互不覆盖'}
                                </span>
                                {!stInstalled && (
                                  <Button
                                    size="sm"
                                    variant={stBusy ? 'secondary' : 'ghost'}
                                    disabled={stBusy}
                                    onClick={() => void installExtra(extra)}
                                  >
                                    {stBusy ? <Spinner size={12} weight={2.6} /> : <Download size={12} />}
                                    {stBusy ? '' : '安装'}
                                  </Button>
                                )}
                              </li>
                            )
                          })}
                        </ul>
                      </motion.div>
                    )}
                  </AnimatePresence>
                </div>
              </motion.li>
            )
          })}
        </motion.ul>
      )}

      <SectionCard title="Runtime 隔离范围" collapsible defaultOpen={false} className="mt-6">
        <div className="space-y-2 text-base leading-relaxed text-ink-muted">
          <p>实例启动时，PHL 只把它绑定的 Node 注入到该进程的环境变量里，不修改全局 PATH。</p>
          <p>
            插件依赖安装在实例自己的 <code className="font-mono text-sm">node_modules</code> 内，
            不同实例之间不会互相污染。
          </p>
        </div>
      </SectionCard>
    </PageShell>
  )
}
