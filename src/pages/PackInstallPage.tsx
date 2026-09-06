import { useEffect } from 'react'
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  Package,
  ShieldCheck,
  FolderSearch,
} from 'lucide-react'
import { usePackStore, type PackStep } from '@/stores/packStore'
import { useUIStore } from '@/stores'
import { isDesktop as isDesktopFlag } from '@/lib/desktopCore'
import { Badge, Button, Card, EmptyState, Input, ProgressBar, SectionCard, Skeleton } from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelGroup, PanelShell } from '@/components/layout/Panel'

const STEPS: { key: PackStep; label: string }[] = [
  { key: 'pick', label: '选择整合包' },
  { key: 'preview', label: '预览确认' },
  { key: 'progress', label: '安装' },
]

export function PackInstallPanel() {
  const step = usePackStore((s) => s.step)
  const index = STEPS.findIndex((st) => st.key === step)
  return (
    <PanelShell>
      <PanelGroup title="安装步骤">
        <div className="mt-1 flex flex-col gap-0.5">
          {STEPS.map((st, i) => {
            const done = i < index
            const active = i === index
            return (
              <div
                key={st.key}
                className={`flex items-center gap-2.5 rounded-sm px-1.5 py-1.5 text-base ${
                  active ? 'text-ink' : 'text-ink-muted'
                }`}
              >
                <span
                  className={`flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-full text-[11px] ring-1 ${
                    done
                      ? 'bg-ok/15 text-ok ring-ok/40'
                      : active
                        ? 'bg-accent/10 text-accent ring-accent/40'
                        : 'bg-surface text-ink-faint ring-line-strong'
                  }`}
                >
                  {done ? '✓' : i + 1}
                </span>
                {st.label}
              </div>
            )
          })}
        </div>
      </PanelGroup>
      <div className="p-3 text-sm text-ink-faint">
        整合包是一台机器给另一台机器的完整环境包：插件、配置、可选历史对话。凭据不会随包携带。
      </div>
    </PanelShell>
  )
}

export function PackInstallPage() {
  const reset = usePackStore((s) => s.reset)
  const push = useUIStore((s) => s.push)
  useEffect(() => () => reset(), [reset])

  return (
    <PageShell
      title="安装整合包"
      subtitle="选择一个 .phlpack 文件，PHL 会校验它、检查依赖，然后把它装成一个可启动的实例。"
      actions={
        <Button variant="ghost" onClick={() => push({ name: 'instances' })}>
          <ArrowLeft size={13} />
          返回
        </Button>
      }
    >
      {!isDesktopFlag ? (
        <EmptyState
          icon={<Package size={20} />}
          title="桌面端功能"
          description="安装整合包需要桌面版 PHL 读写本地文件。浏览器模式（npm run dev）下此流程不可用。"
        />
      ) : (
        <StepSwitch />
      )}
    </PageShell>
  )
}

function StepSwitch() {
  const step = usePackStore((s) => s.step)
  if (step === 'pick') return <PickStep />
  if (step === 'preview') return <PreviewStep />
  return <ProgressStep />
}

/* -------------------------------- pick -------------------------------- */

function PickStep() {
  const pickPack = usePackStore((s) => s.pickPack)
  const previewState = usePackStore((s) => s.previewState)
  return (
    <EmptyState
      icon={<FolderSearch size={20} />}
      title="选择一个整合包"
      description="点击右侧按钮，选择本地的 .phlpack 文件。PHL 会先完整校验它（格式、路径安全、文件校验值），再展示要安装的依赖。"
      action={
        <Button variant="primary" onClick={() => void pickPack()} disabled={previewState === 'loading'}>
          {previewState === 'loading' ? '校验中…' : '选择 .phlpack 文件'}
        </Button>
      }
    />
  )
}

/* ------------------------------- preview ------------------------------ */

function PreviewStep() {
  const preview = usePackStore((s) => s.preview)
  const previewState = usePackStore((s) => s.previewState)
  const previewError = usePackStore((s) => s.previewError)
  const name = usePackStore((s) => s.name)
  const setName = usePackStore((s) => s.setName)
  const back = usePackStore((s) => s.back)
  const install = usePackStore((s) => s.install)

  if (previewState === 'loading') return <Skeleton className="h-[220px]" />
  if (previewState === 'error') {
    return (
      <Card tone="danger" className="p-4">
        <p className="text-sm text-ink">整合包校验失败：{previewError}</p>
        <Button variant="secondary" className="mt-3" onClick={back}>
          返回重选
        </Button>
      </Card>
    )
  }
  if (!preview) return null

  const statusBadge = (kind: string) => {
    const dep = preview.dependencies.find((d) => d.kind === kind)
    if (!dep) return null
    const tone =
      dep.status === 'installed' || dep.status === 'embedded'
        ? 'ok'
        : dep.status === 'downloadable'
          ? 'warn'
          : 'danger'
    const text =
      dep.status === 'installed'
        ? '已安装'
        : dep.status === 'embedded'
          ? '包内自带'
          : dep.status === 'downloadable'
            ? '可下载'
            : '缺失'
    return (
      <Badge tone={tone}>
        {dep.id} · {text}
      </Badge>
    )
  }

  return (
    <div className="flex flex-col gap-3">
      <SectionCard title={preview.name} description={preview.description || undefined}>
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <span className="text-ink-muted">作者 {preview.author || '未知'}</span>
          <span className="text-ink-muted">版本 {preview.version}</span>
          {statusBadge('dsh')}
          {statusBadge('runtime')}
        </div>
        <div className="mt-2 flex flex-wrap gap-3 text-sm text-ink-muted">
          <span>插件 {preview.pluginCount}（其中包内自带 {preview.embeddedPluginCount}）</span>
          <span>
            历史对话：{preview.sessionsIncluded ? `${preview.sessionCount} 条` : '不包含'}
          </span>
          {preview.secretsExcluded && (
            <span className="inline-flex items-center gap-1 text-ok">
              <ShieldCheck size={12} /> 凭据已排除
            </span>
          )}
        </div>
      </SectionCard>

      {preview.dependencies.some((d) => d.kind === 'plugin') && (
        <SectionCard title="插件依赖" description="缺失的必需插件会阻止安装。">
          <div className="flex flex-col gap-1.5 text-sm">
            {preview.dependencies
              .filter((d) => d.kind === 'plugin')
              .map((d) => (
                <div key={d.id} className="flex items-center justify-between gap-2">
                  <span className="min-w-0 truncate font-mono text-ink">{d.id}</span>
                  <span className={d.status === 'missing' ? 'text-danger' : 'text-ink-faint'}>
                    {d.status === 'embedded'
                      ? '包内自带'
                      : d.status === 'downloadable'
                        ? '需在线安装'
                        : '缺失'}
                    {d.required && d.status === 'missing' ? '（必需）' : ''}
                  </span>
                </div>
              ))}
          </div>
        </SectionCard>
      )}

      {preview.warnings.length > 0 && (
        <Card className="p-3">
          <ul className="flex flex-col gap-1 text-xs text-ink-muted">
            {preview.warnings.map((w, i) => (
              <li key={i} className="flex items-start gap-1.5">
                <AlertTriangle size={12} className="mt-0.5 shrink-0 text-warn" />
                {w}
              </li>
            ))}
          </ul>
        </Card>
      )}

      <SectionCard title="实例名称" description="安装后在 PHL 里显示的名字。">
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={preview.name} />
      </SectionCard>

      <div className="flex justify-between">
        <Button variant="ghost" onClick={back}>
          <ArrowLeft size={13} />
          重选
        </Button>
        <Button variant="primary" onClick={() => void install()}>
          <Check size={13} />
          安装为实例
        </Button>
      </div>
    </div>
  )
}

/* ------------------------------- progress ----------------------------- */

function ProgressStep() {
  const installing = usePackStore((s) => s.installing)
  const installError = usePackStore((s) => s.installError)
  const back = usePackStore((s) => s.back)
  const push = useUIStore((s) => s.push)
  const reset = usePackStore((s) => s.reset)

  // Success (not installing, no error) → the store already admitted + reloaded;
  // return to the instance list.
  useEffect(() => {
    if (!installing && !installError) {
      const t = setTimeout(() => {
        reset()
        push({ name: 'instances' })
      }, 600)
      return () => clearTimeout(t)
    }
  }, [installing, installError, reset, push])

  if (installError) {
    return (
      <Card tone="danger" className="p-4">
        <p className="text-sm text-ink">安装失败：{installError}</p>
        <p className="mt-1 text-xs text-ink-faint">已回滚，未留下半个实例。</p>
        <Button variant="secondary" className="mt-3" onClick={back}>
          返回修改
        </Button>
      </Card>
    )
  }
  return (
    <Card className="p-5">
      <h3 className="text-sm font-semibold text-ink">正在安装整合包…</h3>
      <p className="mt-1 text-xs text-ink-muted">校验、解包、写清单，成功后原子落位。</p>
      <div className="mt-4">
        <ProgressBar value={installing ? 0.6 : 1} active={installing} />
      </div>
    </Card>
  )
}
