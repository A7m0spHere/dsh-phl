import { Blocks, RotateCcw, Search } from 'lucide-react'
import { motion } from 'motion/react'
import { formatCount } from '@/lib/format'
import { useMotion } from '@/lib/motion'
import { useCatalogStore } from '@/stores'
import type { PluginCatalogOrigin } from '@/services'
import { PLUGIN_CATEGORY_LABELS, type Plugin, type PluginCategory } from '@/types'
import { Button, Dropdown, EmptyState, Input, Notice, Skeleton } from '@/components/ui'
import { RegistryCard } from './RegistryCard'

/** 把回退链里的 base URL 翻成人话；未知源退化为 hostname。 */
function catalogSourceLabel(servedFrom: string): string {
  if (servedFrom === 'cache') return '本地缓存'
  if (servedFrom === 'https://awesome-dsh-plugin.com') return '官方源'
  if (servedFrom === 'https://dsh-ai.org') return '国内镜像（dsh-ai.org）'
  if (servedFrom === 'https://awesome-dsh-plugin.github.io/awesome-dsh-plugin')
    return 'GitHub Pages 镜像'
  try {
    return new URL(servedFrom).hostname
  } catch {
    return servedFrom
  }
}

export interface PluginRegistryViewProps {
  instance: { id: string; versionId: string } | undefined
  dshVersionName: string | undefined
  pluginsOffline: boolean
  pluginsError: string | null
  pluginsOrigin: PluginCatalogOrigin | null
  catalogLoading: boolean
  catalogCount: number
  query: string
  setQuery: (v: string) => void
  category: string
  setCategory: (v: string) => void
  source: string
  setSource: (v: string) => void
  sort: string
  setSort: (v: string) => void
  categories: string[]
  registryCount: number
  shown: Plugin[]
  onLoadMore: () => void
  onResetFilters: () => void
  installedVersionFor: (pluginId: string) => string | undefined
  onSelect: (id: string) => void
}

/**
 * The plugin-market shell (§M4): provenance/offline notices, the search +
 * filter + sort bar, the paged card grid, load-more, and the no-match empty
 * state. Filtering/sorting/paging are computed by the parent and passed in as
 * props, so this stays a pure view of the current selection.
 */
export function PluginRegistryView(p: PluginRegistryViewProps) {
  const { t, scale } = useMotion()
  const riseShift = scale === 0 ? 0 : 8
  const reload = () => void useCatalogStore.getState().load()
  return (
    <>
      {p.pluginsOffline && (
        <Notice
          tone="warn"
          title="在线插件目录暂时不可用"
          className="mb-4"
          action={
            <Button size="sm" variant="secondary" onClick={reload}>
              重试
            </Button>
          }
        >
          {p.pluginsError || '无法连接任何发布源'}。已安装的插件不受影响，恢复连接后重试即可浏览插件库。
        </Notice>
      )}
      {p.pluginsOrigin?.fromCache && (
        <Notice
          tone="warn"
          title="在线目录全部不可用，当前展示离线缓存"
          className="mb-4"
          action={
            <Button size="sm" variant="secondary" onClick={reload}>
              重试
            </Button>
          }
        >
          这份目录是上次成功获取时存下的
          {p.pluginsOrigin.updated ? `（数据更新于 ${p.pluginsOrigin.updated}）` : ''}，可能缺少新收录或已下架的插件。安装仍会联网取包，不受影响。
        </Notice>
      )}
      {p.pluginsOrigin && !p.pluginsOrigin.fromCache && p.pluginsOrigin.usedFallback && (
        <Notice
          tone="info"
          title={`已切换到备用目录源：${catalogSourceLabel(p.pluginsOrigin.servedFrom)}`}
          className="mb-4"
          action={
            <Button size="sm" variant="secondary" onClick={reload}>
              重试原源
            </Button>
          }
        >
          你配置的下载源暂时不可用，本次目录由备用源提供，更新时效可能稍逊。
        </Notice>
      )}
      {p.catalogLoading && p.catalogCount === 0 ? (
        <div className="space-y-2" aria-hidden>
          {Array.from({ length: 6 }).map((_, i) => (
            <Skeleton key={i} className="h-[92px]" />
          ))}
        </div>
      ) : (
        <>
          <motion.div
            initial={{ opacity: 0, y: riseShift }}
            animate={{ opacity: 1, y: 0 }}
            transition={t(0.24)}
            className="mb-3 rounded-lg bg-surface p-4 ring-1 ring-inset ring-line"
          >
            <div className="text-base font-medium text-ink">搜索插件</div>
            <div className="mt-3 grid items-center gap-x-4 gap-y-2.5 sm:grid-cols-2">
              <label className="flex min-w-0 items-center gap-2.5">
                <span className="w-8 shrink-0 text-sm text-ink-muted">名称</span>
                <Input
                  value={p.query}
                  onChange={(e) => p.setQuery(e.target.value)}
                  placeholder="按名称、简介或作者搜索"
                  prefix={<Search size={13} />}
                  className="min-w-0 flex-1"
                />
              </label>
              <label className="flex min-w-0 items-center gap-2.5">
                <span className="w-8 shrink-0 text-sm text-ink-muted">分类</span>
                <Dropdown
                  ariaLabel="分类"
                  className="min-w-0 flex-1"
                  value={p.category}
                  onChange={p.setCategory}
                  options={[
                    { value: 'all', label: '全部分类' },
                    ...p.categories.map((c) => ({
                      value: c,
                      label: (PLUGIN_CATEGORY_LABELS[c as PluginCategory] ?? c) as string,
                    })),
                  ]}
                />
              </label>
              <label className="flex min-w-0 items-center gap-2.5">
                <span className="w-8 shrink-0 text-sm text-ink-muted">来源</span>
                <Dropdown
                  ariaLabel="来源"
                  className="min-w-0 flex-1"
                  value={p.source}
                  onChange={p.setSource}
                  options={[
                    { value: 'all', label: '全部来源' },
                    { value: 'npm', label: 'npm 包' },
                    { value: 'github', label: 'GitHub 源码' },
                    { value: 'tarball', label: '预构建包' },
                  ]}
                />
              </label>
              <label className="flex min-w-0 items-center gap-2.5">
                <span className="w-8 shrink-0 text-sm text-ink-muted">排序</span>
                <Dropdown
                  ariaLabel="排序"
                  className="min-w-0 flex-1"
                  value={p.sort}
                  onChange={p.setSort}
                  options={[
                    { value: 'recommended', label: '最多安装' },
                    { value: 'stars', label: '最多星标' },
                    { value: 'newest', label: '最近收录' },
                  ]}
                />
              </label>
            </div>
            <div className="mt-3 flex items-center justify-between border-t border-line/70 pt-3">
              <div className="flex min-w-0 items-baseline gap-2 text-sm text-ink-faint">
                <span className="shrink-0">
                  共 {formatCount(p.registryCount)} 个插件
                  {p.registryCount > p.shown.length && `，当前显示 ${p.shown.length} 个`}
                </span>
                {p.pluginsOrigin && !p.pluginsOffline && (
                  <span className="truncate" title={p.pluginsOrigin.servedFrom}>
                    目录来自 {catalogSourceLabel(p.pluginsOrigin.servedFrom)}
                    {p.pluginsOrigin.updated && ` · 更新于 ${p.pluginsOrigin.updated}`}
                  </span>
                )}
              </div>
              <Button size="sm" variant="ghost" onClick={p.onResetFilters}>
                <RotateCcw size={12} />
                重置条件
              </Button>
            </div>
          </motion.div>

          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={t(0.16)}
            className="space-y-2"
          >
            {p.shown.map((plugin) => (
              <RegistryCard
                key={plugin.id}
                plugin={plugin}
                instanceId={p.instance?.id}
                instanceVersionId={p.instance?.versionId}
                dshVersionName={p.dshVersionName}
                installedVersion={p.installedVersionFor(plugin.id)}
                onSelect={p.onSelect}
              />
            ))}
          </motion.div>
          {p.registryCount > p.shown.length && (
            <div className="mt-3 flex justify-center">
              <Button variant="secondary" onClick={p.onLoadMore}>
                加载更多（还有 {formatCount(p.registryCount - p.shown.length)} 个）
              </Button>
            </div>
          )}
          {p.registryCount === 0 && (
            <EmptyState
              icon={<Blocks size={20} />}
              title="没有匹配的插件"
              description="换个关键词，或清除分类筛选。"
            />
          )}
        </>
      )}
    </>
  )
}
