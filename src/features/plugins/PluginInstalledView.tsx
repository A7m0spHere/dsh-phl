import { useState } from 'react'
import { motion } from 'motion/react'
import { AlertTriangle, ArrowUpCircle, Blocks, Link2, Package2, RefreshCw, Trash2, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'
import { useUIStore, type PluginTransferState } from '@/stores'
import type { InstalledPlugin, Plugin } from '@/types'
import type { LatestVersionState } from '@/stores/catalogStore'
import { Badge, Button, EmptyState, Notice, Switch, Tooltip } from '@/components/ui'
import { STAGE_LABEL, SourceBadge, TransferInline } from './visuals'

/** The row shape the parent computes for an installed plugin. */
export type InstalledPluginRow = InstalledPlugin & {
  meta?: Plugin
  name: string
  newest?: string
  outdated: boolean
  check?: LatestVersionState
  compat: 'ok' | 'bad' | 'unknown'
}

const TRUST_LABEL: Record<string, string> = {
  verified: '已验证',
  pinned: '已固定',
  unverified: '未验证',
  unknown: '信任未知',
}

const TRUST_HINT: Record<string, string> = {
  verified: 'npm 固定版本，安装时已通过 registry sha512 校验',
  pinned: '内容固定（校验和或 commit SHA），重装可复现',
  unverified: '来源未固定（如 GitHub HEAD），内容可能随时变化，请确认来源可信',
  unknown: '安装记录缺少信任信息（旧版本 PHL 安装）',
}

/**
 * 未固定来源的插件在更新前必须再次确认：HEAD 归档的内容自上次安装后可能
 * 已经被上游替换，这次“更新”实际上是一次不可审查的换血 (T-108)。
 */
async function updateWithTrustWarning(
  plugin: { name: string; trust?: string },
  run: () => Promise<void>,
): Promise<void> {
  if (plugin.trust === 'unverified' || plugin.trust === 'unknown') {
    const ok = await useUIStore.getState().confirm({
      title: '更新未验证来源的插件？',
      message: `「${plugin.name}」的安装来源未固定（${plugin.trust === 'unknown' ? '信任未知' : '如 GitHub HEAD'}），更新会以当前远端内容替换现有文件，且无法提前校验内容是否被改动。`,
      confirmLabel: '仍然更新',
      cancelLabel: '取消',
    })
    if (!ok) return
  }
  await run()
}

export interface PluginInstalledViewProps {
  instance: { id: string; name: string }
  tab: 'installed' | 'updates'
  rows: InstalledPluginRow[]
  upgradableCount: number
  checkCounts: { checking: number; failed: number; unresolvable: number }
  installStateFor: (pluginId: string) => PluginTransferState['stage'] | undefined
  setEnabled: (instanceId: string, pluginId: string, enabled: boolean) => void
  uninstall: (instanceId: string, pluginId: string) => void
  install: (instanceId: string, pluginId: string) => Promise<void>
  onGoUpdates: () => void
  onGoRegistry: () => void
  onRecheck: () => void
}

/**
 * The installed + updates lists (§M4): the cross-tab update notice, the
 * updates re-check toolbar, each installed-plugin row (enable/disable switch,
 * update with the trust confirmation, uninstall confirm), and the empty state
 * that distinguishes "checking / failed" from a real "all up to date" (§R5).
 * `confirmUninstall` is this view's own UI state.
 */
export function PluginInstalledView(p: PluginInstalledViewProps) {
  const { stagger, riseItem } = useMotion()
  const [confirmUninstall, setConfirmUninstall] = useState<string | null>(null)
  const { instance, rows, checkCounts, tab } = p
  const { id: instanceId } = instance

  return (
    <>
      {tab === 'installed' && p.upgradableCount > 0 && (
        <Notice
          tone="info"
          title={`${p.upgradableCount} 个插件有新版本`}
          className="mb-4"
          action={
            <Button size="sm" variant="secondary" onClick={p.onGoUpdates}>
              查看
            </Button>
          }
        >
          更新只作用于当前实例，其他实例仍保持原有版本。
        </Notice>
      )}

      {tab === 'updates' && (
        <div className="mb-3 flex items-center justify-between gap-2">
          <span className="text-sm text-ink-faint">
            {checkCounts.checking > 0
              ? `正在检查 ${checkCounts.checking} 个插件…`
              : checkCounts.failed > 0
                ? `${checkCounts.failed} 个插件检查失败，可重新检查`
                : checkCounts.unresolvable > 0
                  ? `${checkCounts.unresolvable} 个插件的来源无法检查更新`
                  : '更新只作用于当前实例的插件。'}
          </span>
          <Button
            size="sm"
            variant="secondary"
            disabled={checkCounts.checking > 0}
            onClick={p.onRecheck}
          >
            <RefreshCw size={13} className={checkCounts.checking > 0 ? 'animate-spin' : undefined} />
            {checkCounts.checking > 0 ? '检查中…' : '重新检查'}
          </Button>
        </div>
      )}

      {rows.length === 0 ? (
        <EmptyState
          icon={<Blocks size={20} />}
          title={
            tab === 'updates'
              ? checkCounts.checking > 0
                ? '正在检查更新…'
                : checkCounts.failed > 0
                  ? '部分插件未能完成检查'
                  : '所有插件都是最新的'
              : '这个实例还没有插件'
          }
          description={
            tab === 'updates'
              ? checkCounts.checking > 0
                ? '检查完成后，可更新的插件会显示在这里。'
                : checkCounts.failed > 0
                  ? '网络或插件来源可能临时不可用，点上方“重新检查”重试。'
                  : `「${instance.name}」中的插件都已经是最新版本。`
              : '从插件库里挑一个装进这个实例，它不会影响其他实例。'
          }
          action={
            <Button variant="secondary" onClick={p.onGoRegistry}>
              浏览插件库
            </Button>
          }
        />
      ) : (
        <motion.ul variants={stagger(0.03)} initial="hidden" animate="show" className="space-y-1.5">
          {rows.map((row) => {
            const stage = p.installStateFor(row.pluginId)
            return (
              <motion.li
                key={row.pluginId}
                variants={riseItem}
                className="group/row rounded-lg bg-surface px-3.5 py-2.5 ring-1 ring-inset ring-line transition-shadow duration-200 hover:ring-line-strong/70"
              >
                <div className="flex items-center gap-3">
                  <span
                    className={cn(
                      'flex h-7 w-7 shrink-0 items-center justify-center rounded-lg',
                      row.enabled ? 'bg-ok/10 text-ok' : 'bg-surface-sunken text-ink-faint',
                    )}
                  >
                    {row.linked ? <Link2 size={13} /> : <Package2 size={13} />}
                  </span>

                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className={cn('text-base', row.enabled ? 'text-ink' : 'text-ink-faint')}>
                        {row.name}
                      </span>
                      {row.linked && <Badge tone="accent">本地链接</Badge>}
                      {row.meta && <SourceBadge plugin={row.meta} />}
                      {row.trust && row.trust !== 'verified' && (
                        <Tooltip content={TRUST_HINT[row.trust]}>
                          <Badge tone={row.trust === 'pinned' ? 'accent' : 'warn'}>
                            {TRUST_LABEL[row.trust]}
                          </Badge>
                        </Tooltip>
                      )}
                      {row.compat === 'bad' && <Badge tone="danger">不兼容当前版本</Badge>}
                      {row.compat === 'unknown' && !row.linked && <Badge tone="warn">兼容性未知</Badge>}
                    </div>
                    <div className="mt-0.5 flex items-center gap-1.5 text-sm text-ink-faint">
                      <span className="font-mono">{row.version}</span>
                      {row.outdated && (
                        <>
                          <span className="text-ink-faint/50">→</span>
                          <span className="font-mono text-accent-ink">{row.newest}</span>
                        </>
                      )}
                      {/* Distinct non-"up-to-date" states (§R5). */}
                      {row.check?.status === 'checking' && (
                        <span className="inline-flex items-center gap-1 text-ink-faint">
                          <RefreshCw size={11} className="animate-spin" />
                          检查中…
                        </span>
                      )}
                      {row.check?.status === 'error' && (
                        <Tooltip content={row.check.message}>
                          <span className="inline-flex items-center gap-1 text-warn">
                            <AlertTriangle size={11} />
                            检查失败 · 可重新检查
                          </span>
                        </Tooltip>
                      )}
                      {row.check?.status === 'unresolvable' && (
                        <span className="text-ink-faint">该来源无法检查更新</span>
                      )}
                      {row.meta && (
                        <>
                          <span className="text-ink-faint/50">·</span>
                          <span>{row.meta.author}</span>
                        </>
                      )}
                    </div>
                  </div>

                  <div className="flex shrink-0 items-center gap-1.5">
                    {stage ? (
                      <Badge tone="warn">{STAGE_LABEL[stage] ?? '安装中'}</Badge>
                    ) : (
                      row.outdated &&
                      row.newest && (
                        <Button
                          size="sm"
                          variant="primary"
                          onClick={() =>
                            void updateWithTrustWarning(row, () => p.install(instanceId, row.pluginId))
                          }
                        >
                          <ArrowUpCircle size={12} />
                          更新
                        </Button>
                      )
                    )}
                    {!stage && (
                      <Tooltip content={row.enabled ? '停用' : '启用'}>
                        <Switch
                          checked={row.enabled}
                          onChange={(v) => void p.setEnabled(instanceId, row.pluginId, v)}
                          label={`启用 ${row.name}`}
                        />
                      </Tooltip>
                    )}
                    {!stage && !row.linked && confirmUninstall !== row.pluginId && (
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => setConfirmUninstall(row.pluginId)}
                        className="opacity-0 transition-opacity group-hover/row:opacity-100 focus:opacity-100"
                      >
                        <Trash2 size={12} />
                      </Button>
                    )}
                    {confirmUninstall === row.pluginId && (
                      <div className="flex items-center gap-1.5">
                        <Button
                          size="sm"
                          variant="danger"
                          onClick={() => {
                            setConfirmUninstall(null)
                            void p.uninstall(instanceId, row.pluginId)
                          }}
                        >
                          确认卸载
                        </Button>
                        <Button size="sm" variant="ghost" onClick={() => setConfirmUninstall(null)}>
                          <X size={12} />
                        </Button>
                      </div>
                    )}
                  </div>
                </div>
                <TransferInline instanceId={instanceId} pluginId={row.pluginId} />
              </motion.li>
            )
          })}
        </motion.ul>
      )}
    </>
  )
}
