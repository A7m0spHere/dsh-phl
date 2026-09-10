import { useEffect, useMemo, useRef, useState } from 'react'
import { motion, useAnimation } from 'motion/react'
import { EASE, useMotion } from '@/lib/motion'
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  Download,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { isVersionInstallable, isRuntimeInstallable } from '@/types'
import {
  draftIssues,
  useCatalogStore,
  useInstanceStore,
  useUIStore,
  useWizardStore,
  type DraftIssues,
} from '@/stores'
import {
  Button,
} from '@/components/ui'
import {
  type AnimControls,
  VersionSection,
  RuntimeSection,
  SettingsSection,
  MoreSettings,
  PathPreview,
  CreatingOverlay,
} from '@/features/create/sections'
import { PageShell } from '@/components/layout/Page'
import { PanelGroup, PanelShell } from '@/components/layout/Panel'

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

const ISSUE_LABELS: Record<keyof DraftIssues, string> = {
  name: '名称',
  version: '版本',
  runtime: 'Runtime',
  port: '端口',
}

export function CreateInstancePage({ cloneFrom }: { cloneFrom?: string }) {
  const draft = useWizardStore((s) => s.draft)
  const reset = useWizardStore((s) => s.reset)
  const instances = useInstanceStore((s) => s.instances)
  const createInstance = useInstanceStore((s) => s.createInstance)
  const creating = useInstanceStore((s) => s.createProgress !== null)
  const versions = useCatalogStore((s) => s.versions)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const navigate = useUIStore((s) => s.navigate)
  const back = useUIStore((s) => s.back)
  const { t, scale, stagger, riseItem } = useMotion()

  const [attempted, setAttempted] = useState(false)
  const nameInputRef = useRef<HTMLInputElement>(null)
  const nameControls = useAnimation()
  const versionControls = useAnimation()
  const runtimeControls = useAnimation()

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
  const issues = useMemo(() => draftIssues(draft, names), [draft, names])
  const issueKeys = ['name', 'version', 'runtime', 'port'] as const
  const hasIssues = issueKeys.some((k) => issues[k])

  // Components still missing a download, shown in the footer so the
  // auto-install on submit never comes as a surprise.
  const pendingNames = useMemo(() => {
    const out: string[] = []
    const v = versions.find((x) => x.id === draft.versionId)
    if (v && isVersionInstallable(v)) out.push(`DSH ${v.name}`)
    const r = runtimes.find((x) => x.id === draft.runtimeId)
    if (r && isRuntimeInstallable(r)) out.push(r.name)
    return out
  }, [versions, runtimes, draft.versionId, draft.runtimeId])

  useEffect(() => {
    if (!hasIssues) setAttempted(false)
  }, [hasIssues])

  const shake = (controls: AnimControls) => {
    void controls.start({ x: [0, -5, 5, -3, 3, 0], transition: t(0.38, 0, EASE) })
  }

  const focusSection = (id: (typeof issueKeys)[number]) => {
    document
      .getElementById(`create-section-${id}`)
      ?.scrollIntoView({ behavior: scale === 0 ? 'auto' : 'smooth', block: 'start' })
    if (id === 'name') nameInputRef.current?.focus()
  }

  const submit = () => {
    if (hasIssues) {
      setAttempted(true)
      const first = issueKeys.find((k) => issues[k])
      if (first) {
        focusSection(first)
        if (first === 'name') shake(nameControls)
        else if (first === 'version') shake(versionControls)
        else if (first === 'runtime') shake(runtimeControls)
      }
      return
    }
    void doCreate()
  }

  const doCreate = async () => {
    const catalog = useCatalogStore.getState()
    const version = catalog.versions.find((v) => v.id === draft.versionId)
    const runtime = catalog.runtimes.find((r) => r.id === draft.runtimeId)
    const versionPending = version && isVersionInstallable(version) ? version : null
    const runtimePending = runtime && isRuntimeInstallable(runtime) ? runtime : null

    const instance = await createInstance(draft)
    if (!instance) return
    reset()
    navigate({ name: 'instance', id: instance.id })

    if (!versionPending && !runtimePending) return
    const labels = [
      versionPending ? `DSH ${versionPending.name}` : null,
      runtimePending?.name ?? null,
    ].filter(Boolean)
    useUIStore.getState().toast({
      kind: 'info',
      title: '正在补齐缺失组件',
      message: `${labels.join('、')} 正在后台下载，完成后即可启动实例。`,
    })
    // Chained, not parallel: two npm/nodejs downloads racing split bandwidth.
    void (async () => {
      if (versionPending) await catalog.installVersion(versionPending.id)
      if (runtimePending) await catalog.installRuntime(runtimePending.id)
    })()
  }

  const missing = issueKeys.filter((k) => issues[k]).map((k) => ISSUE_LABELS[k])

  return (
    <div className="relative h-full">
      <PageShell
        maxWidth={960}
        title="创建实例"
        subtitle="所有选择都在这一页完成，确认无误后统一创建。"
      >
        <motion.div
          variants={stagger(0.06, 0.02)}
          initial="hidden"
          animate="show"
          className="flex flex-col items-start gap-3 lg:flex-row"
        >
          <motion.div variants={riseItem} className="w-full min-w-0 flex-1 space-y-3">
            <VersionSection
              issue={issues.version}
              attempted={attempted}
              controls={versionControls}
            />
            <RuntimeSection
              issue={issues.runtime}
              attempted={attempted}
              controls={runtimeControls}
            />
          </motion.div>

          <motion.div
            variants={riseItem}
            className="w-full shrink-0 space-y-3 lg:sticky lg:top-0 lg:w-[300px]"
          >
            <SettingsSection
              nameIssue={issues.name}
              attempted={attempted}
              controls={nameControls}
              nameInputRef={nameInputRef}
            />
            <MoreSettings portIssue={issues.port} attempted={attempted} />
            <PathPreview />
          </motion.div>
        </motion.div>

        <div className="sticky bottom-3 z-10 mt-4">
          <div className="flex items-center gap-3 rounded-lg bg-surface-raised px-3.5 py-2.5 shadow-pop ring-1 ring-inset ring-line">
            {missing.length ? (
              <span className="flex min-w-0 items-center gap-1.5 text-sm text-warn">
                <AlertTriangle size={13} className="shrink-0" />
                <span className="truncate">还需选择：{missing.join('、')}</span>
              </span>
            ) : pendingNames.length ? (
              <span className="flex min-w-0 items-center gap-1.5 text-sm text-info">
                <Download size={13} className="shrink-0" />
                <span className="truncate">创建后将自动下载：{pendingNames.join('、')}</span>
              </span>
            ) : (
              <span className="flex items-center gap-1.5 text-sm text-ok">
                <Check size={13} />
                配置就绪，随时可以创建
              </span>
            )}

            <div className="ml-auto flex shrink-0 items-center gap-2">
              <Button
                variant="ghost"
                onClick={() => {
                  reset()
                  back()
                }}
              >
                取消
              </Button>
              <Button
                variant="primary"
                size="lg"
                className="group/sheen min-w-[112px]"
                sheen
                loading={creating}
                onClick={submit}
              >
                创建实例
              </Button>
            </div>
          </div>
        </div>
      </PageShell>

      <CreatingOverlay />
    </div>
  )
}
