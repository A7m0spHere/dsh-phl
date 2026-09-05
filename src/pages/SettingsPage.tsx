import { useEffect, useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  BookOpen,
  Download,
  FolderOpen,
  HardDrive,
  Info,
  Keyboard,
  Palette,
  Settings2,
  SlidersHorizontal,
  Sparkles,
  Stethoscope,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import {
  chooseDirectory,
  cancelTransfer,
  desktop,
  freeSpace,
  moveRootData,
  rootDataSummary,
  runDiagnostics,
  clearDownloadCache,
  type DiagnosticReport,
  type MoveProgress,
} from '@/lib/desktop'
import { formatBytes, formatDateTime } from '@/lib/format'
import { hueTone } from '@/lib/hue'
import { useMotion } from '@/lib/motion'
import { repository } from '@/services'
import {
  ACCENTS,
  normalizeRoot,
  useCatalogStore,
  useApiConfigStore,
  useInstanceStore,
  useIsDark,
  useSettingsStore,
  useUIStore,
  useViewStore,
  type Accent,
  type Density,
  type MotionLevel,
  type SettingsSection,
  type Theme,
  type UIScale,
} from '@/stores'
import {
  Button,
  Input,
  Notice,
  ProgressBar,
  Segmented,
  SettingRow,
  Spinner,
  Switch,
  Select,
} from '@/components/ui'
import { PageShell, PageSection } from '@/components/layout/Page'
import { PanelGroup, PanelItem, PanelShell } from '@/components/layout/Panel'
import { ShortcutsSection } from '@/components/settings/ShortcutsSection'
import { Logo } from '@/components/layout/Logo'

const SECTIONS: { id: SettingsSection; label: string; icon: ReactNode }[] = [
  { id: 'general', label: '通用', icon: <Settings2 size={13} /> },
  { id: 'downloads', label: '下载', icon: <Download size={13} /> },
  { id: 'appearance', label: '外观', icon: <Palette size={13} /> },
  { id: 'shortcuts', label: '快捷键', icon: <Keyboard size={13} /> },
  { id: 'storage', label: '存储', icon: <HardDrive size={13} /> },
  { id: 'diagnostics', label: '诊断', icon: <Stethoscope size={13} /> },
  { id: 'advanced', label: '高级', icon: <SlidersHorizontal size={13} /> },
  { id: 'about', label: '关于', icon: <Info size={13} /> },
]

/**
 * Common places to park a multi-gigabyte data directory.
 *
 * No "user directory" preset: the only correct value for it is whatever
 * `defaultRoot()` resolves on this machine, and the hardcoded placeholder
 * that used to sit here pointed the app at a nonexistent user's AppData once
 * the root started driving real file operations.
 */
const ROOT_PRESETS = [
  { label: 'D:\\PHL', path: 'D:\\PHL' },
  { label: 'E:\\PHL', path: 'E:\\PHL' },
]

const MOVE_KIND_LABELS: Record<string, string> = {
  instances: '实例目录',
  versions: 'DSH 版本',
  runtimes: 'Node Runtime',
  cache: '下载缓存',
}

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

function AccentPicker() {
  const accent = useUIStore((s) => s.accent)
  const setAccent = useUIStore((s) => s.setAccent)
  const dark = useIsDark()
  const { spring } = useMotion()

  return (
    <div className="flex gap-1.5">
      {ACCENTS.map((a) => (
        <button
          key={a.id}
          title={a.label}
          onClick={() => setAccent(a.id as Accent)}
          className="relative flex h-7 w-7 items-center justify-center rounded-lg transition-transform duration-150 hover:scale-105"
        >
          {/* the swatch re-declares the accent tokens locally, so each chip
              paints itself in the palette it would apply */}
          <span
            className={cn('h-5 w-5 rounded-md', dark && 'dark')}
            data-accent={a.id}
            style={{ background: 'hsl(var(--c-accent))' }}
          />
          {accent === a.id && (
            <motion.span
              layoutId="accent-ring"
              transition={spring}
              className="absolute inset-0 rounded-lg ring-2 ring-accent ring-offset-1 ring-offset-canvas"
            />
          )}
        </button>
      ))}
    </div>
  )
}

function InstanceColourLegend() {
  const instances = useInstanceStore((s) => s.instances)
  const dark = useIsDark()
  return (
    <div className="flex flex-wrap gap-1.5">
      {instances.slice(0, 6).map((i) => {
        const tone = hueTone(i.hue, dark)
        return (
          <span
            key={i.id}
            className="inline-flex items-center gap-1.5 rounded-full px-2 py-1 text-2xs"
            style={{ background: tone.soft, color: tone.text }}
          >
            <span className="h-[6px] w-[6px] rounded-full" style={{ background: tone.solid }} />
            {i.name}
          </span>
        )
      })}
    </div>
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

  /**
   * 诊断报告只在进入本分区时（或用户点了重新检查）生成：全部是廉价的存在性
   * 探测，但没必要在打开设置页时就跑。
   */
  const [report, setReport] = useState<DiagnosticReport | null>(null)
  const [diagLoading, setDiagLoading] = useState(false)
  const [cacheBusy, setCacheBusy] = useState(false)

  useEffect(() => {
    if (section !== 'diagnostics' || !desktop.isDesktop) return
    setDiagLoading(true)
    void runDiagnostics()
      .then(setReport, () => setReport(null))
      .finally(() => setDiagLoading(false))
  }, [section, settings.root])

  const rerunDiagnostics = () => {
    if (!desktop.isDesktop || diagLoading) return
    setDiagLoading(true)
    void runDiagnostics()
      .then(setReport, (err) => {
        setReport(null)
        ui.toast({
          kind: 'error',
          title: '诊断失败',
          message: err instanceof Error ? err.message : String(err),
        })
      })
      .finally(() => setDiagLoading(false))
  }

  const clearCache = async () => {
    if (!report) return
    const ok = await ui.confirm({
      title: '清理下载缓存',
      message: '删除下载残留（.part）与保留的压缩包。它们只用于加速重装，不影响任何已安装的版本、Runtime 与实例。',
      detail: `将释放 ${formatBytes(report.cacheBytes)}`,
      tone: 'danger',
      confirmLabel: '清理',
    })
    if (!ok) return
    setCacheBusy(true)
    try {
      const freed = await clearDownloadCache()
      ui.toast({ kind: 'success', title: '缓存已清理', message: `释放 ${formatBytes(freed)}` })
      rerunDiagnostics()
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '清理缓存失败',
        message: err instanceof Error ? err.message : String(err),
      })
    } finally {
      setCacheBusy(false)
    }
  }

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
          message: `已完成 ${formatBytes(summary.bytes)}，数据目录未更改。已搬走的部分保留在新目录，重新迁移会自动续传剩余部分。`,
          duration: 6000,
        })
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
                description="打开应用后自动执行的操作"
                control={
                  <Select
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
                description="保持实例继续运行"
                control={
                  <Switch
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
                description="仅提示，不会自动安装或替换已有版本"
                control={
                  <Switch
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

        {section === 'diagnostics' && (
          <>
            <PageSection>
              <div className="flex items-center justify-between gap-3 rounded-lg bg-surface p-4 ring-1 ring-inset ring-line">
                <div className="min-w-0">
                  <div className="text-md font-medium text-ink">环境检查</div>
                  <div className="mt-0.5 truncate text-sm text-ink-faint">
                    {report
                      ? `生成于 ${formatDateTime(report.generatedAt)} · ${report.root}`
                      : '检查数据目录、版本与 Runtime 完整性、实例引用与下载缓存。'}
                  </div>
                </div>
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={diagLoading}
                  onClick={rerunDiagnostics}
                >
                  {diagLoading ? '检查中…' : '重新检查'}
                </Button>
              </div>
            </PageSection>

            <PageSection>
              {!desktop.isDesktop ? (
                <Notice tone="info">诊断仅在桌面端可用；浏览器预览没有真实的文件系统可检查。</Notice>
              ) : !report ? (
                <div className="rounded-lg bg-surface p-4 text-sm text-ink-faint ring-1 ring-inset ring-line">
                  {diagLoading ? '正在检查…' : '尚无检查结果，点上方「重新检查」生成。'}
                </div>
              ) : (
                <div className="overflow-hidden rounded-lg bg-surface ring-1 ring-inset ring-line">
                  {report.items.map((item, idx) => (
                    <div
                      key={item.id}
                      className={cn(
                        'flex items-start gap-3 px-4 py-3',
                        idx > 0 && 'border-t border-line',
                      )}
                    >
                      <span
                        className={cn(
                          'mt-1.5 h-2 w-2 shrink-0 rounded-full',
                          item.level === 'ok' && 'bg-ok',
                          item.level === 'warn' && 'bg-warn',
                          item.level === 'fail' && 'bg-danger',
                        )}
                      />
                      <div className="min-w-0 flex-1">
                        <div className="text-base text-ink">{item.label}</div>
                        {item.detail && (
                          <div className="mt-0.5 break-all text-sm text-ink-faint">
                            {item.detail}
                          </div>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </PageSection>

            {report && report.cacheBytes > 0 && (
              <PageSection title="清理">
                <Notice tone="info" className="mb-3">
                  下载缓存只用于加速重装；删除它不影响任何已安装的版本、Runtime 与实例。
                </Notice>
                <Button variant="secondary" disabled={cacheBusy} onClick={() => void clearCache()}>
                  {cacheBusy ? '清理中…' : `清理下载缓存（${formatBytes(report.cacheBytes)}）`}
                </Button>
              </PageSection>
            )}
          </>
        )}

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
                  title="隔离 node_modules"
                  description="每个实例使用独立的插件依赖树，避免跨版本污染"
                  control={
                    <Switch
                      checked={settings.isolateNodeModules}
                      onChange={(v) => settings.set('isolateNodeModules', v)}
                    />
                  }
                />
                <SettingRow
                  title="日志级别"
                  control={
                    <Select
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
                <SettingRow
                  title="开发者模式"
                  description="显示实例进程的原始输出与内部状态"
                  control={
                    <Switch
                      checked={settings.developerMode}
                      onChange={(v) => settings.set('developerMode', v)}
                    />
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

/**
 * Full-screen progress for a root migration. The root only flips when the
 * move reports success, so closing this overlay any way other than "done"
 * leaves the old root in charge.
 */
function MigrationOverlay({
  info,
  progress,
  onCancel,
}: {
  info: { from: string; to: string } | null
  progress: MoveProgress | null
  onCancel: () => void
}) {
  const { overlay, pop } = useMotion()
  const kindLabel = progress ? (MOVE_KIND_LABELS[progress.kind] ?? progress.kind) : '准备中…'

  return (
    <AnimatePresence>
      {info && (
        <motion.div key="migration" className="absolute inset-0 z-40 flex items-center justify-center">
          <motion.div
            variants={overlay}
            initial="hidden"
            animate="show"
            exit="out"
            className="absolute inset-0 bg-canvas/70 backdrop-blur-sm"
          />
          <motion.div
            variants={pop}
            initial="hidden"
            animate="show"
            exit="out"
            className="relative w-[420px] rounded-xl bg-surface-raised p-5 shadow-pop ring-1 ring-inset ring-line"
          >
            <div className="flex items-center gap-2.5">
              <HardDrive size={15} className="text-accent" />
              <span className="text-md font-medium text-ink">正在迁移数据</span>
              <span className="num ml-auto text-sm text-ink-faint">
                {progress ? Math.round(progress.progress * 100) : 0}%
              </span>
            </div>

            <ProgressBar value={progress?.progress ?? 0} active className="mt-3.5" height={5} />

            <div className="mt-3.5 space-y-1.5 text-sm">
              <div className="flex items-center gap-2 text-ink">
                <Spinner size={12} />
                正在移动：{kindLabel}
                {progress && (
                  <span className="num ml-auto text-ink-faint">
                    {formatBytes(progress.bytesDone)} / {formatBytes(progress.bytesTotal)}
                  </span>
                )}
              </div>
              <div className="break-all font-mono text-2xs leading-relaxed text-ink-faint">
                {info.from}
                <br />→ {info.to}
              </div>
            </div>

            <div className="mt-4 flex justify-end">
              <Button size="sm" variant="ghost" onClick={onCancel}>
                取消
              </Button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
