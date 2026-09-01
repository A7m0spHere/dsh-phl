import { useEffect, useMemo, type ComponentType, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  Check,
  ChevronLeft,
  ChevronRight,
  CircleDashed,
  Download,
  FlaskConical,
  Package,
  Server,
  Shield,
  Sparkles,
  Square,
  Wrench,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { formatBytes, formatDate, slugify } from '@/lib/format'
import { HUES, hueTone } from '@/lib/hue'
import { useMotion } from '@/lib/motion'
import { PHL_ROOT } from '@/data/instances'
import {
  stepStatus,
  useCatalogStore,
  useInstanceStore,
  useIsDark,
  useUIStore,
  useWizardStore,
  WIZARD_STEPS,
  type WizardStep,
} from '@/stores'
import type { InstanceKind } from '@/types'
import {
  Badge,
  Button,
  Chip,
  Field,
  Input,
  Notice,
  ProgressBar,
  Spinner,
  Switch,
  TextArea,
} from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
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

/* ------------------------------------------------------------------ *
 * context panel — the step tracker
 * ------------------------------------------------------------------ */

export function CreateInstancePanel() {
  const step = useWizardStore((s) => s.step)
  const draft = useWizardStore((s) => s.draft)
  const setStep = useWizardStore((s) => s.setStep)
  const instances = useInstanceStore((s) => s.instances)
  const { t, scale } = useMotion()

  const currentIndex = WIZARD_STEPS.findIndex((s) => s.id === step)
  const names = instances.map((i) => i.name)

  return (
    <PanelShell>
      <PanelGroup title="创建实例">
        <ol className="relative mt-1">
          {/* The rail fills as the wizard advances, so progress is readable
              without counting checkmarks. */}
          <span
            aria-hidden
            className="pointer-events-none absolute inset-y-4 left-[13px] w-px overflow-hidden bg-line"
          >
            <motion.span
              className="block w-px bg-accent"
              initial={false}
              animate={{
                height: `${(currentIndex / Math.max(1, WIZARD_STEPS.length - 1)) * 100}%`,
              }}
              transition={t(0.34)}
            />
          </span>

          {WIZARD_STEPS.map((s, index) => {
            const done = index < currentIndex && stepStatus(s.id, draft, names).ok
            const active = s.id === step
            const reachable = index <= currentIndex
            return (
              <li key={s.id}>
                <button
                  disabled={!reachable}
                  onClick={() => setStep(s.id)}
                  className={cn(
                    'relative flex w-full items-center gap-2.5 rounded-sm py-2 pl-1.5 pr-2 text-left transition-colors',
                    reachable ? 'hover:bg-surface-hover/70' : 'cursor-default opacity-45',
                  )}
                >
                  <span
                    className={cn(
                      'relative z-10 flex h-[22px] w-[22px] shrink-0 items-center justify-center rounded-full ring-1 transition-colors duration-200',
                      done
                        ? 'bg-accent text-[hsl(var(--c-accent-on))] ring-accent'
                        : active
                          ? 'bg-surface text-accent-ink ring-accent'
                          : 'bg-surface text-ink-faint ring-line-strong',
                    )}
                  >
                    <AnimatePresence mode="wait" initial={false}>
                      {done ? (
                        <motion.span
                          key="check"
                          initial={{ scale: scale === 0 ? 1 : 0.4, opacity: 0 }}
                          animate={{ scale: 1, opacity: 1 }}
                          exit={{ opacity: 0 }}
                          transition={t(0.18)}
                        >
                          <Check size={12} strokeWidth={3} />
                        </motion.span>
                      ) : (
                        <motion.span
                          key="num"
                          initial={{ opacity: 0 }}
                          animate={{ opacity: 1 }}
                          exit={{ opacity: 0 }}
                          className="num text-2xs font-semibold"
                        >
                          {index + 1}
                        </motion.span>
                      )}
                    </AnimatePresence>
                  </span>
                  <span className="min-w-0">
                    <span
                      className={cn(
                        'block truncate text-base',
                        active ? 'font-medium text-ink' : 'text-ink-muted',
                      )}
                    >
                      {s.label}
                    </span>
                    <span className="block truncate text-sm text-ink-faint">{s.hint}</span>
                  </span>
                </button>
              </li>
            )
          })}
        </ol>
      </PanelGroup>

      <div className="mt-auto p-3">
        <div className="rounded-lg bg-surface-sunken p-3 text-sm leading-relaxed text-ink-faint ring-1 ring-inset ring-line">
          创建后 PHL 会为这个实例分配独立的 DSH_HOME、插件目录与 workspace，和其他实例完全隔离。
        </div>
      </div>
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ *
 * step bodies
 * ------------------------------------------------------------------ */

function BasicsStep() {
  const draft = useWizardStore((s) => s.draft)
  const patch = useWizardStore((s) => s.patch)
  const dark = useIsDark()

  return (
    <div className="space-y-5">
      <div className="flex gap-4">
        <InstanceTile name={draft.name || '新实例'} hue={draft.hue} size={52} />
        <div className="flex-1 space-y-4">
          <Field label="实例名称" required hint="用于区分不同环境，例如 Plugin Dev / Legacy Test">
            <Input
              autoFocus
              value={draft.name}
              onChange={(e) => patch({ name: e.target.value })}
              placeholder="例如：Plugin Development"
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
      </div>

      <Field label="用途">
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          {KINDS.map((k) => (
            <button
              key={k.id}
              onClick={() => patch({ kind: k.id })}
              className={cn(
                'rounded-lg px-3 py-2.5 text-left ring-1 ring-inset transition-all duration-150',
                draft.kind === k.id
                  ? 'bg-accent-soft ring-accent'
                  : 'bg-surface ring-line hover:ring-line-strong',
              )}
            >
              <div
                className={cn(
                  'text-base font-medium',
                  draft.kind === k.id ? 'text-accent-ink' : 'text-ink',
                )}
              >
                {k.label}
              </div>
              <div className="mt-0.5 text-sm text-ink-faint">{k.description}</div>
            </button>
          ))}
        </div>
      </Field>

      <Field label="标识颜色" hint="用于在列表、启动栏和详情页中快速辨认这个实例">
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
                  'h-7 w-7 rounded-lg transition-transform duration-150',
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
  )
}

function VersionStep() {
  const draft = useWizardStore((s) => s.draft)
  const patch = useWizardStore((s) => s.patch)
  const versions = useCatalogStore((s) => s.versions)
  const installVersion = useCatalogStore((s) => s.installVersion)
  const { stagger, riseItem } = useMotion()

  const sorted = useMemo(
    () =>
      [...versions].sort((a, b) => {
        const ai = a.state.kind === 'installed' ? 0 : 1
        const bi = b.state.kind === 'installed' ? 0 : 1
        if (ai !== bi) return ai - bi
        return new Date(b.releasedAt).getTime() - new Date(a.releasedAt).getTime()
      }),
    [versions],
  )

  return (
    <div className="space-y-3">
      <Notice tone="info">
        实例会永久固定这个版本。升级其他实例不会影响它，这正是多版本共存的关键。
      </Notice>

      <motion.ul variants={stagger(0.03)} initial="hidden" animate="show" className="space-y-1.5">
        {sorted.map((v) => {
          const installed = v.state.kind === 'installed'
          const busy = ['queued', 'downloading', 'extracting', 'verifying'].includes(v.state.kind)
          const active = draft.versionId === v.id
          return (
            <motion.li key={v.id} variants={riseItem}>
              <button
                onClick={() => patch({ versionId: v.id })}
                className={cn(
                  'flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left ring-1 ring-inset transition-all duration-150',
                  active
                    ? 'bg-accent-soft ring-accent'
                    : 'bg-surface ring-line hover:ring-line-strong',
                )}
              >
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
                    <span className="font-mono text-base font-medium text-ink">{v.name}</span>
                    {v.latest && <Badge tone="accent">最新</Badge>}
                    {v.legacy && <Badge tone="neutral">Legacy</Badge>}
                    {v.channel === 'nightly' && <Badge tone="warn">Nightly</Badge>}
                    {installed ? (
                      <Badge tone="ok">已安装</Badge>
                    ) : busy ? (
                      <Badge tone="accent">安装中</Badge>
                    ) : (
                      <Badge tone="outline">未安装</Badge>
                    )}
                  </span>
                  <span className="mt-0.5 block text-sm text-ink-faint">
                    {formatDate(v.releasedAt)} · {formatBytes(v.size)} · 需要 Node{' '}
                    {v.requiresNode.join(' / ')}
                  </span>
                </span>

                {!installed && !busy && (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={(e) => {
                      e.stopPropagation()
                      void installVersion(v.id)
                    }}
                  >
                    <Download size={12} />
                    安装
                  </Button>
                )}
              </button>
            </motion.li>
          )
        })}
      </motion.ul>
    </div>
  )
}

function RuntimeStep() {
  const draft = useWizardStore((s) => s.draft)
  const patch = useWizardStore((s) => s.patch)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const installRuntime = useCatalogStore((s) => s.installRuntime)
  const version = useCatalogStore((s) => s.versions.find((v) => v.id === draft.versionId))
  const { stagger, riseItem } = useMotion()

  return (
    <div className="space-y-3">
      {version && (
        <Notice tone="info">
          DSH {version.name} 在 Node {version.requiresNode.join(' / ')} 上验证过。选择其他版本可以启动，但
          PHL 会在启动前提示风险。
        </Notice>
      )}

      <motion.ul variants={stagger(0.03)} initial="hidden" animate="show" className="space-y-1.5">
        {runtimes.map((r) => {
          const installed = r.state.kind === 'installed'
          const busy = ['downloading', 'extracting'].includes(r.state.kind)
          const active = draft.runtimeId === r.id
          const recommended = version?.requiresNode.includes(r.major)
          return (
            <motion.li key={r.id} variants={riseItem}>
              <button
                onClick={() => patch({ runtimeId: r.id })}
                className={cn(
                  'flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left ring-1 ring-inset transition-all duration-150',
                  active
                    ? 'bg-accent-soft ring-accent'
                    : 'bg-surface ring-line hover:ring-line-strong',
                )}
              >
                <span
                  className={cn(
                    'flex h-8 w-8 shrink-0 items-center justify-center rounded-lg',
                    installed ? 'bg-ok/10 text-ok' : 'bg-surface-sunken text-ink-faint',
                  )}
                >
                  {busy ? <Spinner size={14} /> : <Server size={15} />}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-2">
                    <span className="text-base font-medium text-ink">{r.name}</span>
                    <span className="font-mono text-sm text-ink-faint">v{r.version}</span>
                    {r.lts && <Badge tone="neutral">LTS</Badge>}
                    {r.system && <Badge tone="outline">系统</Badge>}
                    {recommended && <Badge tone="ok">推荐</Badge>}
                    {version && !recommended && <Badge tone="warn">未验证</Badge>}
                  </span>
                  <span className="mt-0.5 block text-sm text-ink-faint">
                    {installed ? '已安装' : busy ? '安装中' : `未安装 · ${formatBytes(r.size)}`}
                  </span>
                </span>
                {!installed && !busy && (
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={(e) => {
                      e.stopPropagation()
                      void installRuntime(r.id)
                    }}
                  >
                    <Download size={12} />
                    安装
                  </Button>
                )}
              </button>
            </motion.li>
          )
        })}
      </motion.ul>
    </div>
  )
}

function TemplateStep() {
  const draft = useWizardStore((s) => s.draft)
  const patch = useWizardStore((s) => s.patch)
  const templates = useCatalogStore((s) => s.templates)
  const plugins = useCatalogStore((s) => s.plugins)
  const suggestPort = useInstanceStore((s) => s.suggestPort)

  return (
    <div className="space-y-5">
      <Field label="模板" hint="模板只决定初始插件组合，之后随时可以调整">
        <div className="grid gap-2 sm:grid-cols-2">
          {templates.map((tpl) => {
            const Icon = TEMPLATE_ICONS[tpl.icon] ?? Square
            const active = draft.templateId === tpl.id
            return (
              <button
                key={tpl.id}
                onClick={() => patch({ templateId: tpl.id })}
                className={cn(
                  'rounded-lg p-3 text-left ring-1 ring-inset transition-all duration-150',
                  active
                    ? 'bg-accent-soft ring-accent'
                    : 'bg-surface ring-line hover:ring-line-strong',
                )}
              >
                <div className="flex items-center gap-2">
                  <Icon size={14} className={active ? 'text-accent-ink' : 'text-ink-faint'} />
                  <span
                    className={cn(
                      'text-base font-medium',
                      active ? 'text-accent-ink' : 'text-ink',
                    )}
                  >
                    {tpl.name}
                  </span>
                  <span className="ml-auto text-2xs text-ink-faint">
                    {tpl.plugins.length ? `${tpl.plugins.length} 插件` : '无插件'}
                  </span>
                </div>
                <p className="mt-1.5 text-sm leading-relaxed text-ink-muted">{tpl.description}</p>
                {tpl.plugins.length > 0 && (
                  <div className="mt-2 flex flex-wrap gap-1">
                    {tpl.plugins.map((id) => (
                      <Chip key={id}>{plugins.find((p) => p.id === id)?.name ?? id}</Chip>
                    ))}
                  </div>
                )}
              </button>
            )
          })}
        </div>
      </Field>

      <Field label="端口">
        <div className="flex items-center gap-4 rounded-lg bg-surface px-3 py-2.5 ring-1 ring-inset ring-line">
          <div className="flex-1">
            <div className="text-base text-ink">自动分配端口</div>
            <div className="mt-0.5 text-sm text-ink-faint">
              启动时选择第一个空闲端口，避免多个实例冲突
            </div>
          </div>
          <Switch
            checked={draft.autoPort}
            onChange={(v) => patch({ autoPort: v, port: v ? draft.port : suggestPort() })}
          />
        </div>
        <AnimatePresence initial={false}>
          {!draft.autoPort && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: 'auto', opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              className="overflow-hidden"
            >
              <div className="pt-2">
                <Input
                  type="number"
                  value={draft.port}
                  min={1024}
                  max={65535}
                  onChange={(e) => patch({ port: Number(e.target.value) })}
                  prefix="localhost:"
                  className="w-[220px]"
                />
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </Field>
    </div>
  )
}

function ReviewStep() {
  const draft = useWizardStore((s) => s.draft)
  const version = useCatalogStore((s) => s.versions.find((v) => v.id === draft.versionId))
  const runtime = useCatalogStore((s) => s.runtimes.find((r) => r.id === draft.runtimeId))
  const template = useCatalogStore((s) => s.templates.find((t) => t.id === draft.templateId))
  const suggestPort = useInstanceStore((s) => s.suggestPort)

  const root = `${PHL_ROOT}\\instances\\${slugify(draft.name || 'instance')}`
  const port = draft.autoPort ? suggestPort() : draft.port

  const rows: [string, ReactNode][] = [
    ['名称', draft.name],
    ['DSH 版本', version?.name ?? '—'],
    ['Runtime', `${runtime?.name ?? '—'}${runtime ? ` (v${runtime.version})` : ''}`],
    ['模板', template?.name ?? '—'],
    ['端口', `:${port}${draft.autoPort ? ' (自动)' : ''}`],
  ]

  const paths = [
    ['DSH_HOME', `${root}\\dsh-home`],
    ['Plugins', `${root}\\dsh-home\\plugins`],
    ['Workspace', `${root}\\workspace`],
  ]

  const needsInstall =
    (version && version.state.kind !== 'installed') || (runtime && runtime.state.kind !== 'installed')

  return (
    <div className="space-y-4">
      <div className="rounded-lg bg-surface p-4 ring-1 ring-inset ring-line">
        <div className="flex items-center gap-3">
          <InstanceTile name={draft.name || '新实例'} hue={draft.hue} size={40} />
          <div className="min-w-0">
            <div className="text-md font-medium text-ink">{draft.name || '未命名实例'}</div>
            <div className="mt-0.5 truncate text-sm text-ink-muted">
              {draft.note || '没有备注'}
            </div>
          </div>
        </div>

        <dl className="mt-4 grid gap-x-8 gap-y-1 sm:grid-cols-2">
          {rows.map(([label, value]) => (
            <div key={label} className="flex items-baseline gap-3 py-[3px]">
              <dt className="w-[76px] shrink-0 text-sm text-ink-faint">{label}</dt>
              <dd className="min-w-0 flex-1 truncate text-base text-ink">{value}</dd>
            </div>
          ))}
        </dl>
      </div>

      <div className="rounded-lg bg-surface-sunken p-4 ring-1 ring-inset ring-line">
        <div className="mb-2 flex items-center gap-1.5 text-sm font-medium text-ink-muted">
          <CircleDashed size={13} />
          将要创建的隔离目录
        </div>
        <ul className="space-y-1">
          {paths.map(([label, path]) => (
            <li key={label} className="flex items-baseline gap-3">
              <span className="w-[76px] shrink-0 text-sm text-ink-faint">{label}</span>
              <span className="min-w-0 flex-1 break-all font-mono text-sm text-ink-muted">
                {path}
              </span>
            </li>
          ))}
        </ul>
      </div>

      {needsInstall && (
        <Notice tone="warn" title="所选组件尚未安装">
          实例仍然可以创建，但首次启动前需要先完成下载。PHL 会在启动失败时明确告诉你缺少哪一项。
        </Notice>
      )}
    </div>
  )
}

const STEP_BODIES: Record<WizardStep, ComponentType> = {
  basics: BasicsStep,
  version: VersionStep,
  runtime: RuntimeStep,
  template: TemplateStep,
  review: ReviewStep,
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

function CreatingOverlay() {
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
                      active ? 'text-ink' : done ? 'text-ink-muted' : 'text-ink-faint/60',
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

export function CreateInstancePage({ cloneFrom }: { cloneFrom?: string }) {
  const step = useWizardStore((s) => s.step)
  const draft = useWizardStore((s) => s.draft)
  const next = useWizardStore((s) => s.next)
  const prev = useWizardStore((s) => s.prev)
  const reset = useWizardStore((s) => s.reset)
  const instances = useInstanceStore((s) => s.instances)
  const createInstance = useInstanceStore((s) => s.createInstance)
  const creating = useInstanceStore((s) => s.createProgress !== null)
  const versions = useCatalogStore((s) => s.versions)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const navigate = useUIStore((s) => s.navigate)
  const back = useUIStore((s) => s.back)
  const { t, scale } = useMotion()

  // Seed sensible defaults once the catalog is available: the newest installed
  // version, and a runtime it actually supports.
  useEffect(() => {
    if (draft.versionId || !versions.length) return
    const source = cloneFrom ? instances.find((i) => i.id === cloneFrom) : undefined
    const version =
      (source && versions.find((v) => v.id === source.versionId)) ??
      versions.find((v) => v.state.kind === 'installed' && !v.legacy) ??
      versions[0]
    const runtime =
      (source && runtimes.find((r) => r.id === source.runtimeId)) ??
      runtimes.find((r) => r.state.kind === 'installed' && version.requiresNode.includes(r.major)) ??
      runtimes.find((r) => r.state.kind === 'installed')
    useWizardStore.getState().patch({
      versionId: version?.id ?? null,
      runtimeId: runtime?.id ?? null,
      ...(source ? { name: `${source.name} Copy`, kind: source.kind, hue: source.hue } : {}),
    })
  }, [versions, runtimes, draft.versionId, cloneFrom, instances])

  const names = useMemo(() => instances.map((i) => i.name), [instances])
  const status = stepStatus(step, draft, names)
  const index = WIZARD_STEPS.findIndex((s) => s.id === step)
  const isLast = index === WIZARD_STEPS.length - 1
  const Body = STEP_BODIES[step]

  const allValid = WIZARD_STEPS.every((s) => stepStatus(s.id, draft, names).ok)

  const submit = async () => {
    const instance = await createInstance(draft)
    if (instance) {
      reset()
      navigate({ name: 'instance', id: instance.id })
    }
  }

  return (
    <div className="relative h-full">
      <PageShell
        maxWidth={760}
        title={WIZARD_STEPS[index].label}
        subtitle={
          {
            basics: '给实例起一个能一眼认出的名字。',
            version: '选择这个实例要固定使用的 DSH 版本。',
            runtime: '选择运行 DSH 的 Node Runtime。',
            template: '选择初始插件组合，并决定端口分配方式。',
            review: '确认配置，PHL 会为它建立完全独立的目录。',
          }[step]
        }
      >
        <div className="relative">
          <AnimatePresence mode="wait" initial={false}>
            <motion.div
              key={step}
              initial={{ opacity: 0, x: scale === 0 ? 0 : 16 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: scale === 0 ? 0 : -12 }}
              transition={t(0.22)}
            >
              <Body />
            </motion.div>
          </AnimatePresence>
        </div>

        <div className="mt-6 flex items-center gap-2 border-t border-line pt-4">
          <Button
            variant="ghost"
            onClick={() => {
              reset()
              back()
            }}
          >
            取消
          </Button>
          <div className="ml-auto flex items-center gap-2">
            {!status.ok && <span className="mr-1 text-sm text-danger">{status.error}</span>}
            {index > 0 && (
              <Button variant="secondary" onClick={prev}>
                <ChevronLeft size={13} />
                上一步
              </Button>
            )}
            {isLast ? (
              <Button
                variant="primary"
                size="lg"
                className="group/sheen min-w-[112px]"
                sheen
                disabled={!allValid || creating}
                loading={creating}
                onClick={submit}
              >
                创建实例
              </Button>
            ) : (
              <Button variant="primary" disabled={!status.ok} onClick={next}>
                下一步
                <ChevronRight size={13} />
              </Button>
            )}
          </div>
        </div>
      </PageShell>

      <CreatingOverlay />
    </div>
  )
}
