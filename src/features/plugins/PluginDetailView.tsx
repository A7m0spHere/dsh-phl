import { ArrowLeft, Github, Package2 } from 'lucide-react'
import { openExternal } from '@/lib/desktop'
import { formatDate } from '@/lib/format'
import { latestRelease, PLUGIN_CATEGORY_LABELS, type Plugin, type PluginCategory } from '@/types'
import { Badge, Button, Dropdown } from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { InstanceTile } from '@/components/instance'
import { AuthorBadge, PluginAvatar, Popularity, SourceBadge, TransferInline } from './visuals'

/** The props the detail view needs; all state stays in PluginsPage. */
export interface PluginDetailProps {
  detail: Plugin
  instance:
    | { id: string; name: string; hue: number; profile: string; versionId: string }
    | undefined
  instances: { id: string; name: string }[]
  scope: string
  repoShots: string[]
  installedVersion: string | undefined
  installing: boolean
  setDetailId: (id: string | null) => void
  setScope: (id: string) => void
  onInstall: (id: string) => void
  onRequireInstance: (run: (target: { id: string }) => void) => void
  onCreateInstance: () => void
}

export function PluginDetailView(props: PluginDetailProps) {
  const {
    detail,
    instance,
    instances,
    scope,
    repoShots,
    installedVersion,
    installing,
    setDetailId,
    setScope,
    onInstall,
    onRequireInstance,
    onCreateInstance,
  } = props
  const detailNpmPkg = detail.source.kind === 'npm' ? detail.source.pkg : null

  return (
    <PageShell
      title={
        <span className="flex items-center gap-2.5">
          <button
            title="返回插件库"
            aria-label="返回插件库"
            onClick={() => setDetailId(null)}
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-surface text-ink-muted ring-1 ring-inset ring-line transition-colors duration-150 hover:bg-surface-hover hover:text-ink"
          >
            <ArrowLeft size={14} />
          </button>
          <button
            className="text-base text-ink-faint transition-colors duration-150 hover:text-ink"
            onClick={() => setDetailId(null)}
          >
            插件库
          </button>
          <span className="text-base text-ink-faint/50">/</span>
          <span className="max-w-[320px] truncate text-lg font-medium text-ink">{detail.name}</span>
        </span>
      }
    >
      <div className="flex flex-col gap-4 xl:flex-row">
        <div className="min-w-0 flex-1">
          <div className="rounded-lg bg-surface p-4 ring-1 ring-inset ring-line">
            <div className="flex items-start gap-3">
              <PluginAvatar plugin={detail} size={44} />
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-1.5">
                  <h2 className="text-lg font-medium text-ink">{detail.name}</h2>
                  {detail.official && <Badge tone="accent">官方</Badge>}
                  <SourceBadge plugin={detail} />
                  <Badge tone="outline">
                    {PLUGIN_CATEGORY_LABELS[detail.category as PluginCategory] ?? detail.category}
                  </Badge>
                </div>
                <p className="mt-2 text-base leading-relaxed text-ink">{detail.summary}</p>
                {detail.summaryEn && (
                  <p className="mt-1 text-sm leading-relaxed text-ink-faint">{detail.summaryEn}</p>
                )}
                <div className="mt-3 flex flex-wrap items-center gap-2 text-sm text-ink-faint">
                  <AuthorBadge plugin={detail} />
                  <Popularity plugin={detail} />
                  {detail.addedAt && (
                    <>
                      <span className="text-ink-faint/50">·</span>
                      <span>收录于 {formatDate(detail.addedAt)}</span>
                    </>
                  )}
                </div>
                {(detail.repoUrl || detail.source.kind === 'npm') && (
                  <div className="mt-3 flex flex-wrap items-center gap-3">
                    {detail.repoUrl && (
                      <button
                        className="inline-flex items-center gap-1 text-sm text-accent-ink hover:underline"
                        onClick={() => void openExternal(detail.repoUrl!)}
                      >
                        <Github size={12} />
                        仓库主页
                      </button>
                    )}
                    {detailNpmPkg && (
                      <button
                        className="inline-flex items-center gap-1 text-sm text-accent-ink hover:underline"
                        onClick={() =>
                          void openExternal(`https://www.npmjs.com/package/${detailNpmPkg}`)
                        }
                      >
                        <Package2 size={12} />
                        npm 页面
                      </button>
                    )}
                  </div>
                )}
              </div>
            </div>
            {instance && <TransferInline instanceId={instance.id} pluginId={detail.id} />}
          </div>

          {(() => {
            const shots = [
              ...(detail.screenshots ?? []),
              ...repoShots.filter((u) => !(detail.screenshots ?? []).includes(u)),
            ]
            if (shots.length === 0) return null
            return (
              <div className="mt-4">
                <div className="text-sm font-medium text-ink">效果预览</div>
                <div className="mt-2 flex gap-3 overflow-x-auto pb-1">
                  {shots.map((src) => (
                    <button
                      key={src}
                      className="shrink-0 overflow-hidden rounded-lg bg-surface-sunken ring-1 ring-inset ring-line transition-shadow duration-200 hover:shadow-lift"
                      onClick={() => void openExternal(src)}
                      title="点击查看原图"
                    >
                      <img
                        src={src}
                        alt={`${detail.name} 效果图`}
                        loading="lazy"
                        className="h-48 max-w-[420px] object-contain"
                        onError={(e) => {
                          ;(e.currentTarget.parentElement as HTMLElement).style.display = 'none'
                        }}
                      />
                    </button>
                  ))}
                </div>
                {!detail.screenshots?.length && (
                  <div className="mt-1 text-xs text-ink-faint">来自插件仓库的 README</div>
                )}
              </div>
            )
          })()}
        </div>

        <div className="w-full shrink-0 xl:w-72">
          <div className="rounded-lg bg-surface p-4 ring-1 ring-inset ring-line">
            <div className="text-sm font-medium text-ink">安装到</div>
            {instance ? (
              <>
                <div className="mt-2 flex items-center gap-2">
                  <InstanceTile name={instance.name} hue={instance.hue} size={28} />
                  <Dropdown
                    ariaLabel="安装到实例"
                    className="min-w-0 flex-1"
                    value={scope}
                    onChange={setScope}
                    options={instances.map((i) => ({ value: i.id, label: i.name }))}
                  />
                </div>
                <div className="mt-2 text-sm leading-relaxed text-ink-faint">
                  插件只装入这个实例的 profile（{instance.profile}），不影响其他实例。
                </div>
                <div className="mt-3">
                  {installedVersion ? (
                    <Badge tone="neutral">已安装</Badge>
                  ) : installing ? (
                    <Badge tone="warn">正在安装…</Badge>
                  ) : (
                    <Button
                      variant="primary"
                      block
                      onClick={() => onRequireInstance((target) => onInstall(target.id))}
                    >
                      安装 {latestRelease(detail)?.version ?? ''}
                    </Button>
                  )}
                </div>
              </>
            ) : (
              <>
                <div className="mt-2 text-sm leading-relaxed text-ink-faint">
                  插件必须装入某个实例的 profile。创建实例后回来，一键安装到这里。
                </div>
                <Button variant="primary" block className="mt-3" onClick={onCreateInstance}>
                  新建实例
                </Button>
              </>
            )}
          </div>
        </div>
      </div>
    </PageShell>
  )
}
