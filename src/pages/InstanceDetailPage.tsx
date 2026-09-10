import { useEffect, useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  Camera,
  Copy,
  ExternalLink,
  FolderOpen,
  MessageSquare,
  MoreHorizontal,
  Pencil,
  Play,
  Plug,
  RotateCcw,
  Square,
  Terminal,
  Trash2,
  X,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { isDesktop as isDesktopFlag } from '@/lib/desktopCore'
import { instanceSessionCount } from '@/lib/desktop'
import { resolveBoundVersion } from '@/lib/instanceVersion'
import { formatBytes, formatDateTime, formatDuration, formatRelative } from '@/lib/format'
import { useUptime } from '@/lib/hooks'
import { useMotion } from '@/lib/motion'
import { linkedPluginNames } from '@/data/instances'
import { useCatalogStore, useInstanceStore, useUIStore } from '@/stores'
import { latestRelease } from '@/types'
import { isVersionBusy } from '@/types/version'
import {
  Badge,
  Button,
  Chip,
  DataRow,
  EmptyState,
  Field,
  IconButton,
  Input,
  Menu,
  Notice,
  ProgressBar,
  SectionCard,
  Select,
  Switch,
  Tooltip,
} from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelDivider, PanelGroup, PanelItem, PanelShell, PanelStat } from '@/components/layout/Panel'
import { InstanceTile, LaunchTimeline, StatusPill, useInstanceActions } from '@/components/instance'
import { SessionCopyPanel } from '@/components/instance/SessionCopyPanel'
import { ApiBindingCard } from '@/components/instance/ApiBindingCard'
import { EnvironmentHealthCard } from '@/components/instance/EnvironmentHealthCard'

/* ------------------------------------------------------------------ *
 * context panel — sibling instances, so switching stays one click away
 * ------------------------------------------------------------------ */

export function InstanceDetailPanel({ id }: { id: string }) {
  const instances = useInstanceStore((s) => s.instances)
  const states = useInstanceStore((s) => s.states)
  const push = useUIStore((s) => s.push)
  const instance = instances.find((i) => i.id === id)
  const version = useCatalogStore((s) => resolveBoundVersion(s.versions, instance?.versionId))
  const runtime = useCatalogStore((s) => s.runtimes.find((r) => r.id === instance?.runtimeId))

  return (
    <PanelShell>
      <PanelGroup title="所有实例">
        {instances.map((i) => (
          <PanelItem
            key={i.id}
            groupId="detail-instances"
            active={i.id === id}
            label={i.name}
            icon={
              <span
                className={cn(
                  'block h-[7px] w-[7px] rounded-full',
                  states[i.id]?.status === 'running'
                    ? 'bg-ok'
                    : states[i.id]?.status === 'error'
                      ? 'bg-danger'
                      : 'bg-ink-faint/40',
                )}
              />
            }
            count={`:${i.port}`}
            onClick={() => push({ name: 'instance', id: i.id })}
          />
        ))}
      </PanelGroup>

      {instance && (
        <>
          <PanelDivider />
          <PanelGroup title="环境摘要">
            <PanelStat label="DSH" value={version?.name ?? '未知'} />
            <PanelStat label="Runtime" value={runtime?.name ?? '未知'} />
            <PanelStat label="插件" value={instance.plugins.length} />
            <PanelStat label="端口" value={`:${instance.port}`} />
            <PanelStat label="占用" value={formatBytes(instance.diskUsage)} />
          </PanelGroup>
        </>
      )}
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ *
 * page
 * ------------------------------------------------------------------ */

export function InstanceDetailPage({ id }: { id: string }) {
  const instance = useInstanceStore((s) => s.instances.find((i) => i.id === id))
  const state = useInstanceStore((s) => s.states[id]) ?? { status: 'stopped' as const }
  const toggle = useInstanceStore((s) => s.toggle)
  const launch = useInstanceStore((s) => s.launch)
  const dismissError = useInstanceStore((s) => s.dismissError)
  const setFocus = useInstanceStore((s) => s.setFocus)
  const update = useInstanceStore((s) => s.updateInstance)
  const restoreSnapshot = useInstanceStore((s) => s.restoreSnapshot)
  const deleteSnapshot = useInstanceStore((s) => s.deleteSnapshot)
  const snapshotTransfer = useInstanceStore((s) => s.snapshotTransfers[id])
  const snapshotOp = useInstanceStore((s) => s.snapshotOps[id])
  const cancelSnapshot = useInstanceStore((s) => s.cancelSnapshot)
  const deletingSnapshots = useInstanceStore((s) => s.deletingSnapshots)
  const confirm = useUIStore((s) => s.confirm)
  const versions = useCatalogStore((s) => s.versions)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const plugins = useCatalogStore((s) => s.plugins)
  const navigate = useUIStore((s) => s.navigate)
  const toast = useUIStore((s) => s.toast)
  const { t, stagger, riseItem } = useMotion()
  const actions = useInstanceActions(instance)
  const uptime = useUptime(state.status === 'running' ? state.startedAt : undefined)

  // Viewing an instance *is* aiming the launcher at it: the dock below the
  // list page and the cards' focus ring always agree with this route.
  useEffect(() => {
    setFocus(id)
  }, [id, setFocus])

  // History-conversation count is read from disk, not stored on the record —
  // it changes as the user talks to DSH. `null` = measuring, and a failure
  // reads as "不可用" rather than lying with 0.
  const [sessionCount, setSessionCount] = useState<number | null>(null)
  // Copying conversations in/out moves the on-disk count without touching the
  // record; bumping this tick re-runs the measurement so the badge is never
  // a stale lie right under the user's eyes.
  const [sessionCountTick, setSessionCountTick] = useState(0)
  useEffect(() => {
    let alive = true
    setSessionCount(null)
    // Browser mode has no filesystem to count; the bridge would answer 0 and
    // the UI would report a confidently wrong number. Stay in "不可查" state.
    if (instance && isDesktopFlag) {
      instanceSessionCount(instance.id)
        .then((n) => alive && setSessionCount(n))
        .catch(() => alive && setSessionCount(null))
    }
    return () => {
      alive = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [instance?.id, instance?.plugins.length, sessionCountTick])

  // Editing the pinned environment is a deliberate, explicit act — the whole
  // point of an instance is that its version does not drift on its own.
  const [editing, setEditing] = useState(false)
  const [edit, setEdit] = useState({
    versionId: '',
    runtimeId: '',
    port: 3080,
    autoPort: false,
    profile: '',
  })

  const beginEdit = () => {
    if (!instance) return
    setEdit({
      // Start editing from the RESOLVED id: a legacy bare-version binding
      // becomes canonical on save, so the bad value cannot outlive the edit.
      versionId: resolveBoundVersion(versions, instance.versionId)?.id ?? instance.versionId,
      runtimeId: instance.runtimeId,
      port: instance.port,
      autoPort: instance.autoPort,
      profile: instance.profile,
    })
    setEditing(true)
  }

  const saveEdit = () => {
    if (!instance) return
    update(instance.id, {
      versionId: edit.versionId,
      runtimeId: edit.runtimeId,
      port: edit.port,
      autoPort: edit.autoPort,
      profile: edit.profile.trim() || 'default',
    })
    setEditing(false)
    toast({
      kind: 'success',
      title: '实例配置已更新',
      message: '下次启动时生效。',
    })
  }

  const version = resolveBoundVersion(versions, instance?.versionId)
  const runtime = runtimes.find((r) => r.id === instance?.runtimeId)

  const pluginRows = useMemo(() => {
    if (!instance) return []
    return instance.plugins.map((installed) => {
      const meta = plugins.find((p) => p.id === installed.pluginId)
      const newest = meta ? latestRelease(meta) : undefined
      const outdated = !!newest && !installed.linked && newest.version !== installed.version
      const incompatible =
        !!newest &&
        !!meta &&
        !!instance &&
        !meta.releases
          .find((r) => r.version === installed.version)
          ?.compatible.includes(instance.versionId)
      return {
        ...installed,
        name: meta?.name ?? linkedPluginNames[installed.pluginId] ?? installed.pluginId,
        official: meta?.official,
        newestVersion: newest?.version,
        outdated,
        incompatible,
      }
    })
  }, [instance, plugins])

  if (!instance) {
    return (
      <PageShell title="实例不存在">
        <EmptyState
          title="找不到这个实例"
          description="它可能已经被删除了。"
          action={
            <Button variant="primary" onClick={() => navigate({ name: 'instances' })}>
              返回实例列表
            </Button>
          }
        />
      </PageShell>
    )
  }

  const status = state.status
  const running = status === 'running'
  const busy = status === 'starting' || status === 'stopping'
  const failed = status === 'error'
  const outdatedCount = pluginRows.filter((p) => p.outdated).length

  /**
   * Routed through the catalog store, not `updateInstance`: the enabled state
   * lives in the profile's `cordis.patch.yml` and is read back from there, so
   * patching only the in-memory record left the switch reverting on the next
   * load while DSH kept loading the plugin.
   */
  const togglePlugin = (pluginId: string, enabled: boolean) => {
    void useCatalogStore.getState().setPluginEnabled(instance.id, pluginId, enabled)
  }

  const copy = (text: string, label: string) => {
    void navigator.clipboard?.writeText(text)
    toast({ kind: 'info', title: `已复制${label}`, message: text, duration: 2400 })
  }

  return (
    <PageShell
      title={
        <div className="flex items-center gap-3">
          <InstanceTile
            name={instance.name}
            hue={instance.hue}
            status={status}
            size={40}
            layoutId={`tile-${instance.id}`}
          />
          <div className="min-w-0">
            <div className="flex items-center gap-2.5">
              <span className="truncate">{instance.name}</span>
              <StatusPill status={status} startedAt={state.startedAt} />
              {state.lastExit && (
                <Tooltip
                  content={`进程异常退出（退出码 ${state.lastExit.code ?? '未知'}，运行 ${state.lastExit.ranFor}s）；日志在实例目录 logs/ 下，下次启动后消失`}
                >
                  <Badge tone="danger">上次退出 {state.lastExit.code ?? '?'}</Badge>
                </Tooltip>
              )}
              {instance?.managementMode === 'external' && (
                <Tooltip content="DSH_HOME 在你的目录下，PHL 负责启动与查看；写操作被禁用。">
                  <Badge tone="warn">原地接入</Badge>
                </Tooltip>
              )}
              {instance?.source === 'adopted' && instance?.managementMode === 'managed-copy' && (
                <Badge tone="neutral">接入的副本</Badge>
              )}
            </div>
            <div className="mt-1 truncate text-base font-normal text-ink-muted">
              {instance.note ?? '没有备注'}
            </div>
          </div>
        </div>
      }
      actions={
        <div className="flex items-center gap-2">
          {running && (
            <Tooltip content={`localhost:${instance.port}`}>
              <Button variant="secondary" onClick={actions.openWebUI}>
                <ExternalLink size={13} />
                打开 WebUI
              </Button>
            </Tooltip>
          )}
          <Button
            variant={running ? 'secondary' : 'primary'}
            size="lg"
            className="group/sheen min-w-[104px]"
            sheen={!running && !busy}
            onClick={() => {
              if (failed) {
                dismissError(instance.id)
                void launch(instance.id)
              } else toggle(instance.id)
            }}
          >
            {running ? (
              <>
                <Square size={13} /> 停止
              </>
            ) : busy ? (
              <>
                <X size={13} /> 取消
              </>
            ) : failed ? (
              <>
                <RotateCcw size={13} /> 重试
              </>
            ) : (
              <>
                <Play size={13} className="fill-current" /> 启动
              </>
            )}
          </Button>
          <Menu
            items={actions.menuItems}
            trigger={({ toggle: openMenu, menuProps }) => (
              <IconButton
                label="更多操作"
                size="lg"
                variant="secondary"
                onClick={openMenu}
                {...menuProps}
              >
                <MoreHorizontal size={15} />
              </IconButton>
            )}
          />
        </div>
      }
    >
      {/* ---- live状态区：启动中 / 失败 ---- */}
      <AnimatePresence initial={false} mode="popLayout">
        {status === 'starting' && (
          <motion.div
            key="timeline"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: 'auto' }}
            exit={{ opacity: 0, height: 0 }}
            transition={t(0.26)}
            className="overflow-hidden"
          >
            <div className="mb-4 rounded-lg bg-accent-soft/50 p-4 ring-1 ring-inset ring-accent/15">
              <LaunchTimeline state={state} />
            </div>
          </motion.div>
        )}

        {failed && state.error && (
          <motion.div
            key="error"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: 'auto' }}
            exit={{ opacity: 0, height: 0 }}
            transition={t(0.26)}
            className="overflow-hidden"
          >
            <Notice tone="danger" title={state.error.title} className="mb-4">
              <p>{state.error.detail}</p>
              {state.error.hint && <p className="mt-1 text-ink-faint">{state.error.hint}</p>}
              <div className="mt-2.5 flex gap-2">
                {(!version || version.state.kind !== 'installed') && (
                  <Button size="sm" variant="primary" onClick={() => navigate({ name: 'versions' })}>
                    {version ? `去安装 DSH ${version.name}` : '去「版本」页安装 DSH'}
                  </Button>
                )}
                {runtime && runtime.state.kind !== 'installed' && (
                  <Button size="sm" variant="primary" onClick={() => navigate({ name: 'runtimes' })}>
                    去安装 {runtime.name}
                  </Button>
                )}
                <Button size="sm" variant="secondary" onClick={() => dismissError(instance.id)}>
                  忽略
                </Button>
              </div>
            </Notice>
          </motion.div>
        )}
      </AnimatePresence>

      <motion.div variants={stagger(0.04)} initial="hidden" animate="show" className="space-y-3">
        {/* ---- 环境 ---- */}
        <motion.div variants={riseItem}>
          <SectionCard
            title="运行环境"
            icon={<Terminal size={14} />}
            extra={
              editing ? (
                <>
                  <Button size="xs" variant="ghost" onClick={() => setEditing(false)}>
                    取消
                  </Button>
                  <Button size="xs" variant="primary" onClick={saveEdit}>
                    保存
                  </Button>
                </>
              ) : (
                <Tooltip content={running || busy ? '停止实例后才能修改绑定' : '修改版本、Runtime 与端口'}>
                  <Button size="xs" variant="ghost" onClick={beginEdit} disabled={running || busy}>
                    <Pencil size={11} />
                    编辑
                  </Button>
                </Tooltip>
              )
            }
          >
            <AnimatePresence mode="wait" initial={false}>
              {editing ? (
                <motion.div
                  key="edit"
                  initial={{ opacity: 0, y: 6 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -4 }}
                  transition={t(0.18)}
                  className="grid gap-4 md:grid-cols-2"
                >
                  <Field
                    label="DSH 版本"
                    hint="实例将永久固定这个版本，直到你再次修改"
                  >
                    <Select
                      value={edit.versionId}
                      onChange={(e) => setEdit({ ...edit, versionId: e.target.value })}
                    >
                      {/* A controlled select renders blank when its value is
                          not among the options — which is exactly the state a
                          legacy or unbound instance opens in. Keep the
                          current value visible so the dropdown is never an
                          empty mystery. */}
                      {!versions.some((v) => v.id === edit.versionId) && (
                        <option value={edit.versionId}>
                          {edit.versionId ? `当前绑定：${edit.versionId}（未识别）` : '未绑定版本'}
                        </option>
                      )}
                      {versions.map((v) => (
                        <option key={v.id} value={v.id}>
                          {v.name}
                          {v.state.kind === 'installed' ? '' : '（未安装）'}
                        </option>
                      ))}
                    </Select>
                  </Field>

                  <Field
                    label="Runtime"
                    error={
                      (() => {
                        const nv = versions.find((v) => v.id === edit.versionId)
                        const nr = runtimes.find((r) => r.id === edit.runtimeId)
                        return nv && nr && nv.requiresNode.length > 0 && !nv.requiresNode.includes(nr.major)
                          ? `${nv.name} 未在 ${nr.name} 上验证过`
                          : null
                      })()
                    }
                  >
                    <Select
                      value={edit.runtimeId}
                      onChange={(e) => setEdit({ ...edit, runtimeId: e.target.value })}
                    >
                      {runtimes.map((r) => (
                        <option key={r.id} value={r.id}>
                          {r.name} (v{r.version})
                          {r.state.kind === 'installed' ? '' : ' — 未安装'}
                        </option>
                      ))}
                    </Select>
                  </Field>

                  <Field label="Profile">
                    <Input
                      value={edit.profile}
                      onChange={(e) => setEdit({ ...edit, profile: e.target.value })}
                      placeholder="default"
                    />
                  </Field>

                  <Field label="端口" hint={edit.autoPort ? '启动时自动选择空闲端口' : undefined}>
                    <div className="flex items-center gap-3">
                      <Input
                        type="number"
                        min={1024}
                        max={65535}
                        disabled={edit.autoPort}
                        value={edit.port}
                        onChange={(e) => setEdit({ ...edit, port: Number(e.target.value) })}
                        prefix="localhost:"
                        className="flex-1"
                      />
                      <span className="flex shrink-0 items-center gap-2 text-sm text-ink-muted">
                        自动
                        <Switch
                          checked={edit.autoPort}
                          onChange={(v) => setEdit({ ...edit, autoPort: v })}
                          label="自动分配端口"
                        />
                      </span>
                    </div>
                  </Field>
                </motion.div>
              ) : (
                <motion.div
                  key="view"
                  initial={{ opacity: 0, y: 6 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -4 }}
                  transition={t(0.18)}
                  className="grid gap-x-8 md:grid-cols-2"
                >
              <div>
                <DataRow
                  label="DSH 版本"
                  value={
                    <span className="flex items-center gap-2">
                      {version?.name ?? (instance.versionId || '未绑定版本')}
                      {version ? (
                        version.state.kind === 'installed' ? (
                          <Badge tone="ok">已安装</Badge>
                        ) : isVersionBusy(version) ? (
                          <Badge tone="warn">安装中</Badge>
                        ) : (
                          <Badge tone="warn">未安装</Badge>
                        )
                      ) : (
                        <Badge tone="warn">未绑定</Badge>
                      )}
                      {version?.legacy && <Badge tone="neutral">Legacy</Badge>}
                    </span>
                  }
                />
                <DataRow
                  label="Runtime"
                  value={
                    <span className="flex items-center gap-2">
                      {runtime?.name ?? instance.runtimeId}
                      <span className="font-mono text-sm text-ink-faint">v{runtime?.version}</span>
                      {version && runtime && version.requiresNode.length > 0 && !version.requiresNode.includes(runtime.major) && (
                        <Badge tone="warn">版本不匹配</Badge>
                      )}
                    </span>
                  }
                />
                <DataRow label="Profile" value={instance.profile} />
                {instance.managementMode === 'external' && (
                  <p className="mt-1 text-xs leading-relaxed text-ink-faint">
                    原地接入：源 DSH 自带的可执行程序不在 PHL 管理范围内，其版本也不会被读取；启动时由上方绑定的版本运行这份环境。
                  </p>
                )}
              </div>
              <div>
                <DataRow
                  label="端口"
                  value={
                    <span className="flex items-center gap-2">
                      {instance.port ? (
                        <Chip>:{instance.port}</Chip>
                      ) : (
                        <span className="text-sm text-ink-faint">未分配</span>
                      )}
                      {instance.autoPort && <span className="text-sm text-ink-faint">自动分配</span>}
                    </span>
                  }
                  action={
                    <IconButton
                      label="复制地址"
                      size="xs"
                      variant="ghost"
                      onClick={actions.copyPort}
                    >
                      <Copy size={11} />
                    </IconButton>
                  }
                />
                <DataRow
                  label="进程"
                  value={
                    running && state.pid ? (
                      <span className="num">PID {state.pid}</span>
                    ) : (
                      <span className="text-ink-faint">未运行</span>
                    )
                  }
                />
                <DataRow
                  label="本次运行"
                  value={
                    running ? (
                      <span className="num">{formatDuration(uptime)}</span>
                    ) : (
                      <span className="text-ink-faint">—</span>
                    )
                  }
                />
              </div>
                </motion.div>
              )}
            </AnimatePresence>
          </SectionCard>
        </motion.div>

        {/* ---- 隔离路径 ---- */}
        <motion.div variants={riseItem}>
          <SectionCard
            title="隔离路径"
            icon={<FolderOpen size={14} />}
            description="这些目录只属于该实例"
            collapsible
            extra={
              <Button size="xs" variant="ghost" onClick={actions.revealFolder}>
                打开目录
              </Button>
            }
          >
            <DataRow
              label="DSH_HOME"
              value={instance.dshHome}
              mono
              action={
                <IconButton
                  label="复制"
                  size="xs"
                  variant="ghost"
                  onClick={() => copy(instance.dshHome, 'DSH_HOME')}
                >
                  <Copy size={11} />
                </IconButton>
              }
            />
            <DataRow
              label="Workspace"
              value={instance.workspace}
              mono
              action={
                <IconButton
                  label="复制"
                  size="xs"
                  variant="ghost"
                  onClick={() => copy(instance.workspace, 'Workspace 路径')}
                >
                  <Copy size={11} />
                </IconButton>
              }
            />
            <DataRow
              label="启动参数"
              value={
                instance.args.length ? (
                  <span className="font-mono text-sm">{instance.args.join(' ')}</span>
                ) : (
                  <span className="text-ink-faint">无</span>
                )
              }
            />
            <DataRow
              label="环境变量"
              value={
                Object.keys(instance.env).length ? (
                  <span className="flex flex-wrap gap-1.5">
                    {Object.entries(instance.env).map(([k, v]) => (
                      <Chip key={k}>
                        {k}={v}
                      </Chip>
                    ))}
                  </span>
                ) : (
                  <span className="text-ink-faint">继承默认值</span>
                )
              }
            />
          </SectionCard>
        </motion.div>

        {/* ---- 数据：会话真源在 DSH，这里只提供迁移视角 ---- */}
        <motion.div variants={riseItem}>
          <SectionCard
            title="数据"
            icon={<MessageSquare size={14} />}
            description="历史对话以 DSH 自身存储为真源；浏览与继续对话在 DSH 里完成。"
          >
            <DataRow
              label="历史对话"
              value={
                sessionCount === null ? (
                  <span className="text-ink-faint">{isDesktopFlag ? '统计中…' : '桌面端可查'}</span>
                ) : (
                  `${sessionCount} 条`
                )
              }
            />
            {instance.adoptedFrom && (
              <DataRow
                label="来源"
                value={
                  <span className="text-sm">
                    {instance.adoptedFrom.mode === 'external' ? '原地接入' : '接入副本'}：
                    {instance.adoptedFrom.dshHome}
                    {instance.adoptedFrom.detectedVersion
                      ? `（DSH ${instance.adoptedFrom.detectedVersion}）`
                      : ''}
                  </span>
                }
              />
            )}
            {instance.managementMode === 'external' && (
              <p className="mt-1 text-xs text-ink-faint">
                原地接入的会话数据始终保存在你自己的 DSH_HOME；「删除实例」只会把登记从 PHL 移除。
              </p>
            )}
            {/* Mounted through a re-measure (copy just landed) on purpose: the
                panel's own selection must survive the count refresh. */}
            <SessionCopyPanel
              instanceId={instance.id}
              onCopied={() => setSessionCountTick((t) => t + 1)}
            />
          </SectionCard>
        </motion.div>

        {/* ---- 模型与 API ---- */}
        <motion.div variants={riseItem}>
          <ApiBindingCard instance={instance} />
        </motion.div>

        {/* ---- 插件 ---- */}
        <motion.div variants={riseItem}>
          <SectionCard
            title="插件"
            icon={<Plug size={14} />}
            description={`${instance.plugins.filter((p) => p.enabled).length} / ${instance.plugins.length} 已启用`}
            collapsible
            extra={
              <>
                {outdatedCount > 0 && <Badge tone="accent">{outdatedCount} 个可更新</Badge>}
                <Button size="xs" variant="ghost" onClick={() => navigate({ name: 'plugins' })}>
                  管理插件
                </Button>
              </>
            }
            bodyClassName="px-0 py-1"
          >
            {pluginRows.length === 0 ? (
              <EmptyState
                compact
                title="这个实例还没有插件"
                description="插件安装在实例内部，不会影响其他实例。"
                action={
                  <Button size="sm" variant="secondary" onClick={() => navigate({ name: 'plugins' })}>
                    浏览插件
                  </Button>
                }
              />
            ) : (
              <ul>
                {pluginRows.map((p) => (
                  <li
                    key={p.pluginId}
                    className="group/plugin flex items-center gap-3 px-4 py-[7px] transition-colors hover:bg-surface-hover/60"
                  >
                    <span
                      className={cn(
                        'h-[6px] w-[6px] shrink-0 rounded-full',
                        p.enabled ? 'bg-ok' : 'bg-ink-faint/35',
                      )}
                    />
                    <span
                      className={cn(
                        'min-w-0 flex-1 truncate text-base',
                        p.enabled ? 'text-ink' : 'text-ink-faint',
                      )}
                    >
                      {p.name}
                    </span>
                    {p.linked && <Badge tone="accent">本地链接</Badge>}
                    {p.incompatible && <Badge tone="warn">兼容性未知</Badge>}
                    {p.outdated && <Badge tone="accent">→ {p.newestVersion}</Badge>}
                    <span className="num w-14 shrink-0 text-right font-mono text-sm text-ink-faint">
                      {p.version}
                    </span>
                    <Switch
                      checked={p.enabled}
                      onChange={(v) => togglePlugin(p.pluginId, v)}
                      label={`启用 ${p.name}`}
                    />
                  </li>
                ))}
              </ul>
            )}
          </SectionCard>
        </motion.div>

        {/* ---- 环境健康 ---- */}
        <motion.div variants={riseItem}>
          <EnvironmentHealthCard instanceId={instance.id} />
        </motion.div>

        {/* ---- 快照 ---- */}
        <motion.div variants={riseItem}>
          <SectionCard
            title="快照"
            icon={<Camera size={14} />}
            description={`${instance.snapshots.length} 个`}
            collapsible
            defaultOpen={instance.snapshots.length > 0}
            extra={
              <Tooltip content={running || busy ? '请先停止实例，再创建快照' : ''}>
                <Button
                  size="xs"
                  variant="ghost"
                  disabled={!!snapshotOp || running || busy}
                  onClick={actions.snapshot}
                >
                  {snapshotOp === 'create' ? '创建中…' : '创建快照'}
                </Button>
              </Tooltip>
            }
          >
            {snapshotTransfer && (
              <div className="mb-3 rounded bg-surface-sunken px-3 py-2 ring-1 ring-inset ring-line">
                <ProgressBar value={snapshotTransfer.progress} active height={4} />
                <div className="mt-1.5 flex items-center justify-between text-sm text-ink-faint">
                  <span>
                    {snapshotOp === 'restore'
                      ? '正在还原 dsh-home…（交换阶段不可取消）'
                      : '正在复制 dsh-home…'}
                  </span>
                  <span className="flex items-center gap-2">
                    <span className="num">
                      {formatBytes(snapshotTransfer.bytesDone)} /{' '}
                      {formatBytes(snapshotTransfer.bytesTotal)}
                    </span>
                    <Button size="xs" variant="ghost" onClick={() => cancelSnapshot(instance.id)}>
                      取消
                    </Button>
                  </span>
                </div>
              </div>
            )}
            {instance.snapshots.length === 0 ? (
              <EmptyState
                compact
                title="还没有快照"
                description="快照会记录当前的 DSH 版本、Runtime、插件与配置。回滚只替换 dsh-home（插件与配置），不会改变实例引用的 DSH 与 Runtime。"
                action={
                  <Tooltip content={running || busy ? '请先停止实例，再创建快照' : ''}>
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={running || busy}
                      onClick={actions.snapshot}
                    >
                      创建第一个快照
                    </Button>
                  </Tooltip>
                }
              />
            ) : (
              <ul className="space-y-1">
                {instance.snapshots.map((snap) => (
                  <li
                    key={snap.id}
                    className="group/snap flex items-center gap-3 rounded px-2 py-2 transition-colors hover:bg-surface-hover/60"
                  >
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-base text-ink">{snap.label}</div>
                      <div className="mt-0.5 text-sm text-ink-faint">
                        {formatDateTime(snap.createdAt)} · DSH{' '}
                        {versions.find((v) => v.id === snap.versionId)?.name} · {snap.pluginCount}{' '}
                        插件 · {formatBytes(snap.size)}
                      </div>
                    </div>
                    <div className="flex shrink-0 gap-1 opacity-0 transition-opacity group-hover/snap:opacity-100">
                      <Tooltip content={running || busy ? '请先停止实例，再回滚' : ''}>
                        <Button
                          size="xs"
                          variant="secondary"
                          disabled={
                            !!snapshotOp || running || busy ||
                            !!deletingSnapshots[`${instance.id}:${snap.id}`]
                          }
                          onClick={async () => {
                            const ok = await confirm({
                              title: `回滚到「${snap.label}」`,
                              message: '当前 dsh-home 里的插件与配置会被快照内容整体替换。',
                              detail: `快照记录：DSH ${snap.versionId} · ${snap.runtimeId} · ${snap.pluginCount} 个插件。回滚不改变实例当前引用的 DSH 版本与 Runtime。`,
                              confirmLabel: '回滚',
                            })
                            if (ok) await restoreSnapshot(instance.id, snap.id)
                          }}
                        >
                          回滚
                        </Button>
                      </Tooltip>
                      <IconButton
                        label="删除快照"
                        size="xs"
                        variant="ghost"
                        disabled={!!deletingSnapshots[`${instance.id}:${snap.id}`] || !!snapshotOp}
                        onClick={async () => {
                          const ok = await confirm({
                            title: '删除快照',
                            message: `删除「${snap.label}」？快照目录会从磁盘移除，不可恢复。`,
                            tone: 'danger',
                            confirmLabel: '删除',
                          })
                          if (ok) await deleteSnapshot(instance.id, snap.id)
                        }}
                      >
                        <Trash2 size={11} />
                      </IconButton>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </SectionCard>
        </motion.div>

        {/* ---- 统计 ---- */}
        <motion.div variants={riseItem}>
          <SectionCard title="统计" collapsible defaultOpen={false}>
            <div className="grid gap-x-8 md:grid-cols-2">
              <div>
                <DataRow label="创建于" value={formatDateTime(instance.createdAt)} />
                <DataRow label="最近运行" value={formatRelative(instance.lastRunAt)} />
              </div>
              <div>
                <DataRow label="累计运行" value={formatDuration(instance.totalRuntime)} />
                <DataRow label="磁盘占用" value={formatBytes(instance.diskUsage)} />
              </div>
            </div>
          </SectionCard>
        </motion.div>

        {/* ---- 危险操作 ---- */}
        <motion.div variants={riseItem}>
          <div className="flex items-center gap-3 rounded-lg bg-danger/[0.04] px-4 py-3 ring-1 ring-inset ring-danger/15">
            <div className="min-w-0 flex-1">
              <div className="text-base font-medium text-ink">删除实例</div>
              <div className="mt-0.5 text-sm text-ink-muted">
                DSH_HOME、插件、Profile 与 workspace 会被一并删除，不可恢复。
              </div>
            </div>
            <Button variant="danger" onClick={actions.remove} disabled={running || busy}>
              <Trash2 size={13} />
              删除
            </Button>
          </div>
        </motion.div>
      </motion.div>
    </PageShell>
  )
}
