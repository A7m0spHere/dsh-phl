import { Fragment, useMemo, useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  Archive,
  Check,
  ClipboardCopy,
  Download,
  ExternalLink,
  HardDrive,
  Package,
  RefreshCw,
  RotateCcw,
  Search,
  Trash2,
  X,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { buildAgentInstallTask, releaseUrl } from '@/lib/githubBuildTask'
import { registryBase } from '@/services/tauriVersions'
import { useSettingsStore } from '@/stores/settingsStore'
import { openExternal } from '@/lib/desktop'
import { formatBytes, formatDate, formatSpeed } from '@/lib/format'
import { useMotion } from '@/lib/motion'
import {
  useCatalogStore,
  useInstanceStore,
  useUIStore,
  useViewStore,
  type VersionFilter,
} from '@/stores'
import type { DshVersion } from '@/types'
import {
  Badge,
  Button,
  EmptyState,
  Input,
  Notice,
  ProgressBar,
  SectionCard,
  Skeleton,
  Tooltip,
} from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelDivider, PanelGroup, PanelItem, PanelShell, PanelStat } from '@/components/layout/Panel'

/* ------------------------------------------------------------------ *
 * panel
 * ------------------------------------------------------------------ */

export function VersionsPanel() {
  const versions = useCatalogStore((s) => s.versions)
  const filter = useViewStore((s) => s.versionFilter)
  const setFilter = useViewStore((s) => s.setVersionFilter)
  const query = useViewStore((s) => s.versionQuery)
  const setQuery = useViewStore((s) => s.setVersionQuery)

  const installed = versions.filter((v) => v.state.kind === 'installed')
  const counts: Record<VersionFilter, number> = {
    all: versions.length,
    installed: installed.length,
    available: versions.filter((v) => v.state.kind !== 'installed').length,
    legacy: versions.filter((v) => v.legacy).length,
  }

  const items: { id: VersionFilter; label: string; icon: ReactNode }[] = [
    { id: 'all', label: '全部版本', icon: <Package size={14} /> },
    { id: 'installed', label: '已安装', icon: <Check size={14} /> },
    { id: 'available', label: '可安装', icon: <Download size={14} /> },
    { id: 'legacy', label: 'Legacy', icon: <Archive size={14} /> },
  ]

  return (
    <PanelShell>
      <div className="p-2.5 pb-1">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索版本"
          prefix={<Search size={13} />}
        />
      </div>

      <PanelGroup>
        {items.map((it) => (
          <PanelItem
            key={it.id}
            groupId="version-filter"
            icon={it.icon}
            label={it.label}
            count={counts[it.id]}
            active={filter === it.id}
            onClick={() => setFilter(it.id)}
          />
        ))}
      </PanelGroup>

      <PanelDivider />

      <PanelGroup title="存储">
        <PanelStat label="已安装" value={`${installed.length} 个`} />
        <PanelStat
          label="占用"
          value={formatBytes(installed.reduce((sum, v) => sum + v.size, 0))}
        />
      </PanelGroup>

      <div className="mt-auto p-3">
        <div className="flex items-start gap-2 rounded-lg bg-surface-sunken p-3 text-sm leading-relaxed text-ink-faint ring-1 ring-inset ring-line">
          <HardDrive size={13} className="mt-[2px] shrink-0" />
          <span>多个版本可以同时安装。删除某个版本不会影响仍在使用它的实例配置，但这些实例将无法启动。</span>
        </div>
      </div>
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ *
 * row
 * ------------------------------------------------------------------ */

/**
 * Inline escape hatch for a pending release: hand the "build this version
 * from source" job to a DSH agent. PHL generates the task and copies it; the
 * user pastes it into a running instance where the *agent's own* approval
 * gates apply to every command — PHL never runs the build silently. The
 * warning follows the AUR/Homebrew shape: consequence → responsibility →
 * safer alternative, and the safe path (wait for the registry) stays adjacent.
 */
function AgentBuildGuide({ versionName, onClose }: { versionName: string; onClose: () => void }) {
  const toast = useUIStore((s) => s.toast)
  const root = useSettingsStore((s) => s.root)
  const registry = registryBase()
  const task = buildAgentInstallTask({
    versionName,
    root,
    registryBase: registry.includes('npmjs.org') ? undefined : registry,
  })

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(task)
      toast({ kind: 'success', title: '安装任务已复制', message: '粘贴到任一运行中的 DSH 对话框，逐步审批执行。', duration: 6000 })
    } catch {
      toast({ kind: 'error', title: '复制失败', message: '请手动选中下方文本复制。' })
    }
  }

  return (
    <div className="mt-3 rounded-lg bg-surface-sunken p-3.5 ring-1 ring-inset ring-line">
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="text-base font-medium text-ink">让 DSH agent 从源码构建 {versionName}</div>
          <p className="mt-1 text-sm leading-relaxed text-ink-muted">
            安装包源尚未收录此版本，但 GitHub 已发布源码。下面的任务会指导一个 DSH agent：克隆对应 tag → 安装依赖 → 构建 → 把产物放进 PHL 的 versions 目录并登记。
          </p>
        </div>
        <Button size="xs" variant="ghost" onClick={onClose} aria-label="关闭">
          <X size={13} />
        </Button>
      </div>

      <div className="mt-2.5 rounded-md bg-warn/10 px-2.5 py-2 text-sm leading-relaxed text-ink-muted ring-1 ring-inset ring-warn/25">
        <span className="font-medium text-ink">注意：</span>
        源码构建耗时可能数分钟、可能失败，产物未经官方发布流程签名校验；agent 执行的每条命令需你自行审批，构建结果由你负责。此过程会调用模型、消耗 token。
        <span className="text-ink-faint"> 推荐做法仍是等版本源收录后点「同步更新」自动安装。</span>
      </div>

      <details className="mt-2.5 group">
        <summary className="cursor-pointer list-none text-sm text-accent-ink hover:underline">
          查看安装任务全文
        </summary>
        <pre className="mt-2 max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-md bg-canvas p-2.5 font-mono text-xs leading-relaxed text-ink-muted ring-1 ring-inset ring-line">
          {task}
        </pre>
      </details>

      <div className="mt-3 flex flex-wrap items-center gap-1.5">
        <Button size="sm" variant="primary" onClick={() => void copy()}>
          <ClipboardCopy size={12} />
          复制安装任务
        </Button>
        <Button size="sm" variant="secondary" onClick={() => void openExternal(releaseUrl(versionName))}>
          <ExternalLink size={12} />
          在 GitHub 打开
        </Button>
      </div>
      <p className="mt-2 text-sm leading-relaxed text-ink-faint">
        用法：启动任一已安装版本的实例并打开 WebUI → 把任务粘贴给 agent → 逐步审批执行 → 完成后点本页右上角「同步更新」，PHL 会把构建出的版本登记为已安装。
      </p>
    </div>
  )
}

function VersionRow({ version, usedBy }: { version: DshVersion; usedBy: string[] }) {
  const install = useCatalogStore((s) => s.installVersion)
  const cancel = useCatalogStore((s) => s.cancelVersion)
  const remove = useCatalogStore((s) => s.removeVersion)
  const confirm = useUIStore((s) => s.confirm)
  const { t, riseItem } = useMotion()
  const [guideOpen, setGuideOpen] = useState(false)

  const state = version.state
  const installed = state.kind === 'installed'
  const busy = ['queued', 'downloading', 'extracting', 'verifying'].includes(state.kind)

  const onRemove = async () => {
    const ok = await confirm({
      title: `删除 DSH ${version.name}`,
      message: usedBy.length
        ? `有 ${usedBy.length} 个实例固定使用这个版本，删除后它们将无法启动，直到重新安装。`
        : '版本包会从本机移除，实例数据不受影响。',
      detail: usedBy.join('、') || undefined,
      tone: usedBy.length ? 'danger' : 'default',
      confirmLabel: '删除',
    })
    if (ok) await remove(version.id)
  }

  return (
    <motion.li variants={riseItem} layout="position" transition={t(0.24)}>
      <div
        className={cn(
          'group/row relative overflow-hidden rounded-lg bg-surface px-3.5 py-3 ring-1 ring-inset transition-[box-shadow,background-color] duration-200',
          state.kind === 'failed' ? 'ring-danger/25' : 'ring-line hover:ring-line-strong/70',
        )}
      >
        <div className="flex items-center gap-3">
          <span
            className={cn(
              'flex h-9 w-9 shrink-0 items-center justify-center rounded-lg',
              installed ? 'bg-ok/10 text-ok' : 'bg-surface-sunken text-ink-faint',
            )}
          >
            <Package size={16} />
          </span>

          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-md font-medium text-ink">{version.name}</span>
              {version.latest && <Badge tone="accent">最新</Badge>}
              {version.channel === 'nightly' && <Badge tone="warn">Nightly</Badge>}
              {version.channel === 'alpha' && <Badge tone="warn">Alpha</Badge>}
              {version.legacy && <Badge tone="neutral">Legacy</Badge>}
              {installed && state.installHealth === 'degraded' && (
                <Tooltip
                  allowOverflow
                  content={`部分依赖在安装时被跳过：${(state.skippedDependencies ?? []).join('、')}`}
                >
                  <Badge tone="warn">依赖降级</Badge>
                </Tooltip>
              )}
              {installed && <Badge tone="ok">已安装</Badge>}
              {version.pendingPublish && (
                <Tooltip
                  allowOverflow
                  content="GitHub 已发布此版本，但 npm 尚未上架安装包；上架后会自动提醒并可安装，也可让 agent 从源码构建"
                >
                  <Badge tone="warn">npm 未收录</Badge>
                </Tooltip>
              )}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-ink-faint">
              {[
                version.releasedAt ? <span key="date">{formatDate(version.releasedAt)}</span> : null,
                version.size > 0 ? <span key="size">{formatBytes(version.size)}</span> : null,
                version.requiresNode.length > 0 ? (
                  <span key="node">Node {version.requiresNode.join(' / ')}</span>
                ) : null,
              ]
                .filter(Boolean)
                .map((item, i) => (
                  <Fragment key={`meta-${i}`}>
                    {i > 0 && <span className="text-ink-faint/50">·</span>}
                    {item}
                  </Fragment>
                ))}
              {usedBy.length > 0 && (
                <>
                  <span className="text-ink-faint/50">·</span>
                  <Tooltip content={usedBy.join('、')}>
                    <span className="text-accent-ink">{usedBy.length} 个实例使用</span>
                  </Tooltip>
                </>
              )}
              {version.pendingPublish && (
                <>
                  <span className="text-ink-faint/50">·</span>
                  <button
                    className="text-accent-ink transition-colors hover:underline"
                    onClick={() => void openExternal('https://www.npmjs.com/package/@deepseek-ai/dsh')}
                  >
                    查看 npm 包
                  </button>
                </>
              )}
            </div>
          </div>

          <div className="flex shrink-0 items-center gap-1.5">
            {busy ? (
              <Button size="sm" variant="secondary" onClick={() => cancel(version.id)}>
                <X size={12} />
                取消
              </Button>
            ) : installed ? (
              <Button
                size="sm"
                variant="ghost"
                onClick={onRemove}
                className="opacity-0 transition-opacity group-hover/row:opacity-100 focus:opacity-100"
              >
                <Trash2 size={12} />
                删除
              </Button>
            ) : version.pendingPublish ? (
              <Button size="sm" variant="secondary" onClick={() => setGuideOpen((v) => !v)}>
                <Package size={12} />
                让 agent 构建
              </Button>
            ) : state.kind === 'failed' ? (
              <Button size="sm" variant="secondary" onClick={() => void install(version.id)}>
                <RotateCcw size={12} />
                重试
              </Button>
            ) : (
              <Button size="sm" variant="primary" onClick={() => void install(version.id)}>
                <Download size={12} />
                安装
              </Button>
            )}
          </div>
        </div>

        {/* download / extract feedback */}
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
                  value={'progress' in state ? state.progress : 0.97}
                  indeterminate={state.kind === 'queued'}
                  active={state.kind === 'downloading'}
                  height={4}
                />
                <div className="mt-1.5 flex items-center justify-between text-sm text-ink-faint">
                  <span>
                    {state.kind === 'queued'
                      ? '排队中…'
                      : state.kind === 'downloading'
                        ? `${formatBytes(state.bytesDone)} / ${formatBytes(version.size)}`
                        : state.kind === 'extracting'
                          ? '正在解压到版本目录…'
                          : '正在校验完整性…'}
                  </span>
                  <span className="num">
                    {state.kind === 'downloading'
                      ? formatSpeed(state.bytesPerSec)
                      : 'progress' in state
                        ? `${Math.round(state.progress * 100)}%`
                        : ''}
                  </span>
                </div>
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        <AnimatePresence initial={false}>
          {version.pendingPublish && guideOpen && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: 'auto', opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={t(0.24)}
              className="overflow-hidden"
            >
              <AgentBuildGuide versionName={version.name} onClose={() => setGuideOpen(false)} />
            </motion.div>
          )}
        </AnimatePresence>

        {state.kind === 'failed' && (
          <div className="mt-2.5 text-sm text-danger">{state.reason}</div>
        )}

        {version.notes.length > 0 && (
          <div className="mt-2.5 border-t border-line pt-2.5">
            <ul className="space-y-0.5">
              {version.notes.map((note, i) => (
                <li key={i} className="flex gap-2 text-sm text-ink-muted">
                  <span className="text-ink-faint/60">·</span>
                  {note}
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </motion.li>
  )
}

/* ------------------------------------------------------------------ *
 * page
 * ------------------------------------------------------------------ */

/** "14:32" — the sync label only cares about time of day. */
function formatClock(ms: number) {
  const d = new Date(ms)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

export function VersionsPage() {
  const versions = useCatalogStore((s) => s.versions)
  const loaded = useCatalogStore((s) => s.versionsLoaded)
  const syncing = useCatalogStore((s) => s.versionsSyncing)
  const syncedAt = useCatalogStore((s) => s.versionsSyncedAt)
  const refreshVersions = useCatalogStore((s) => s.refreshVersions)
  const instances = useInstanceStore((s) => s.instances)
  const filter = useViewStore((s) => s.versionFilter)
  const query = useViewStore((s) => s.versionQuery)
  const { stagger } = useMotion()

  const usedBy = useMemo(() => {
    const map = new Map<string, string[]>()
    for (const i of instances) {
      map.set(i.versionId, [...(map.get(i.versionId) ?? []), i.name])
    }
    return map
  }, [instances])

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase()
    return versions
      .filter((v) => {
        if (filter === 'installed' && v.state.kind !== 'installed') return false
        if (filter === 'available' && v.state.kind === 'installed') return false
        if (filter === 'legacy' && !v.legacy) return false
        return !q || v.name.toLowerCase().includes(q)
      })
      .sort((a, b) => new Date(b.releasedAt).getTime() - new Date(a.releasedAt).getTime())
  }, [versions, filter, query])

  const orphaned = useMemo(
    () =>
      instances.filter(
        (i) => versions.find((v) => v.id === i.versionId)?.state.kind !== 'installed',
      ),
    [instances, versions],
  )

  return (
    <PageShell
      title="DSH 版本"
      subtitle="多个版本可以同时安装在本机。实例固定引用其中一个，升级不会覆盖旧版本。"
      actions={
        <>
          {syncedAt !== null && (
            <span className="mr-1 text-sm text-ink-faint">
              上次同步 {syncing ? '中…' : formatClock(syncedAt)}
            </span>
          )}
          <Button
            size="sm"
            variant="secondary"
            onClick={() => void refreshVersions()}
            disabled={syncing}
          >
            <RefreshCw size={12} className={cn(syncing && 'animate-spin')} />
            {syncing ? '同步中' : '同步更新'}
          </Button>
        </>
      }
    >
      {orphaned.length > 0 && (
        <Notice tone="warn" title="有实例引用了未安装的版本" className="mb-4">
          {orphaned.map((i) => i.name).join('、')} 暂时无法启动。安装对应版本后即可恢复。
        </Notice>
      )}

      {!loaded ? (
        <div className="space-y-2">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-[92px]" />
          ))}
        </div>
      ) : visible.length === 0 ? (
        <EmptyState icon={<Package size={20} />} title="没有匹配的版本" description="换一个筛选条件试试。" />
      ) : (
        <motion.ul variants={stagger()} initial="hidden" animate="show" className="space-y-2">
          {visible.map((v) => (
            <VersionRow key={v.id} version={v} usedBy={usedBy.get(v.id) ?? []} />
          ))}
        </motion.ul>
      )}

      <SectionCard title="关于版本隔离" collapsible defaultOpen={false} className="mt-6">
        <div className="space-y-2 text-base leading-relaxed text-ink-muted">
          <p>
            每个版本安装在 <code className="font-mono text-sm">versions/</code> 下的独立目录，实例通过 id
            引用它，而不是依赖全局安装。
          </p>
          <p>
            这意味着同一台机器上可以同时存在 rc.5 与 rc.8，并由不同实例分别使用；升级一个实例不会影响另一个。
          </p>
        </div>
      </SectionCard>
    </PageShell>
  )
}
