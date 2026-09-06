import { useEffect, useState } from 'react'
import { motion } from 'motion/react'
import {
  BookOpen,
  FolderOpen,
  Sparkles,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import {
  chooseDirectory,
  cancelTransfer,
  desktop,
  freeSpace,
  migrationStatus,
  migrationUndo,
  moveRootData,
  type MigrationJournal,
  rootDataSummary,
  type MoveProgress,
} from '@/lib/desktop'
import { formatBytes } from '@/lib/format'
import { useMotion } from '@/lib/motion'
import { repository } from '@/services'
import {
  normalizeRoot,
  useCatalogStore,
  useApiConfigStore,
  useInstanceStore,
  useSettingsStore,
  useUIStore,
  useViewStore,
  type Density,
  type MotionLevel,
  type Theme,
  type UIScale,
} from '@/stores'
import {
  Button,
  Input,
  Notice,
  Segmented,
  SettingRow,
  Switch,
  Select,
} from '@/components/ui'
import { PageShell, PageSection } from '@/components/layout/Page'
import { PanelGroup, PanelItem, PanelShell } from '@/components/layout/Panel'
import { ShortcutsSection } from '@/components/settings/ShortcutsSection'
import { Logo } from '@/components/layout/Logo'

import { AccentPicker, InstanceColourLegend } from '@/features/settings/appearance'
import { MigrationOverlay } from '@/features/settings/MigrationOverlay'
import { DiagnosticsSection } from '@/features/settings/DiagnosticsSection'
import { ROOT_PRESETS, SECTIONS } from '@/features/settings/consts'

export function SettingsPanel() {
  const section = useViewStore((s) => s.settingsSection)
  const setSection = useViewStore((s) => s.setSettingsSection)

  return (
    <PanelShell>
      <PanelGroup title="设置">
        {SECTIONS.map((s) => (
          <PanelItem
            key={s.id}
            groupId="settings-section"
            icon={s.icon}
            label={s.label}
            active={section === s.id}
            onClick={() => setSection(s.id)}
          />
        ))}
      </PanelGroup>
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ */

export function SettingsPage() {
  const section = useViewStore((s) => s.settingsSection)
  const ui = useUIStore()
  const settings = useSettingsStore()
  const instances = useInstanceStore((s) => s.instances)
  const measureDiskUsage = useInstanceStore((s) => s.measureDiskUsage)
  const versions = useCatalogStore((s) => s.versions)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const { t } = useMotion()

  /**
   * Real sizes are only measured when this section is open. Walking every
   * instance tree — `node_modules` included — is far too slow to do on page
   * load, and nothing outside the storage view reads `diskUsage`.
   */
  const [orphans, setOrphans] = useState<{ name: string; size: number }[]>([])
  useEffect(() => {
    if (section !== 'storage') return
    void measureDiskUsage()
    void repository.listOrphanInstanceDirs().then(setOrphans, () => setOrphans([]))
  }, [section, measureDiskUsage])

  const dropOrphan = async (name: string, size: number) => {
    const ok = await ui.confirm({
      title: `删除残留目录 ${name}`,
      message:
        '这个目录里没有 instance.json，因此不属于任何实例——通常是中断的创建，或实例目录尚未真实落盘时期留下的插件文件。删除后无法恢复。',
      detail: `将释放 ${formatBytes(size)}`,
      tone: 'danger',
      confirmLabel: '删除',
    })
    if (!ok) return
    try {
      await repository.removeOrphanInstanceDir(name)
      setOrphans((list) => list.filter((o) => o.name !== name))
      ui.toast({ kind: 'success', title: `已删除 ${name}` })
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: `删除 ${name} 失败`,
        message: err instanceof Error && err.message ? err.message : String(err),
      })
    }
  }

  const [rootDraft, setRootDraft] = useState(settings.root)

  /**
   * Points the app at a new data root and re-reads everything from it.
   *
   * Without the reload the in-memory instance and catalog lists keep
   * describing the old root while every subsequent write resolves against the
   * new one — edits fail with "实例不存在", and a delete reports success while
   * the real directory survives, unreachable, in the old location.
   */
  const switchRootTo = async (next: string) => {
    settings.setRoot(next)
    setRootDraft(next)
    await Promise.all([
      useInstanceStore.getState().reload(),
      useCatalogStore.getState().load(),
      useApiConfigStore.getState().load(),
    ]).catch((err) => {
      ui.toast({
        kind: 'error',
        title: '新目录读取失败',
        message: err instanceof Error && err.message ? err.message : String(err),
      })
    })
  }

  // Keep the field in step when the root changes from somewhere else.
  useEffect(() => setRootDraft(settings.root), [settings.root])

  /** Free bytes per known path; `null` = the drive could not be queried. */
  const [driveSpace, setDriveSpace] = useState<Record<string, number | null>>({})
  useEffect(() => {
    if (section !== 'storage') return
    for (const path of [settings.root, ...ROOT_PRESETS.map((p) => p.path), rootDraft]) {
      if (!path || path in driveSpace) continue
      void freeSpace(path).then((n) => setDriveSpace((m) => ({ ...m, [path]: n })))
    }
    // `driveSpace` is deliberately not a dependency: the guard above makes it
    // a write-once cache, and including it would re-fetch on every answer.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [section, settings.root, rootDraft])

  /**
   * An in-flight root migration. While it exists, the storage view shows a
   * progress overlay; the root itself only flips once the move reports done.
   */
  const [migration, setMigration] = useState<null | { from: string; to: string; transferId: string }>(null)
  const [moveProgress, setMoveProgress] = useState<MoveProgress | null>(null)

  /**
   * A migration cancelled or interrupted by a crash leaves its journal
   * behind; entering the storage view surfaces it as 继续 / 撤销 (O-06),
   * so the half-moved directories are never a silent dead end.
   */
  const [openJournal, setOpenJournal] = useState<MigrationJournal | null>(null)
  const [journalBusy, setJournalBusy] = useState<'resume' | 'undo' | null>(null)
  useEffect(() => {
    if (section !== 'storage') return
    void migrationStatus().then(setOpenJournal, () => setOpenJournal(null))
  }, [section])

  const clearJournal = () => void setOpenJournal(null)

  const undoOpenMigration = async () => {
    if (!openJournal) return
    const ok = await ui.confirm({
      title: '撤销未完成的迁移',
      message: '已复制到新目录的数据将全部搬回旧目录，新目录恢复迁移前的状态。期间不要移动这两个目录。',
      detail: `${openJournal.from}\n  ↑\n${openJournal.to}`,
      tone: 'danger',
      confirmLabel: '撤销迁移',
    })
    if (!ok) return
    setJournalBusy('undo')
    try {
      await migrationUndo()
      ui.toast({ kind: 'success', title: '迁移已撤销', message: '数据已回到旧目录。' })
      clearJournal()
      await useInstanceStore.getState().reload().catch(() => undefined)
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '撤销失败',
        message: err instanceof Error && err.message ? err.message : String(err),
        duration: 8000,
      })
    } finally {
      setJournalBusy(null)
    }
  }

  const beginMigration = async (from: string, to: string) => {
    const transferId = `move-${Date.now()}`
    setMigration({ from, to, transferId })
    setMoveProgress(null)
    try {
      const summary = await moveRootData(from, to, transferId, setMoveProgress)
      if (summary.cancelled) {
        ui.toast({
          kind: 'info',
          title: '迁移已取消',
          message: `已完成 ${formatBytes(summary.bytes)}，数据目录未更改。进度已记录，可在「存储」页继续或撤销。`,
          duration: 6000,
        })
        void migrationStatus().then(setOpenJournal, () => setOpenJournal(null))
        return
      }
      await switchRootTo(to)
      ui.toast({
        kind: 'success',
        title: '数据迁移完成',
        message: `已迁移 ${summary.moved.length} 个目录（${formatBytes(summary.bytes)}），新目录即刻生效。`,
        duration: 6000,
      })
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '迁移失败',
        message:
          err instanceof Error && err.message
            ? `${err.message} 数据目录未更改；已完成的部分保留在新目录，处理后可重新迁移续传。`
            : String(err),
        duration: 8000,
      })
    } finally {
      setMigration(null)
      setMoveProgress(null)
      // The journal is the truth: a cancelled run leaves a resume entry, a
      // committed one deletes itself. Reflect whichever outcome happened.
      void migrationStatus().then(setOpenJournal, () => setOpenJournal(null))
    }
  }

  const pickRoot = async () => {
    const picked = await chooseDirectory(settings.root)
    if (picked) setRootDraft(picked)
    else if (!desktop.isDesktop) {
      ui.toast({
        kind: 'info',
        title: '目录选择仅在桌面端可用',
        message: '在浏览器预览里请直接输入路径。',
        duration: 3000,
      })
    }
  }

  const applyRoot = async () => {
    const next = normalizeRoot(rootDraft)
    if (!next || next === settings.root) return
    const live = useInstanceStore.getState()
    if (Object.values(live.states).some((s) => ['running', 'starting', 'stopping'].includes(s.status))
      || live.hasPendingWrites() || live.createProgress || Object.keys(live.snapshotTransfers).length
      || useApiConfigStore.getState().saving || useApiConfigStore.getState().syncing
      || useCatalogStore.getState().activeTransfers() > 0) {
      ui.toast({
        kind: 'warn',
        title: '暂时无法更改数据目录',
        message: '请先停止所有实例，并等待下载、快照和配置保存完成。',
      })
      return
    }
    const ok = await ui.confirm({
      title: '更改数据目录',
      message: 'PHL 之后会从新目录读写版本、Runtime 与实例。',
      detail: `${settings.root}\n  ↓\n${next}`,
      tone: 'danger',
      confirmLabel: '仍要更改',
    })
    if (!ok) return
    const summary = await rootDataSummary(settings.root).catch(() => null)
    if (summary?.hasData) {
      const parts = [
        summary.instances.entries && `${summary.instances.entries} 个实例`,
        summary.versions.entries && `${summary.versions.entries} 个版本`,
        summary.runtimes.entries && `${summary.runtimes.entries} 个 Runtime`,
        summary.config.entries && 'API 配置库',
      ].filter(Boolean)
      const move = await ui.confirm({
        title: '立即迁移现有数据？',
        message: `旧目录中仍有 ${parts.join('、') || '数据'}。迁移会把它们连同下载缓存完整搬到新目录；不迁移的话它们将留在原处，且不再出现在 PHL 的列表里。`,
        confirmLabel: '立即迁移',
        cancelLabel: '以后再说',
      })
      if (move) {
        void beginMigration(settings.root, next)
        return
      }
    }
    await switchRootTo(next)
    ui.toast({
      kind: 'info',
      title: '数据目录已更新',
      message: '旧目录的数据仍在原处，之后可在本页重新迁移。',
    })
  }

  const instanceDisk = instances.reduce((sum, i) => sum + i.diskUsage, 0)
  const versionDisk = versions
    .filter((v) => v.state.kind === 'installed')
    .reduce((sum, v) => sum + v.size, 0)
  const runtimeDisk = runtimes
    .filter((r) => r.state.kind === 'installed' && !r.system)
    .reduce((sum, r) => sum + r.size, 0)
  const total = instanceDisk + versionDisk + runtimeDisk

  const bars = [
    { label: '实例', value: instanceDisk, className: 'bg-accent' },
    { label: 'DSH 版本', value: versionDisk, className: 'bg-info' },
    { label: 'Runtime', value: runtimeDisk, className: 'bg-ok' },
  ]

  const title = SECTIONS.find((s) => s.id === section)?.label ?? '设置'

  return (
    <div className="relative h-full">
      <PageShell maxWidth={720} title={title}>
      <motion.div
        key={section}
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={t(0.24)}
      >
        {section === 'general' && (
          <PageSection>
            <div className="rounded-lg bg-surface px-4 ring-1 ring-inset ring-line">
              <SettingRow
                title="启动 PHL 时"
                description="计划中的能力，尚未接入启动流程 —— 当前设置不会生效"
                control={
                  <Select
                    disabled
                    value={settings.startup}
                    onChange={(e) => settings.set('startup', e.target.value as never)}
                    className="w-[168px]"
                  >
                    <option value="none">不做任何事</option>
                    <option value="last">恢复上次运行的实例</option>
                    <option value="favorites">启动所有置顶实例</option>
                  </Select>
                }
              />
              <SettingRow
                title="关闭窗口时最小化到托盘"
                description="托盘驻留尚未实现；关闭窗口即退出（实例可继续运行，见下）"
                control={
                  <Switch
                    disabled
                    checked={settings.minimizeToTray}
                    onChange={(v) => settings.set('minimizeToTray', v)}
                  />
                }
              />
              <SettingRow
                title="退出时停止所有实例"
                description="避免留下孤儿 DSH 进程"
                control={
                  <Switch
                    checked={settings.closeStopsInstances}
                    onChange={(v) => settings.set('closeStopsInstances', v)}
                  />
                }
              />
              <SettingRow
                title="自动检查 DSH 新版本"
                description="独立的应用更新检查尚未接入；版本列表的定时刷新在「下载」分区配置"
                control={
                  <Switch
                    disabled
                    checked={settings.checkUpdates}
                    onChange={(v) => settings.set('checkUpdates', v)}
                  />
                }
              />
              <SettingRow
                title="删除实例前二次确认"
                description="要求输入实例名称才能删除"
                control={
                  <Switch
                    checked={ui.confirmDelete}
                    onChange={(v) => ui.setPref('confirmDelete', v)}
                  />
                }
              />
            </div>
          </PageSection>
        )}

        {section === 'downloads' && (
          <PageSection>
            <div className="rounded-lg bg-surface px-4 ring-1 ring-inset ring-line">
              <SettingRow
                title="下载源"
                description="用于获取 DSH 发行版与 Node Runtime"
                control={
                  <Select
                    value={settings.source}
                    onChange={(e) => settings.set('source', e.target.value as never)}
                    className="w-[168px]"
                  >
                    <option value="official">官方源</option>
                    <option value="mirror-cn">国内镜像</option>
                    <option value="custom">自定义</option>
                  </Select>
                }
              />
              {settings.source === 'custom' && (
                <SettingRow
                  title="自定义地址"
                  control={
                    <Input
                      value={settings.customSource}
                      onChange={(e) => settings.set('customSource', e.target.value)}
                      placeholder="https://"
                      className="w-[280px]"
                    />
                  }
                />
              )}
              <SettingRow
                title="同时下载数"
                description="版本、Runtime 与插件安装共享的传输槽位上限；超出的任务排队为「排队中」，前一个完成或取消后自动开始"
                control={
                  <Segmented
                    size="sm"
                    value={String(settings.concurrency)}
                    onChange={(v) => settings.set('concurrency', Number(v))}
                    options={[
                      { value: '1', label: '1' },
                      { value: '2', label: '2' },
                      { value: '4', label: '4' },
                    ]}
                  />
                }
              />
              <SettingRow
                title="保留安装包"
                description="重装同一版本时无需重新下载，但会占用额外空间"
                control={
                  <Switch
                    checked={settings.keepArchives}
                    onChange={(v) => settings.set('keepArchives', v)}
                  />
                }
              />
              <SettingRow
                title="自动同步版本目录"
                description="仅拉取版本列表，不自动下载或安装；窗口隐藏时暂停，不占后台资源"
                control={
                  <Select
                    value={String(settings.versionRefreshMinutes)}
                    onChange={(e) => settings.set('versionRefreshMinutes', Number(e.target.value))}
                    className="w-[168px]"
                  >
                    <option value="0">关闭</option>
                    <option value="15">每 15 分钟</option>
                    <option value="30">每 30 分钟</option>
                    <option value="60">每 1 小时</option>
                  </Select>
                }
              />
              <SettingRow
                title="GitHub 版本上架 npm 时提醒"
                description="GitHub 已发布、npm 还没收录的版本，一旦上架立即提醒；跟随版本同步检查，无额外开销"
                control={
                  <Switch
                    checked={settings.pendingReleaseAlerts}
                    onChange={(v) => settings.set('pendingReleaseAlerts', v)}
                  />
                }
              />
            </div>
          </PageSection>
        )}

        {section === 'appearance' && (
          <>
            <PageSection title="主题">
              <div className="rounded-lg bg-surface px-4 ring-1 ring-inset ring-line">
                <SettingRow
                  title="配色模式"
                  control={
                    <Segmented
                      value={ui.theme}
                      onChange={(v) => ui.setTheme(v as Theme)}
                      options={[
                        { value: 'light', label: '浅色' },
                        { value: 'dark', label: '深色' },
                        { value: 'system', label: '跟随系统' },
                      ]}
                    />
                  }
                />
                <SettingRow
                  title="强调色"
                  description="用于主按钮、选中态与进度"
                  control={<AccentPicker />}
                />
                <SettingRow
                  title="界面密度"
                  control={
                    <Segmented
                      value={ui.density}
                      onChange={(v) => ui.setDensity(v as Density)}
                      options={[
                        { value: 'compact', label: '紧凑' },
                        { value: 'default', label: '标准' },
                        { value: 'comfortable', label: '宽松' },
                      ]}
                    />
                  }
                />
                <SettingRow
                  title="界面缩放"
                  description="整体放大或缩小，包括字号与间距"
                  control={
                    <Segmented
                      value={String(ui.scale)}
                      onChange={(v) => ui.setScale(Number(v) as UIScale)}
                      options={[
                        { value: '85', label: '85%' },
                        { value: '90', label: '90%' },
                        { value: '100', label: '100%' },
                        { value: '110', label: '110%' },
                      ]}
                    />
                  }
                />
              </div>
            </PageSection>

            <PageSection title="动效" description="降低强度会缩短所有过渡时长，关闭则只保留必要的状态变化。">
              <div className="rounded-lg bg-surface px-4 ring-1 ring-inset ring-line">
                <SettingRow
                  title="动画强度"
                  control={
                    <Segmented
                      value={ui.motion}
                      onChange={(v) => ui.setMotion(v as MotionLevel)}
                      options={[
                        { value: 'full', label: '完整' },
                        { value: 'reduced', label: '精简' },
                        { value: 'off', label: '关闭' },
                      ]}
                    />
                  }
                />
                <SettingRow
                  title="显示底部启动栏"
                  description="在实例页固定显示当前启动目标"
                  control={
                    <Switch
                      checked={ui.showLaunchDock}
                      onChange={(v) => ui.setPref('showLaunchDock', v)}
                    />
                  }
                />
                <SettingRow
                  title="默认实例视图"
                  control={
                    <Segmented
                      size="sm"
                      value={ui.layout}
                      onChange={(v) => ui.setLayout(v as 'grid' | 'list')}
                      options={[
                        { value: 'grid', label: '卡片' },
                        { value: 'list', label: '列表' },
                      ]}
                    />
                  }
                />
              </div>
            </PageSection>

            <PageSection title="实例标识色" description="每个实例有固定的标识色，便于在列表与启动栏中辨认。">
              <div className="rounded-lg bg-surface p-4 ring-1 ring-inset ring-line">
                <InstanceColourLegend />
              </div>
            </PageSection>
          </>
        )}

        {section === 'shortcuts' && <ShortcutsSection />}

        {section === 'storage' && (
          <>
            {openJournal && !migration && (
              <PageSection title="未完成的迁移">
                <Notice tone="warn" title={`旧目录 → 新目录的数据搬运尚未结束`}>
                  <div className="mt-1 break-all text-sm">
                    {openJournal.from} → {openJournal.to}（{openJournal.entries.filter((e) => e.state === 'moved').length}/{openJournal.entries.length} 个目录已到达，
                    {formatBytes(openJournal.entries.reduce((sum, e) => sum + (e.state === 'moved' ? e.bytes : 0), 0))}）。
                    可从中断处继续，或将已复制的数据原路退回。
                  </div>
                  <div className="mt-2.5 flex gap-2">
                    <Button
                      size="sm"
                      variant="primary"
                      onClick={() => void beginMigration(openJournal.from, openJournal.to)}
                    >
                      继续迁移
                    </Button>
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={journalBusy !== null}
                      onClick={() => void undoOpenMigration()}
                    >
                      {journalBusy === 'undo' ? '撤销中…' : '撤销并还原'}
                    </Button>
                  </div>
                </Notice>
              </PageSection>
            )}
            <PageSection
              title="数据目录"
              description="版本、Runtime 与所有实例都存放在这里。这不是 PHL 程序本身的安装位置。"
            >
              <div className="rounded-lg bg-surface p-3 ring-1 ring-inset ring-line">
                <div className="mb-2.5 flex flex-wrap items-center gap-x-4 gap-y-1 text-sm">
                  <span className="text-ink-muted">
                    已占用 <span className="num font-medium text-ink">{formatBytes(total)}</span>
                  </span>
                  <span className="text-ink-muted">
                    所在盘剩余{' '}
                    <span className="num font-medium text-ink">
                      {driveSpace[settings.root] === undefined
                        ? '…'
                        : driveSpace[settings.root] === null
                          ? '未知'
                          : formatBytes(driveSpace[settings.root] as number)}
                    </span>
                  </span>
                </div>

                <div className="flex items-center gap-2">
                  <Input
                    value={rootDraft}
                    onChange={(e) => setRootDraft(e.target.value)}
                    spellCheck={false}
                    className="flex-1 font-mono text-sm"
                  />
                  <Button variant="secondary" onClick={pickRoot}>
                    <FolderOpen size={12} />
                    浏览
                  </Button>
                  <Button
                    variant="primary"
                    disabled={normalizeRoot(rootDraft) === settings.root || !rootDraft.trim()}
                    onClick={applyRoot}
                  >
                    应用
                  </Button>
                </div>

                <div className="mt-2 flex flex-wrap items-center gap-1.5">
                  <span className="text-sm text-ink-faint">常用位置</span>
                  {ROOT_PRESETS.map((preset) => {
                    const free = driveSpace[preset.path]
                    return (
                      <button
                        key={preset.path}
                        onClick={() => setRootDraft(preset.path)}
                        className="rounded-xs bg-surface-sunken px-1.5 py-0.5 font-mono text-2xs text-ink-muted ring-1 ring-inset ring-line transition-colors hover:bg-surface-hover hover:text-ink"
                      >
                        {preset.label}
                        {free != null && <span className="num text-ink-faint"> · {formatBytes(free)}</span>}
                      </button>
                    )
                  })}
                </div>

                {normalizeRoot(rootDraft) !== settings.root &&
                  driveSpace[normalizeRoot(rootDraft)] != null && (
                    <p className="mt-2 text-sm text-ink-faint">
                      新位置所在盘剩余{' '}
                      <span className="num">{formatBytes(driveSpace[normalizeRoot(rootDraft)] as number)}</span>
                      ，应用时可选择把现有数据一并迁移过去。
                    </p>
                  )}

                <p className="mt-2.5 text-sm leading-relaxed text-ink-faint">
                  一个 DSH 版本约 50 MB，一个 Node Runtime 约 30 MB，而带有独立 node_modules
                  的实例可能占用数 GB。如果系统盘空间紧张，建议把这里改到其他分区。
                </p>
              </div>
            </PageSection>

            <PageSection title="占用分布" description={`共 ${formatBytes(total)}`}>
              <div className="rounded-lg bg-surface p-4 ring-1 ring-inset ring-line">
                <div className="flex h-2 overflow-hidden rounded-full bg-surface-sunken">
                  {bars.map((b) => (
                    <motion.div
                      key={b.label}
                      className={cn('h-full', b.className)}
                      initial={{ width: 0 }}
                      animate={{ width: `${total ? (b.value / total) * 100 : 0}%` }}
                      transition={t(0.5)}
                    />
                  ))}
                </div>
                <ul className="mt-3 space-y-1.5">
                  {bars.map((b) => (
                    <li key={b.label} className="flex items-center gap-2 text-base">
                      <span className={cn('h-2 w-2 rounded-full', b.className)} />
                      <span className="text-ink-muted">{b.label}</span>
                      <span className="num ml-auto text-ink">{formatBytes(b.value)}</span>
                    </li>
                  ))}
                </ul>
              </div>
            </PageSection>

            <PageSection title="各实例占用">
              <ul className="space-y-1.5">
                {[...instances]
                  .sort((a, b) => b.diskUsage - a.diskUsage)
                  .map((i) => (
                    <li
                      key={i.id}
                      className="flex items-center gap-3 rounded-lg bg-surface px-3.5 py-2.5 ring-1 ring-inset ring-line"
                    >
                      <span className="min-w-0 flex-1 truncate text-base text-ink">{i.name}</span>
                      <div className="h-1.5 w-32 overflow-hidden rounded-full bg-surface-sunken">
                        <motion.div
                          className="h-full rounded-full bg-accent"
                          initial={{ width: 0 }}
                          animate={{
                            width: `${instanceDisk ? (i.diskUsage / instanceDisk) * 100 : 0}%`,
                          }}
                          transition={t(0.5)}
                        />
                      </div>
                      <span className="num w-16 shrink-0 text-right text-sm text-ink-muted">
                        {formatBytes(i.diskUsage)}
                      </span>
                    </li>
                  ))}
              </ul>
            </PageSection>

            {orphans.length > 0 && (
              <PageSection
                title="残留目录"
                description="instances/ 下没有 instance.json 的目录，不属于任何实例"
              >
                <ul className="space-y-1.5">
                  {orphans.map((o) => (
                    <li
                      key={o.name}
                      className="group/row flex items-center gap-3 rounded-lg bg-surface px-3.5 py-2.5 ring-1 ring-inset ring-warn/25"
                    >
                      <span className="min-w-0 flex-1 truncate font-mono text-sm text-ink">
                        {o.name}
                      </span>
                      <span className="num shrink-0 text-sm text-ink-muted">
                        {formatBytes(o.size)}
                      </span>
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => void dropOrphan(o.name, o.size)}
                        className="opacity-0 transition-opacity group-hover/row:opacity-100 focus:opacity-100"
                      >
                        删除
                      </Button>
                    </li>
                  ))}
                </ul>
              </PageSection>
            )}
          </>
        )}

        {section === 'diagnostics' && <DiagnosticsSection />}

        {section === 'advanced' && (
          <>
            <PageSection>
              <div className="rounded-lg bg-surface px-4 ring-1 ring-inset ring-line">
                <SettingRow
                  title="端口分配起点"
                  description="自动分配时从这个端口开始向上查找空闲端口"
                  control={
                    <Input
                      type="number"
                      value={settings.portStart}
                      onChange={(e) => settings.set('portStart', Number(e.target.value))}
                      className="w-[120px]"
                    />
                  }
                />
                <SettingRow
                  title="日志级别"
                  description="日志分级尚未接入后端；诊断与实例日志始终完整记录"
                  control={
                    <Select
                      disabled
                      value={settings.logLevel}
                      onChange={(e) => settings.set('logLevel', e.target.value as never)}
                      className="w-[130px]"
                    >
                      {['error', 'warn', 'info', 'debug', 'trace'].map((l) => (
                        <option key={l} value={l}>
                          {l}
                        </option>
                      ))}
                    </Select>
                  }
                />
              </div>
            </PageSection>

            <PageSection>
              <Notice tone="warn" title="重置设置">
                恢复所有偏好设置的默认值。实例、版本与插件不会受影响。
                <div className="mt-2.5">
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={async () => {
                      const ok = await ui.confirm({
                        title: '重置所有设置',
                        message: '外观、下载与高级选项都会恢复默认值。',
                        confirmLabel: '重置',
                      })
                      if (ok) {
                        settings.reset()
                        ui.toast({ kind: 'success', title: '设置已恢复默认值' })
                      }
                    }}
                  >
                    重置
                  </Button>
                </div>
              </Notice>
            </PageSection>
          </>
        )}

        {section === 'about' && (
          <>
            <PageSection>
              <div className="flex items-center gap-4 rounded-lg bg-surface p-5 ring-1 ring-inset ring-line">
                <Logo size={44} />
                <div className="min-w-0">
                  <div className="text-lg font-semibold tracking-tight text-ink">PHL</div>
                  <div className="mt-0.5 text-base text-ink-muted">
                    DSH Instance &amp; Runtime Manager
                  </div>
                  <div className="num mt-1.5 flex items-center gap-2 text-sm text-ink-faint">
                    <span>0.1.0</span>
                    <span className="text-ink-faint/50">·</span>
                    <span>核心链路已真实接入（含启动）</span>
                  </div>
                </div>
              </div>
            </PageSection>

            <PageSection title="当前阶段">
              <div className="rounded-lg bg-surface p-3 text-sm leading-relaxed text-ink-muted ring-1 ring-inset ring-line">
                <p>
                  桌面端里，版本、插件、实例、Runtime 与启动进程都已是真实实现：下载落盘并校验完整性，
                  实例是磁盘上的目录，启动会以实例自己的 DSH_HOME 拉起真实的 DSH 进程并探测端口就绪。
                </p>
                <p className="mt-2">
                  浏览器模式（<code className="font-mono text-sm">npm run dev</code>）下一切数据来自
                  Mock Repository，仅用于纯 UI 开发。
                </p>
                <div className="mt-3">
                  <Button
                    variant="secondary"
                    onClick={() => {
                      ui.setGuideOpen(true)
                      ui.navigate({ name: 'instances' })
                    }}
                  >
                    <BookOpen size={12} />
                    重新查看入门指引
                  </Button>
                </div>
              </div>
            </PageSection>

            <PageSection title="参考与边界">
              <div className="rounded-lg bg-surface p-4 text-base leading-relaxed text-ink-muted ring-1 ring-inset ring-line">
                <p className="flex items-start gap-2">
                  <Sparkles size={14} className="mt-1 shrink-0 text-accent" />
                  <span>
                    PHL 在交互质量上参考了 PCL、Prism Launcher 等成熟启动器所验证过的产品模式与体验节奏，
                    但代码、组件、视觉细节与资源均为独立实现，未复制或移植任何上述项目的源码与素材。
                  </span>
                </p>
              </div>
            </PageSection>
          </>
        )}
      </motion.div>
      </PageShell>

      <MigrationOverlay
        info={migration}
        progress={moveProgress}
        onCancel={() => migration && void cancelTransfer(migration.transferId)}
      />
    </div>
  )
}