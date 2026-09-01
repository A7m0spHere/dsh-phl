import { useMemo } from 'react'
import { motion } from 'motion/react'
import {
  ArrowUpCircle,
  Blocks,
  Check,
  Link2,
  Package2,
  Search,
  Trash2,
  X,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { formatCount } from '@/lib/format'
import { useMotion } from '@/lib/motion'
import { linkedPluginNames } from '@/data/instances'
import {
  useCatalogStore,
  useInstanceStore,
  useUIStore,
  useViewStore,
  type PluginTab,
} from '@/stores'
import { latestRelease, type Plugin, type PluginCategory } from '@/types'
import { Badge, Button, Chip, EmptyState, Input, Notice, Switch, Tooltip } from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelDivider, PanelGroup, PanelItem, PanelShell } from '@/components/layout/Panel'
import { InstanceTile } from '@/components/instance'

const CATEGORY_LABEL: Record<PluginCategory, string> = {
  routing: '路由',
  tooling: '工具',
  provider: '模型接入',
  ui: '界面',
  workflow: '工作流',
  diagnostics: '诊断',
}

/* ------------------------------------------------------------------ *
 * panel — scope first: a plugin is always "installed into an instance"
 * ------------------------------------------------------------------ */

export function PluginsPanel() {
  const instances = useInstanceStore((s) => s.instances)
  const states = useInstanceStore((s) => s.states)
  const scope = useViewStore((s) => s.pluginScope)
  const setScope = useViewStore((s) => s.setPluginScope)
  const tab = useViewStore((s) => s.pluginTab)
  const setTab = useViewStore((s) => s.setPluginTab)
  const plugins = useCatalogStore((s) => s.plugins)
  const query = useViewStore((s) => s.pluginQuery)
  const setQuery = useViewStore((s) => s.setPluginQuery)

  const instance = instances.find((i) => i.id === scope)
  const updatable = useMemo(() => {
    if (!instance) return 0
    return instance.plugins.filter((ip) => {
      const meta = plugins.find((p) => p.id === ip.pluginId)
      return meta && !ip.linked && latestRelease(meta).version !== ip.version
    }).length
  }, [instance, plugins])

  const tabs: { id: PluginTab; label: string; count?: number }[] = [
    { id: 'installed', label: '已安装', count: instance?.plugins.length },
    { id: 'registry', label: '插件库', count: plugins.length },
    { id: 'updates', label: '可更新', count: updatable },
  ]

  return (
    <PanelShell>
      <PanelGroup title="作用实例">
        {instances.map((i) => (
          <PanelItem
            key={i.id}
            groupId="plugin-scope"
            active={scope === i.id}
            label={i.name}
            icon={
              <span
                className={cn(
                  'block h-[7px] w-[7px] rounded-full',
                  states[i.id]?.status === 'running' ? 'bg-ok' : 'bg-ink-faint/40',
                )}
              />
            }
            count={i.plugins.length}
            onClick={() => setScope(i.id)}
          />
        ))}
      </PanelGroup>

      <PanelDivider />

      <PanelGroup title="视图">
        {tabs.map((tb) => (
          <PanelItem
            key={tb.id}
            groupId="plugin-tab"
            active={tab === tb.id}
            label={tb.label}
            count={tb.count}
            onClick={() => setTab(tb.id)}
          />
        ))}
      </PanelGroup>

      <PanelDivider />

      <div className="p-2.5">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索插件"
          prefix={<Search size={13} />}
        />
      </div>

      <div className="mt-auto p-3">
        <div className="rounded-lg bg-surface-sunken p-3 text-sm leading-relaxed text-ink-faint ring-1 ring-inset ring-line">
          插件安装在实例内部而不是全局。同一个插件的不同版本可以同时存在于不同实例中。
        </div>
      </div>
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ *
 * page
 * ------------------------------------------------------------------ */

export function PluginsPage() {
  const instances = useInstanceStore((s) => s.instances)
  const update = useInstanceStore((s) => s.updateInstance)
  const plugins = useCatalogStore((s) => s.plugins)
  const versions = useCatalogStore((s) => s.versions)
  const scope = useViewStore((s) => s.pluginScope)
  const setScope = useViewStore((s) => s.setPluginScope)
  const tab = useViewStore((s) => s.pluginTab)
  const setTab = useViewStore((s) => s.setPluginTab)
  const query = useViewStore((s) => s.pluginQuery)
  const toast = useUIStore((s) => s.toast)
  const navigate = useUIStore((s) => s.navigate)
  const { stagger, riseItem } = useMotion()

  const instance = instances.find((i) => i.id === scope) ?? instances[0]
  const dshVersion = versions.find((v) => v.id === instance?.versionId)

  const compatibility = (plugin: Plugin, version: string) => {
    const release = plugin.releases.find((r) => r.version === version)
    if (!release || !instance) return 'unknown' as const
    if (release.compatible.includes(instance.versionId)) return 'ok' as const
    if (release.incompatible?.includes(instance.versionId)) return 'bad' as const
    return 'unknown' as const
  }

  const installed = useMemo(() => {
    if (!instance) return []
    const q = query.trim().toLowerCase()
    return instance.plugins
      .map((ip) => {
        const meta = plugins.find((p) => p.id === ip.pluginId)
        const newest = meta ? latestRelease(meta) : undefined
        return {
          ...ip,
          meta,
          name: meta?.name ?? linkedPluginNames[ip.pluginId] ?? ip.pluginId,
          newest: newest?.version,
          outdated: !!newest && !ip.linked && newest.version !== ip.version,
          compat: meta ? compatibility(meta, ip.version) : ('unknown' as const),
        }
      })
      .filter((p) => !q || p.name.toLowerCase().includes(q))
  }, [instance, plugins, query])

  const registry = useMemo(() => {
    const q = query.trim().toLowerCase()
    return plugins.filter(
      (p) =>
        !q ||
        p.name.toLowerCase().includes(q) ||
        p.summary.toLowerCase().includes(q) ||
        p.author.toLowerCase().includes(q),
    )
  }, [plugins, query])

  if (!instance) {
    return (
      <PageShell title="插件">
        <EmptyState
          icon={<Blocks size={20} />}
          title="还没有实例"
          description="插件必须安装到某个实例里。先创建一个实例，再回来安装插件。"
          action={
            <Button variant="primary" onClick={() => navigate({ name: 'create' })}>
              新建实例
            </Button>
          }
        />
      </PageShell>
    )
  }

  const toggle = (pluginId: string, enabled: boolean) =>
    update(instance.id, {
      plugins: instance.plugins.map((p) => (p.pluginId === pluginId ? { ...p, enabled } : p)),
    })

  const uninstall = (pluginId: string, name: string) => {
    update(instance.id, { plugins: instance.plugins.filter((p) => p.pluginId !== pluginId) })
    toast({ kind: 'info', title: `已从「${instance.name}」移除 ${name}` })
  }

  const install = (plugin: Plugin) => {
    if (instance.plugins.some((p) => p.pluginId === plugin.id)) return
    update(instance.id, {
      plugins: [
        ...instance.plugins,
        { pluginId: plugin.id, version: latestRelease(plugin).version, enabled: true },
      ],
    })
    toast({
      kind: 'success',
      title: `已安装到「${instance.name}」`,
      message: `${plugin.name} ${latestRelease(plugin).version}`,
    })
  }

  const upgrade = (pluginId: string, version: string, name: string) => {
    update(instance.id, {
      plugins: instance.plugins.map((p) => (p.pluginId === pluginId ? { ...p, version } : p)),
    })
    toast({ kind: 'success', title: `${name} 已更新到 ${version}` })
  }

  const upgradable = installed.filter((p) => p.outdated)
  const rows = tab === 'updates' ? upgradable : installed

  return (
    <PageShell
      title="插件"
      subtitle={
        <span className="flex flex-wrap items-center gap-1.5">
          当前作用于
          <span className="font-medium text-ink">{instance.name}</span>
          <Chip>DSH {dshVersion?.name}</Chip>
          <span className="text-ink-faint">插件只影响这一个实例。</span>
        </span>
      }
      actions={
        <div className="flex items-center gap-2">
          <InstanceTile name={instance.name} hue={instance.hue} size={30} />
          <select
            value={scope}
            onChange={(e) => setScope(e.target.value)}
            className="h-8 cursor-pointer rounded bg-surface px-2 pr-6 text-base text-ink ring-1 ring-inset ring-line-strong/60 hover:ring-line-strong focus:outline-none"
          >
            {instances.map((i) => (
              <option key={i.id} value={i.id}>
                {i.name}
              </option>
            ))}
          </select>
        </div>
      }
    >
      {tab !== 'registry' && upgradable.length > 0 && tab === 'installed' && (
        <Notice
          tone="info"
          title={`${upgradable.length} 个插件有新版本`}
          className="mb-4"
          action={
            <Button size="sm" variant="secondary" onClick={() => setTab('updates')}>
              查看
            </Button>
          }
        >
          更新只作用于当前实例，其他实例仍保持原有版本。
        </Notice>
      )}

      {tab === 'registry' ? (
        <motion.div
          variants={stagger(0.03)}
          initial="hidden"
          animate="show"
          className="grid gap-2 lg:grid-cols-2"
        >
          {registry.map((p) => {
            const already = instance.plugins.find((ip) => ip.pluginId === p.id)
            const newest = latestRelease(p)
            const compat = compatibility(p, newest.version)
            return (
              <motion.div
                key={p.id}
                variants={riseItem}
                className="group/plugin rounded-lg bg-surface p-3.5 ring-1 ring-inset ring-line transition-shadow duration-200 hover:shadow-lift hover:ring-line-strong/70"
              >
                <div className="flex items-start gap-3">
                  <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-surface-sunken text-ink-faint">
                    <Package2 size={15} />
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="text-base font-medium text-ink">{p.name}</span>
                      {p.official && <Badge tone="accent">官方</Badge>}
                      <Badge tone="outline">{CATEGORY_LABEL[p.category]}</Badge>
                    </div>
                    <p className="mt-1 text-sm leading-relaxed text-ink-muted">{p.summary}</p>
                    <div className="mt-2 flex items-center gap-2 text-sm text-ink-faint">
                      <span className="font-mono">{newest.version}</span>
                      <span className="text-ink-faint/50">·</span>
                      <span>{p.author}</span>
                      <span className="text-ink-faint/50">·</span>
                      <span>{formatCount(p.downloads)} 次安装</span>
                    </div>
                    <div className="mt-2 flex items-center gap-2">
                      {compat === 'ok' ? (
                        <Badge tone="ok" icon={<Check size={9} />}>
                          兼容 {dshVersion?.name}
                        </Badge>
                      ) : compat === 'bad' ? (
                        <Badge tone="danger" icon={<X size={9} />}>
                          不兼容 {dshVersion?.name}
                        </Badge>
                      ) : (
                        <Badge tone="warn">未在 {dshVersion?.name} 上验证</Badge>
                      )}
                    </div>
                  </div>
                  <div className="shrink-0">
                    {already ? (
                      <Badge tone="neutral">已安装</Badge>
                    ) : (
                      <Button
                        size="sm"
                        variant={compat === 'bad' ? 'secondary' : 'primary'}
                        onClick={() => install(p)}
                      >
                        安装
                      </Button>
                    )}
                  </div>
                </div>
              </motion.div>
            )
          })}
        </motion.div>
      ) : rows.length === 0 ? (
        <EmptyState
          icon={<Blocks size={20} />}
          title={tab === 'updates' ? '所有插件都是最新的' : '这个实例还没有插件'}
          description={
            tab === 'updates'
              ? `「${instance.name}」中的插件都已经是最新版本。`
              : '从插件库里挑一个装进这个实例，它不会影响其他实例。'
          }
          action={
            <Button variant="secondary" onClick={() => setTab('registry')}>
              浏览插件库
            </Button>
          }
        />
      ) : (
        <motion.ul variants={stagger(0.03)} initial="hidden" animate="show" className="space-y-1.5">
          {rows.map((p) => (
            <motion.li
              key={p.pluginId}
              variants={riseItem}
              className="group/row flex items-center gap-3 rounded-lg bg-surface px-3.5 py-2.5 ring-1 ring-inset ring-line transition-shadow duration-200 hover:ring-line-strong/70"
            >
              <span
                className={cn(
                  'flex h-7 w-7 shrink-0 items-center justify-center rounded-lg',
                  p.enabled ? 'bg-ok/10 text-ok' : 'bg-surface-sunken text-ink-faint',
                )}
              >
                {p.linked ? <Link2 size={13} /> : <Package2 size={13} />}
              </span>

              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-1.5">
                  <span className={cn('text-base', p.enabled ? 'text-ink' : 'text-ink-faint')}>
                    {p.name}
                  </span>
                  {p.linked && <Badge tone="accent">本地链接</Badge>}
                  {p.compat === 'bad' && <Badge tone="danger">不兼容当前版本</Badge>}
                  {p.compat === 'unknown' && !p.linked && <Badge tone="warn">兼容性未知</Badge>}
                </div>
                <div className="mt-0.5 flex items-center gap-1.5 text-sm text-ink-faint">
                  <span className="font-mono">{p.version}</span>
                  {p.outdated && (
                    <>
                      <span className="text-ink-faint/50">→</span>
                      <span className="font-mono text-accent-ink">{p.newest}</span>
                    </>
                  )}
                  {p.meta && (
                    <>
                      <span className="text-ink-faint/50">·</span>
                      <span>{p.meta.author}</span>
                    </>
                  )}
                </div>
              </div>

              <div className="flex shrink-0 items-center gap-1.5">
                {p.outdated && p.newest && (
                  <Button
                    size="sm"
                    variant="primary"
                    onClick={() => upgrade(p.pluginId, p.newest!, p.name)}
                  >
                    <ArrowUpCircle size={12} />
                    更新
                  </Button>
                )}
                <Tooltip content={p.enabled ? '停用' : '启用'}>
                  <Switch
                    checked={p.enabled}
                    onChange={(v) => toggle(p.pluginId, v)}
                    label={`启用 ${p.name}`}
                  />
                </Tooltip>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => uninstall(p.pluginId, p.name)}
                  className="opacity-0 transition-opacity group-hover/row:opacity-100 focus:opacity-100"
                >
                  <Trash2 size={12} />
                </Button>
              </div>
            </motion.li>
          ))}
        </motion.ul>
      )}
    </PageShell>
  )
}
