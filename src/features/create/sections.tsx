/** The wizard sections: version picker, runtime picker, settings, overlay. */

import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { AnimatePresence, motion, useAnimation } from 'motion/react'
import { ArrowLeft, Check, ChevronDown, CircleDashed, Download, FlaskConical, FolderOpen, Package, Search, Server, Shield, Sparkles, Square, Wrench, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { formatBytes, formatDate, slugify } from '@/lib/format'
import { freeSpace } from '@/lib/desktop'
import { HUES, hueTone } from '@/lib/hue'
import { useMotion } from '@/lib/motion'
import { draftIssues, useCatalogStore, useInstanceStore, useIsDark, useSettingsStore, useUIStore, useViewStore, useWizardStore } from '@/stores'
import { isVersionBusy, isRuntimeBusy, type ApiInheritance, type DshVersion, type InstanceKind, type Runtime } from '@/types'
import { Badge, Button, EmptyState, Field, Input, ProgressBar, SectionCard, Segmented, Spinner, Switch, TextArea } from '@/components/ui'
import { PanelGroup, PanelShell } from '@/components/layout/Panel'
import { InstanceTile } from '@/components/instance'

const KINDS: { id: InstanceKind; label: string; description: string }[] = [
  { id: 'development', label: '开发', description: '插件开发与调试' },
  { id: 'production', label: '生产', description: '日常主力环境' },
  { id: 'test', label: '测试', description: '回归与问题复现' },
  { id: 'sandbox', label: '试验', description: '尝试新版本' },
]

const TEMPLATE_ICONS: Record<string, typeof Square> = {
  square: Square,
  wrench: Wrench,
  shield: Shield,
  flask: FlaskConical,
}


type VersionCategory = 'all' | 'stable' | 'prerelease' | 'nightly' | 'installed'

/** `useAnimation()`'s handle — the shake keyframes target these per section. */
export type AnimControls = ReturnType<typeof useAnimation>

const VERSION_CATEGORIES: { value: VersionCategory; label: string }[] = [
  { value: 'all', label: '全部' },
  { value: 'stable', label: '正式版' },
  { value: 'prerelease', label: '预发布' },
  { value: 'nightly', label: '夜间' },
  { value: 'installed', label: '已安装' },
]

/* ------------------------------------------------------------------ *
 * shared section chrome
 * ------------------------------------------------------------------ */

/**
 * Every block of the form sits in one of these so a failed submit can point
 * at it: the outer layer glows while an issue is open, the inner one takes
 * the shake keyframes.
 */
function SectionShell({
  id,
  controls,
  flash,
  children,
}: {
  id: string
  controls?: AnimControls
  flash: boolean
  children: ReactNode
}) {
  const { t } = useMotion()
  return (
    <motion.div
      id={`create-section-${id}`}
      className="scroll-mt-2 rounded-lg"
      initial={false}
      animate={{
        boxShadow: flash ? '0 0 0 1.5px hsl(var(--c-warn) / 0.55)' : '0 0 0 0px hsl(var(--c-warn) / 0)',
      }}
      transition={t(0.25)}
    >
      {controls ? <motion.div animate={controls}>{children}</motion.div> : children}
    </motion.div>
  )
}

/** Header badge that makes required choices legible at a glance. */
function SectionState({ issue, doneLabel }: { issue?: string; doneLabel: string }) {
  return issue ? <Badge tone="danger">未完成</Badge> : <Badge tone="ok">{doneLabel}</Badge>
}

interface SectionProps {
  issue?: string
  attempted: boolean
  controls: AnimControls
}

/* ------------------------------------------------------------------ *
 * context panel — required-choices checklist
 * ------------------------------------------------------------------ */

export function CreateInstancePanel() {
  const draft = useWizardStore((s) => s.draft)
  const instances = useInstanceStore((s) => s.instances)
  const versions = useCatalogStore((s) => s.versions)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const back = useUIStore((s) => s.back)
  const { scale } = useMotion()

  const issues = draftIssues(draft, instances.map((i) => i.name))
  const version = versions.find((v) => v.id === draft.versionId)
  const runtime = runtimes.find((r) => r.id === draft.runtimeId)

  const checklist = [
    { id: 'name', label: '实例名称', ok: !issues.name, detail: draft.name.trim() || '未填写' },
    { id: 'version', label: 'DSH 版本', ok: !issues.version, detail: version?.name ?? '未选择' },
    { id: 'runtime', label: 'Runtime', ok: !issues.runtime, detail: runtime?.name ?? '未选择' },
  ]

  return (
    <PanelShell>
      <PanelGroup title="必选配置">
        <ul className="mt-1 space-y-1">
          {checklist.map((item) => (
            <li key={item.id}>
              <button
                onClick={() =>
                  document
                    .getElementById(`create-section-${item.id}`)
                    ?.scrollIntoView({ behavior: scale === 0 ? 'auto' : 'smooth', block: 'start' })
                }
                className="flex w-full items-center gap-2.5 rounded-sm px-1.5 py-1.5 text-left transition-colors hover:bg-surface-hover/70"
              >
                <span
                  className={cn(
                    'flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-full ring-1 transition-colors duration-200',
                    item.ok
                      ? 'bg-ok/15 text-ok ring-ok/40'
                      : 'bg-surface text-ink-faint ring-line-strong',
                  )}
                >
                  {item.ok ? (
                    <Check size={10} strokeWidth={3} />
                  ) : (
                    <span className="h-[5px] w-[5px] rounded-full bg-current opacity-50" />
                  )}
                </span>
                <span className="min-w-0">
                  <span
                    className={cn('block text-base', item.ok ? 'text-ink-muted' : 'text-ink')}
                  >
                    {item.label}
                  </span>
                  <span className="block truncate text-sm text-ink-faint">{item.detail}</span>
                </span>
              </button>
            </li>
          ))}
        </ul>
      </PanelGroup>

      <div className="mt-auto space-y-2 p-3">
        <Button variant="ghost" block onClick={back}>
          <ArrowLeft size={13} />
          返回
        </Button>
        <div className="rounded-lg bg-surface-sunken p-3 text-sm leading-relaxed text-ink-faint ring-1 ring-inset ring-line">
          所有配置一次选好后统一创建。PHL 会为实例分配独立的 DSH_HOME、插件目录与 workspace，和其他实例完全隔离。
        </div>
      </div>
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ *
 * left column — DSH version picker
 * ------------------------------------------------------------------ */

export function VersionSection({ issue, attempted, controls }: SectionProps) {
  const draft = useWizardStore((s) => s.draft)
  const patch = useWizardStore((s) => s.patch)
  const versions = useCatalogStore((s) => s.versions)
  const installVersion = useCatalogStore((s) => s.installVersion)
  const { stagger } = useMotion()
  const [category, setCategory] = useState<VersionCategory>('all')
  const [query, setQuery] = useState('')

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    return [...versions]
      .sort((a, b) => {
        const ai = a.state.kind === 'installed' ? 0 : 1
        const bi = b.state.kind === 'installed' ? 0 : 1
        if (ai !== bi) return ai - bi
        return new Date(b.releasedAt).getTime() - new Date(a.releasedAt).getTime()
      })
      .filter((v) => {
        if (category === 'stable' && v.channel !== 'stable') return false
        if (category === 'prerelease' && v.channel !== 'rc' && v.channel !== 'alpha') return false
        if (category === 'nightly' && v.channel !== 'nightly') return false
        if (category === 'installed' && v.state.kind !== 'installed') return false
        if (q && !v.name.toLowerCase().includes(q)) return false
        return true
      })
  }, [versions, category, query])

  return (
    <SectionShell id="version" controls={controls} flash={attempted && !!issue}>
      <SectionCard
        title="选择 DSH 版本"
        description="实例会固定这个版本，与其他实例互不影响"
        extra={<SectionState issue={issue} doneLabel="已选择" />}
      >
        <div className="flex flex-wrap items-center gap-2">
          <Segmented
            value={category}
            options={VERSION_CATEGORIES}
            onChange={setCategory}
            size="sm"
          />
          <div className="min-w-[150px] flex-1">
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="搜索版本"
              prefix={<Search size={13} />}
              suffix={
                query ? (
                  <button
                    onClick={() => setQuery('')}
                    aria-label="清除搜索"
                    className="text-ink-faint transition-colors hover:text-ink"
                  >
                    <X size={12} />
                  </button>
                ) : undefined
              }
            />
          </div>
        </div>

        <div className="mt-2.5 max-h-[340px] overflow-y-auto pr-0.5">
          {filtered.length ? (
            // Re-keying on the category replays the stagger, so switching
            // filters reads as a fresh list rather than a reshuffle.
            <motion.ul
              key={category}
              variants={stagger(0.03)}
              initial="hidden"
              animate="show"
              className="space-y-1.5"
            >
              {filtered.map((v) => (
                <VersionRow
                  key={v.id}
                  version={v}
                  active={draft.versionId === v.id}
                  onSelect={() => patch({ versionId: v.id })}
                  onInstall={() => void installVersion(v.id)}
                />
              ))}
            </motion.ul>
          ) : (
            <EmptyState
              compact
              icon={<Package size={20} />}
              title="没有匹配的版本"
              description="换个分类或搜索词试试"
              action={
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => {
                    setCategory('all')
                    setQuery('')
                  }}
                >
                  清除筛选
                </Button>
              }
            />
          )}
        </div>
      </SectionCard>
    </SectionShell>
  )
}

function VersionRow({
  version,
  active,
  onSelect,
  onInstall,
}: {
  version: DshVersion
  active: boolean
  onSelect: () => void
  onInstall: () => void
}) {
  const { t, scale, riseItem } = useMotion()
  const installed = version.state.kind === 'installed'
  const busy = isVersionBusy(version)

  return (
    <motion.li variants={riseItem} layout="position">
      <div
        className={cn(
          'flex w-full items-center gap-3 rounded-lg px-3 py-2.5 ring-1 ring-inset transition-all duration-150',
          active ? 'bg-accent-soft ring-accent' : 'bg-surface ring-line hover:ring-line-strong',
        )}
      >
        <button onClick={onSelect} className="flex min-w-0 flex-1 items-center gap-3 text-left">
        <span
          className={cn(
            'flex h-8 w-8 shrink-0 items-center justify-center rounded-lg',
            installed ? 'bg-ok/10 text-ok' : 'bg-surface-sunken text-ink-faint',
          )}
        >
          {busy ? <Spinner size={14} /> : <Package size={15} />}
        </span>

        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-2">
            <span className="font-mono text-base font-medium text-ink">{version.name}</span>
            {version.latest && <Badge tone="accent">最新</Badge>}
            {version.legacy && <Badge tone="neutral">Legacy</Badge>}
            {version.channel === 'nightly' && <Badge tone="warn">Nightly</Badge>}
            {installed ? (
              <Badge tone="ok">已安装</Badge>
            ) : busy ? (
              <Badge tone="accent">安装中</Badge>
            ) : (
              <Badge tone="outline">未安装</Badge>
            )}
          </span>
          <span className="mt-0.5 block text-sm text-ink-faint">
            {formatDate(version.releasedAt)} · {formatBytes(version.size)} · 需要 Node{' '}
            {version.requiresNode.join(' / ')}
          </span>
        </span>
        <AnimatePresence initial={false}>
          {active && (
            <motion.span
              key="check"
              initial={{ scale: scale === 0 ? 1 : 0.4, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              exit={{ scale: scale === 0 ? 1 : 0.4, opacity: 0 }}
              transition={t(0.18)}
              className="flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-full bg-accent text-[hsl(var(--c-accent-on))]"
            >
              <Check size={11} strokeWidth={3} />
            </motion.span>
          )}
        </AnimatePresence>
        </button>
        {!installed && !busy && (
          <Button size="sm" variant="secondary" onClick={onInstall}>
            <Download size={12} />
            安装
          </Button>
        )}
      </div>
    </motion.li>
  )
}

/* ------------------------------------------------------------------ *
 * left column — runtime picker
 * ------------------------------------------------------------------ */

export function RuntimeSection({ issue, attempted, controls }: SectionProps) {
  const draft = useWizardStore((s) => s.draft)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const version = useCatalogStore((s) => s.versions.find((v) => v.id === draft.versionId))

  return (
    <SectionShell id="runtime" controls={controls} flash={attempted && !!issue}>
      <SectionCard
        title="Runtime"
        description="运行 DSH 的 Node 环境"
        extra={<SectionState issue={issue} doneLabel="已选择" />}
      >
        {version && version.requiresNode.length > 0 && (
          <p className="mb-2 text-sm text-ink-faint">
            DSH {version.name} 在 Node {version.requiresNode.join(' / ')} 上验证过；其余大版本可以启动，但首次运行前
            PHL 会提示风险。
          </p>
        )}
        <div className="grid gap-1.5 sm:grid-cols-2">
          {runtimes.map((r) => (
            <RuntimeCard
              key={r.id}
              runtime={r}
              active={draft.runtimeId === r.id}
              recommended={!!version?.requiresNode.includes(r.major)}
              hasVersion={!!version}
              onSelect={() => useWizardStore.getState().patch({ runtimeId: r.id })}
              onInstall={() => void useCatalogStore.getState().installRuntime(r.id)}
            />
          ))}
        </div>
      </SectionCard>
    </SectionShell>
  )
}

function RuntimeCard({
  runtime,
  active,
  recommended,
  hasVersion,
  onSelect,
  onInstall,
}: {
  runtime: Runtime
  active: boolean
  recommended: boolean
  hasVersion: boolean
  onSelect: () => void
  onInstall: () => void
}) {
  const { t, scale } = useMotion()
  const installed = runtime.state.kind === 'installed'
  const busy = isRuntimeBusy(runtime)
  const unverified = hasVersion && !recommended

  return (
    <div
      className={cn(
        'rounded-lg p-2.5 text-left ring-1 ring-inset transition-all duration-150',
        active ? 'bg-accent-soft ring-accent' : 'bg-surface ring-line hover:ring-line-strong',
      )}
    >
      <button onClick={onSelect} className="w-full text-left">
      <div className="flex items-center gap-2">
        <span
          className={cn(
            'flex h-7 w-7 shrink-0 items-center justify-center rounded-md',
            installed ? 'bg-ok/10 text-ok' : 'bg-surface-sunken text-ink-faint',
          )}
        >
          {busy ? <Spinner size={13} /> : <Server size={14} />}
        </span>
        <span className="min-w-0">
          <span
            className={cn('block truncate text-base font-medium', active ? 'text-accent-ink' : 'text-ink')}
          >
            {runtime.name}
          </span>
          <span className="num block font-mono text-2xs text-ink-faint">v{runtime.version}</span>
        </span>
        <AnimatePresence initial={false}>
          {active && (
            <motion.span
              key="check"
              initial={{ scale: scale === 0 ? 1 : 0.4, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              exit={{ scale: scale === 0 ? 1 : 0.4, opacity: 0 }}
              transition={t(0.18)}
              className="ml-auto flex h-[17px] w-[17px] shrink-0 items-center justify-center rounded-full bg-accent text-[hsl(var(--c-accent-on))]"
            >
              <Check size={10} strokeWidth={3} />
            </motion.span>
          )}
        </AnimatePresence>
      </div>

      <div className="mt-2 flex flex-wrap items-center gap-1">
        {runtime.lts && <Badge tone="neutral">LTS</Badge>}
        {runtime.system && <Badge tone="outline">系统</Badge>}
        {recommended && <Badge tone="ok">推荐</Badge>}
        {unverified && <Badge tone="warn">未验证</Badge>}
        <span className="num ml-auto text-2xs text-ink-faint">
          {installed ? '已安装' : busy ? '安装中' : formatBytes(runtime.size)}
        </span>
      </div>
      </button>
      {!installed && !busy && (
        <div className="mt-2 flex justify-end">
          <Button size="xs" variant="secondary" onClick={onInstall}>
            <Download size={11} />
            安装
          </Button>
        </div>
      )}
    </div>
  )
}

/* ------------------------------------------------------------------ *
 * right column — identity settings
 * ------------------------------------------------------------------ */

export function SettingsSection({
  nameIssue,
  attempted,
  controls,
  nameInputRef,
}: {
  nameIssue?: string
  attempted: boolean
  controls: AnimControls
  nameInputRef: React.RefObject<HTMLInputElement>
}) {
  const draft = useWizardStore((s) => s.draft)
  const patch = useWizardStore((s) => s.patch)
  const dark = useIsDark()

  // Duplicates are shown immediately (the user just typed a taken name);
  // the empty-name error waits for a submit attempt so it never nags.
  const showNameError =
    !!nameIssue && (attempted || (draft.name.trim().length > 0 && nameIssue === '已存在同名实例'))

  return (
    <SectionShell id="name" controls={controls} flash={attempted && !!nameIssue}>
      <SectionCard
        title="实例设置"
        extra={<SectionState issue={nameIssue} doneLabel="已填写" />}
      >
        <div className="flex items-start gap-3">
          <InstanceTile name={draft.name || '新实例'} hue={draft.hue} size={44} />
          <Field
            className="min-w-0 flex-1"
            label="实例名称"
            required
            error={showNameError ? nameIssue : undefined}
            hint={showNameError ? undefined : '用于区分不同环境，例如 Plugin Dev'}
          >
            <Input
              ref={nameInputRef}
              value={draft.name}
              onChange={(e) => patch({ name: e.target.value })}
              placeholder="例如：Plugin Development"
              invalid={showNameError}
            />
          </Field>
        </div>

        <div className="mt-3.5 space-y-3.5">
          <Field label="用途">
            <div className="grid grid-cols-4 gap-1.5">
              {KINDS.map((k) => {
                const active = draft.kind === k.id
                return (
                  <button
                    key={k.id}
                    title={k.description}
                    onClick={() => patch({ kind: k.id })}
                    className={cn(
                      'rounded-md py-1.5 text-center text-sm ring-1 ring-inset transition-all duration-150',
                      active
                        ? 'bg-accent-soft font-medium text-accent-ink ring-accent'
                        : 'bg-surface text-ink-muted ring-line hover:ring-line-strong',
                    )}
                  >
                    {k.label}
                  </button>
                )
              })}
            </div>
          </Field>

          <Field label="标识颜色" hint="在列表和启动栏中快速辨认这个实例">
            <div className="flex gap-2">
              {HUES.map((h) => {
                const tone = hueTone(h.id, dark)
                const active = draft.hue === h.id
                return (
                  <button
                    key={h.id}
                    title={h.label}
                    onClick={() => patch({ hue: h.id })}
                    className={cn(
                      'h-6 w-6 rounded-md transition-transform duration-150',
                      active ? 'scale-110' : 'hover:scale-105',
                    )}
                    style={{
                      background: tone.soft,
                      boxShadow: active
                        ? `inset 0 0 0 2px ${tone.solid}`
                        : `inset 0 0 0 1px ${tone.ring}`,
                    }}
                  >
                    <span className="sr-only">{h.label}</span>
                  </button>
                )
              })}
            </div>
          </Field>
        </div>
      </SectionCard>
    </SectionShell>
  )
}

/**
 * Everything a first run can live with defaults for. Owns its open state so
 * a failed submit can force it open when the port is the problem.
 */
export function MoreSettings({ portIssue, attempted }: { portIssue?: string; attempted: boolean }) {
  const [open, setOpen] = useState(false)
  const draft = useWizardStore((s) => s.draft)
  const patch = useWizardStore((s) => s.patch)
  const templates = useCatalogStore((s) => s.templates)
  const suggestPort = useInstanceStore((s) => s.suggestPort)
  const { t, scale } = useMotion()

  useEffect(() => {
    if (attempted && portIssue) setOpen(true)
  }, [attempted, portIssue])

  return (
    <SectionShell id="port" flash={!!portIssue && attempted}>
      <div className="overflow-hidden rounded-lg bg-surface ring-1 ring-inset ring-line">
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="flex min-h-[32px] w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-surface-hover/60"
        >
          <motion.span
            animate={{ rotate: open ? 0 : -90 }}
            transition={t(0.2)}
            className="-ml-1 text-ink-faint"
          >
            <ChevronDown size={14} />
          </motion.span>
          <h3 className="text-base font-medium text-ink">更多设置</h3>
          <span className="truncate text-sm text-ink-faint">模板 · 端口 · 备注</span>
        </button>

        <AnimatePresence initial={false}>
          {open && (
            <motion.div
              key="body"
              initial={scale === 0 ? false : { height: 0, opacity: 0 }}
              animate={{ height: 'auto', opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={t(0.24)}
              className="overflow-hidden"
            >
              <div className="space-y-3.5 border-t border-line px-3 py-2.5">
                <Field label="模板" hint="只决定初始插件组合，之后随时可以调整">
                  <div className="grid grid-cols-2 gap-1.5">
                    {templates.map((tpl) => {
                      const Icon = TEMPLATE_ICONS[tpl.icon] ?? Square
                      const active = draft.templateId === tpl.id
                      return (
                        <button
                          key={tpl.id}
                          title={tpl.description}
                          onClick={() => patch({ templateId: tpl.id })}
                          className={cn(
                            'flex items-center gap-1.5 rounded-md px-2 py-1.5 text-left ring-1 ring-inset transition-all duration-150',
                            active
                              ? 'bg-accent-soft ring-accent'
                              : 'bg-surface ring-line hover:ring-line-strong',
                          )}
                        >
                          <Icon
                            size={13}
                            className={active ? 'text-accent-ink' : 'text-ink-faint'}
                          />
                          <span
                            className={cn(
                              'truncate text-sm font-medium',
                              active ? 'text-accent-ink' : 'text-ink',
                            )}
                          >
                            {tpl.name}
                          </span>
                          {tpl.plugins.length > 0 && (
                            <span className="num ml-auto shrink-0 text-2xs text-ink-faint">
                              {tpl.plugins.length}
                            </span>
                          )}
                        </button>
                      )
                    })}
                  </div>
                </Field>

                <Field label="端口" error={portIssue}>
                  <div className="flex items-center gap-3 rounded-md bg-surface-sunken px-2.5 py-2 ring-1 ring-inset ring-line">
                    <div className="min-w-0 flex-1">
                      <div className="text-sm text-ink">自动分配端口</div>
                      <div className="mt-0.5 text-2xs text-ink-faint">
                        启动时选择第一个空闲端口，避免多实例冲突
                      </div>
                    </div>
                    <Switch
                      checked={draft.autoPort}
                      onChange={(v) => patch({ autoPort: v, port: v ? draft.port : suggestPort() })}
                      label="自动分配端口"
                    />
                  </div>
                  <AnimatePresence initial={false}>
                    {!draft.autoPort && (
                      <motion.div
                        initial={{ height: 0, opacity: 0 }}
                        animate={{ height: 'auto', opacity: 1 }}
                        exit={{ height: 0, opacity: 0 }}
                        transition={t(0.24)}
                        className="overflow-hidden"
                      >
                        <div className="pt-2">
                          <Input
                            type="number"
                            value={draft.port}
                            min={1024}
                            max={65535}
                            invalid={!!portIssue}
                            onChange={(e) => patch({ port: Number(e.target.value) })}
                            prefix="localhost:"
                            className="w-[200px]"
                          />
                        </div>
                      </motion.div>
                    )}
                  </AnimatePresence>
                </Field>

                <Field
                  label="模型与 API"
                  hint="全局库里的供应商配置如何进入这个实例的 DSH_HOME"
                >
                  <Segmented
                    size="sm"
                    value={draft.apiInheritance}
                    onChange={(v) => patch({ apiInheritance: v as ApiInheritance })}
                    options={[
                      { value: 'default', label: '继承全局配置' },
                      { value: 'none', label: '不托管（自行配置）' },
                    ]}
                  />
                </Field>

                <Field label="备注">
                  <TextArea
                    rows={2}
                    value={draft.note}
                    onChange={(e) => patch({ note: e.target.value })}
                    placeholder="这个实例用来做什么？"
                  />
                </Field>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </SectionShell>
  )
}

export function PathPreview() {
  const draft = useWizardStore((s) => s.draft)
  const suggestPort = useInstanceStore((s) => s.suggestPort)
  const settingsRoot = useSettingsStore((s) => s.root)
  const version = useCatalogStore((s) => s.versions.find((v) => v.id === draft.versionId))
  const runtime = useCatalogStore((s) => s.runtimes.find((r) => r.id === draft.runtimeId))
  const navigate = useUIStore((s) => s.navigate)
  const setSettingsSection = useViewStore((s) => s.setSettingsSection)

  const root = `${settingsRoot}\\instances\\${slugify(draft.name || 'instance')}`
  const port = draft.autoPort ? suggestPort() : draft.port

  // Free bytes on the root's drive; `undefined` = not answered yet, `null` =
  // the drive could not be queried (shown as unknown rather than hidden, so
  // the figure's absence never looks like a bug).
  const [free, setFree] = useState<number | null | undefined>(undefined)
  useEffect(() => {
    let alive = true
    void freeSpace(settingsRoot).then((n) => {
      if (alive) setFree(n)
    })
    return () => {
      alive = false
    }
  }, [settingsRoot])

  // Rough "will this fit" check: the version and runtime the instance pins
  // must download before first launch. System runtimes are already on disk.
  const need = (version?.size ?? 0) + (runtime && !runtime.system ? runtime.size : 0)
  const lowSpace = free != null && need > 0 && free < need

  const gotoStorage = () => {
    setSettingsSection('storage')
    navigate({ name: 'settings' })
  }

  return (
    <div className="rounded-lg bg-surface-sunken p-3 ring-1 ring-inset ring-line">
      <div className="mb-1.5 flex items-center gap-1.5 text-sm font-medium text-ink-muted">
        <CircleDashed size={12} />
        将要创建的隔离目录
        <Button size="xs" variant="ghost" className="ml-auto" onClick={gotoStorage}>
          <FolderOpen size={11} />
          更改
        </Button>
      </div>
      <ul className="space-y-1">
        {(
          [
            ['版本', version?.name ?? '—'],
            ['Runtime', runtime?.name ?? '—'],
            ['DSH_HOME', `${root}\\dsh-home`],
            ['Workspace', `${root}\\workspace`],
            ['端口', `localhost:${port}${draft.autoPort ? '（自动）' : ''}`],
          ] as const
        ).map(([label, value]) => (
          <li key={label} className="flex items-baseline gap-2">
            <span className="w-[64px] shrink-0 text-2xs text-ink-faint">{label}</span>
            <span
              className={cn(
                'min-w-0 flex-1 break-all text-2xs',
                label === '版本' || label === 'Runtime' || label === '端口'
                  ? 'text-ink-muted'
                  : 'font-mono text-ink-muted',
              )}
            >
              {value}
            </span>
          </li>
        ))}
      </ul>
      <p className="mt-2 break-all text-2xs leading-relaxed text-ink-faint">
        数据目录 {settingsRoot}
      </p>
      <p className={cn('mt-0.5 text-2xs', lowSpace ? 'text-warn' : 'text-ink-faint')}>
        {free == null
          ? '所在盘剩余空间未知'
          : `所在盘剩余 ${formatBytes(free)}`}
        {lowSpace &&
          ` · 低于本次所需（版本与 Runtime 约 ${formatBytes(need)}），建议先更改存储位置`}
      </p>
    </div>
  )
}

/* ------------------------------------------------------------------ *
 * creating overlay
 * ------------------------------------------------------------------ */

const CREATE_STEPS: { id: string; label: string }[] = [
  { id: 'create', label: '创建实例目录' },
  { id: 'environment', label: '准备运行环境' },
  { id: 'configure', label: '写入隔离配置' },
  { id: 'plugins', label: '安装模板插件' },
]

export function CreatingOverlay() {
  const progress = useInstanceStore((s) => s.createProgress)
  const cancel = useInstanceStore((s) => s.cancelCreate)
  const { t, overlay, pop } = useMotion()

  const currentIndex = progress ? CREATE_STEPS.findIndex((s) => s.id === progress.step) : -1

  return (
    <AnimatePresence>
      {progress && (
        <motion.div
          key="creating"
          className="absolute inset-0 z-40 flex items-center justify-center"
        >
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
            className="relative w-[400px] rounded-xl bg-surface-raised p-5 shadow-pop ring-1 ring-inset ring-line"
          >
            <div className="flex items-center gap-2.5">
              <Sparkles size={15} className="text-accent" />
              <span className="text-md font-medium text-ink">正在创建实例</span>
              <span className="num ml-auto text-sm text-ink-faint">
                {Math.round(progress.progress * 100)}%
              </span>
            </div>

            <ProgressBar value={progress.progress} active className="mt-3.5" height={5} />

            <ol className="mt-4 space-y-1.5">
              {CREATE_STEPS.map((s, index) => {
                const done = progress.step === 'done' || index < currentIndex
                const active = index === currentIndex
                return (
                  <li
                    key={s.id}
                    className={cn(
                      'flex items-center gap-2.5 text-base transition-colors duration-200',
                      active ? 'text-ink' : done ? 'text-ink-muted' : 'text-ink-faint',
                    )}
                  >
                    <span className="flex h-4 w-4 items-center justify-center">
                      {done ? (
                        <motion.span
                          initial={{ scale: 0.5, opacity: 0 }}
                          animate={{ scale: 1, opacity: 1 }}
                          transition={t(0.2)}
                          className="flex h-[15px] w-[15px] items-center justify-center rounded-full bg-ok/15 text-ok"
                        >
                          <Check size={10} strokeWidth={3} />
                        </motion.span>
                      ) : active ? (
                        <Spinner size={13} weight={2.4} className="text-accent" />
                      ) : (
                        <span className="h-[6px] w-[6px] rounded-full bg-current opacity-40" />
                      )}
                    </span>
                    {s.label}
                  </li>
                )
              })}
            </ol>

            <div className="mt-4 flex justify-end">
              <Button size="sm" variant="ghost" onClick={cancel}>
                取消
              </Button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}

/* ------------------------------------------------------------------ *
 * page
 * ------------------------------------------------------------------ */
