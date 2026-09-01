import { useEffect, useState, type ReactNode } from 'react'
import { motion } from 'motion/react'
import {
  BookOpen,
  Download,
  FolderOpen,
  HardDrive,
  Info,
  Palette,
  Settings2,
  SlidersHorizontal,
  Sparkles,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { chooseDirectory, desktop } from '@/lib/desktop'
import { formatBytes } from '@/lib/format'
import { hueTone } from '@/lib/hue'
import { useMotion } from '@/lib/motion'
import {
  ACCENTS,
  normalizeRoot,
  useCatalogStore,
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
import { Button, Input, Notice, Segmented, SettingRow, Switch, Select } from '@/components/ui'
import { PageShell, PageSection } from '@/components/layout/Page'
import { PanelGroup, PanelItem, PanelShell } from '@/components/layout/Panel'
import { Logo } from '@/components/layout/Logo'

const SECTIONS: { id: SettingsSection; label: string; icon: ReactNode }[] = [
  { id: 'general', label: '通用', icon: <Settings2 size={13} /> },
  { id: 'downloads', label: '下载', icon: <Download size={13} /> },
  { id: 'appearance', label: '外观', icon: <Palette size={13} /> },
  { id: 'storage', label: '存储', icon: <HardDrive size={13} /> },
  { id: 'advanced', label: '高级', icon: <SlidersHorizontal size={13} /> },
  { id: 'about', label: '关于', icon: <Info size={13} /> },
]

/** Common places to park a multi-gigabyte data directory. */
const ROOT_PRESETS = [
  { label: 'D:\\PHL', path: 'D:\\PHL' },
  { label: 'E:\\PHL', path: 'E:\\PHL' },
  { label: '用户目录', path: 'C:\\Users\\dev\\AppData\\Local\\PHL' },
]

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
  const relocate = useInstanceStore((s) => s.relocate)
  const versions = useCatalogStore((s) => s.versions)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const { t } = useMotion()

  const [rootDraft, setRootDraft] = useState(settings.root)

  // Keep the field in step when the root changes from somewhere else.
  useEffect(() => setRootDraft(settings.root), [settings.root])

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
    const ok = await ui.confirm({
      title: '更改数据目录',
      message:
        '已有的版本、Runtime 与实例会迁移到新位置。迁移期间请不要关闭 PHL；运行中的实例需要先停止。',
      detail: `${settings.root}\n  ↓\n${next}`,
      confirmLabel: '迁移',
    })
    if (!ok) return
    const from = settings.root
    settings.setRoot(next)
    relocate(from, normalizeRoot(next))
    ui.toast({
      kind: 'success',
      title: '数据目录已更新',
      message: '接入 PHL Core 后会执行真实的文件迁移。',
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

        {section === 'storage' && (
          <>
            <PageSection
              title="数据目录"
              description="版本、Runtime 与所有实例都存放在这里。这不是 PHL 程序本身的安装位置。"
            >
              <div className="rounded-lg bg-surface p-3 ring-1 ring-inset ring-line">
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
                  {ROOT_PRESETS.map((preset) => (
                    <button
                      key={preset.path}
                      onClick={() => setRootDraft(preset.path)}
                      className="rounded-xs bg-surface-sunken px-1.5 py-0.5 font-mono text-2xs text-ink-muted ring-1 ring-inset ring-line transition-colors hover:bg-surface-hover hover:text-ink"
                    >
                      {preset.label}
                    </button>
                  ))}
                </div>

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
                    <span>Phase 1 · 前端原型</span>
                  </div>
                </div>
              </div>
            </PageSection>

            <PageSection title="当前阶段">
              <div className="rounded-lg bg-surface p-3 text-sm leading-relaxed text-ink-muted ring-1 ring-inset ring-line">
                <p>
                  这是一个纯前端高保真原型：所有数据来自 Mock Repository，启动、下载与创建都是模拟过程，
                  不会真正操作文件系统或进程。
                </p>
                <p className="mt-2">
                  它的目的是验证「PCL 式实例管理器」这一产品模型：实例是第一公民，版本、Runtime 与插件
                  都从属于实例。
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
  )
}
