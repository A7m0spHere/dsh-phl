/** One market row: catalog entry + install state for the selected instance. */
import { Fragment, memo } from 'react'
import { ArrowUp, Check, Download, Star, X } from 'lucide-react'
import { formatCount, formatRelative } from '@/lib/format'
import { useCatalogStore, useUIStore, pluginKey } from '@/stores'
import { PLUGIN_CATEGORY_LABELS, type Plugin, type PluginCategory } from '@/types'
import { Badge, Button, Card } from '@/components/ui'
import {
  SourceBadge,
  PluginAvatar,
  AuthorBadge,
  TransferInline,
} from './visuals'


export const RegistryCard = memo(function RegistryCard({
  plugin,
  instanceId,
  instanceVersionId,
  dshVersionName,
  installedVersion,
  onSelect,
}: {
  plugin: Plugin
  /** Undefined while no instance exists — the row stays browsable. */
  instanceId?: string
  instanceVersionId?: string
  dshVersionName?: string
  installedVersion?: string
  onSelect: (id: string) => void
}) {
  const install = useCatalogStore((s) => s.installPlugin)
  const transfer = useCatalogStore((s) =>
    instanceId ? s.pluginTransfers[pluginKey(instanceId, plugin.id)] : undefined,
  )

  const newest = plugin.releases[0]
  let compat: 'ok' | 'bad' | 'unknown' = 'unknown'
  if (newest && instanceVersionId) {
    if (newest.compatible.includes(instanceVersionId)) compat = 'ok'
    else if (newest.incompatible?.includes(instanceVersionId)) compat = 'bad'
  }

  const subtitle =
    plugin.source.kind === 'npm'
      ? plugin.source.pkg
      : (plugin.repoUrl?.split('/').pop() ?? plugin.repoUrl)

  const onInstall = () => {
    if (!instanceId) {
      useUIStore.getState().toast({
        kind: 'info',
        title: '先创建一个实例',
        message: '插件安装在实例内部；创建后即可一键安装。',
        action: {
          label: '新建实例',
          run: () => useUIStore.getState().push({ name: 'create' }),
        },
      })
      return
    }
    void install(instanceId, plugin.id)
  }

  return (
    <Card
      interactive
      className="group/row px-3.5 py-3 [content-visibility:auto] [contain-intrinsic-size:auto_136px]"
    >
      <div className="flex items-start gap-3">
        <div className="flex min-w-0 flex-1 items-start gap-3 text-left">
          <PluginAvatar plugin={plugin} />
          <div className="min-w-0 flex-1">
            <button className="block w-full text-left" onClick={() => onSelect(plugin.id)}>
              <span className="flex min-w-0 flex-wrap items-baseline gap-x-1.5">
                <span className="truncate text-base font-medium text-ink transition-colors group-hover/row:text-accent-ink">
                  {plugin.name}
                </span>
                {subtitle && <span className="truncate text-sm text-ink-faint">｜ {subtitle}</span>}
                {plugin.official && <Badge tone="accent">官方</Badge>}
              </span>
              <span className="mt-1 flex min-w-0 items-center gap-1.5">
                <Badge tone="outline">
                  {PLUGIN_CATEGORY_LABELS[plugin.category as PluginCategory] ?? plugin.category}
                </Badge>
                <span className="min-w-0 flex-1 truncate text-sm text-ink-muted">{plugin.summary}</span>
              </span>
            </button>
            <div className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-ink-faint">
              {/* Badges carry their own outline, so they stay ungrouped; the
                  plain metrics after them read as one `·`-separated run,
                  matching the version and installed rows. */}
              <AuthorBadge plugin={plugin} />
              {!instanceId ? (
                <Badge tone="neutral">创建实例后可安装</Badge>
              ) : compat === 'ok' ? (
                <Badge tone="ok" icon={<Check size={9} />}>
                  兼容 {dshVersionName}
                </Badge>
              ) : compat === 'bad' ? (
                <Badge tone="danger" icon={<X size={9} />}>
                  不兼容 {dshVersionName}
                </Badge>
              ) : (
                <Badge tone="warn">未验证</Badge>
              )}
              <SourceBadge plugin={plugin} />
              {[
                plugin.downloads > 0 ? (
                  <span key="downloads" className="inline-flex items-center gap-1">
                    <Download size={11} />
                    {formatCount(plugin.downloads)}
                  </span>
                ) : null,
                plugin.stars !== undefined ? (
                  <span key="stars" className="inline-flex items-center gap-1">
                    <Star size={11} className="text-warn" />
                    {formatCount(plugin.stars)}
                  </span>
                ) : null,
                plugin.addedAt ? (
                  <span key="added" className="inline-flex items-center gap-1">
                    <ArrowUp size={11} />
                    {formatRelative(plugin.addedAt)}
                  </span>
                ) : null,
              ]
                .filter(Boolean)
                .map((item, i) => (
                  <Fragment key={`meta-${i}`}>
                    {i > 0 && <span className="text-ink-faint/50">·</span>}
                    {item}
                  </Fragment>
                ))}
            </div>
          </div>
        </div>
        {/* `Card interactive` puts a pointer cursor on the whole surface,
            but only the title button opens the detail view — the action
            column must not claim an affordance it does not have. */}
        <div className="flex shrink-0 cursor-default flex-col items-end gap-1.5 pt-0.5">
          {installedVersion && (
            <span className="font-mono text-xs text-ink-faint">{installedVersion}</span>
          )}
          {installedVersion ? (
            <Badge tone="neutral">已安装</Badge>
          ) : transfer ? (
            <Badge tone="warn">安装中…</Badge>
          ) : (
            <Button
              size="sm"
              variant={compat === 'bad' ? 'secondary' : 'primary'}
              onClick={onInstall}
            >
              安装
            </Button>
          )}
        </div>
      </div>
      {instanceId && <TransferInline instanceId={instanceId} pluginId={plugin.id} />}
    </Card>
  )
})
