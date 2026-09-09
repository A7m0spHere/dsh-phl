import {
  useEffect,
  useCallback,
  useDeferredValue,
  useMemo,
  useState,
  useTransition,
} from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Blocks, Search, Shuffle } from 'lucide-react'
import { resolveUpdate } from '@/features/plugins/updateState'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'
import { sampleStratified } from '@/lib/sample'
import { fetchRepoScreenshots } from '@/lib/screenshots'
import { linkedPluginNames } from '@/data/instances'
import {
  useCatalogStore,
  useInstanceStore,
  useUIStore,
  useViewStore,
  pluginKey,
  resolvePluginScope,
  type PluginTab,
  type PluginTransferState,
} from '@/stores'
import { useShallow } from 'zustand/react/shallow'
import { latestRelease, type Plugin } from '@/types'
import { Button, Chip, Dropdown, EmptyState, Input } from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelDivider, PanelGroup, PanelItem, PanelShell } from '@/components/layout/Panel'
import { InstanceTile } from '@/components/instance'

import {
  REGISTRY_FIRST_PAINT,
  REGISTRY_PAGE,
  RECOMMEND_COUNT,
  popularity,
} from '@/features/plugins/visuals'
import { PluginDiscovery } from '@/features/plugins/discovery'
import { PluginDetailView } from '@/features/plugins/PluginDetailView'
import { PluginRegistryView } from '@/features/plugins/PluginRegistryView'
import { PluginInstalledView } from '@/features/plugins/PluginInstalledView'

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
  const latestVersions = useCatalogStore((s) => s.latestVersions)
  const query = useViewStore((s) => s.pluginQuery)
  const setQuery = useViewStore((s) => s.setPluginQuery)

  const instance = resolvePluginScope(instances, scope)
  // Write the resolved id back so the scope selector shows what the page is
  // actually acting on, instead of an empty placeholder over a real target.
  useEffect(() => {
    if (instance && instance.id !== scope) setScope(instance.id)
  }, [instance, scope, setScope])

  const updatable = useMemo(() => {
    if (!instance) return 0
    return instance.plugins.filter((ip) => {
      const meta = plugins.find((p) => p.id === ip.pluginId)
      if (!meta || ip.linked) return false
      // Same pure rule the row uses (§M4/R5) — the tab count and the list can
      // never disagree about what counts as updatable.
      return resolveUpdate({
        check: latestVersions[ip.pluginId],
        seedLatest: latestRelease(meta)?.version,
        installedVersion: ip.version,
        linked: !!ip.linked,
      }).outdated
    }).length
  }, [instance, plugins, latestVersions])

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
 * page — a stateful controller: it owns the store selectors, view
 * state, filtering/sorting/pagination, and the handlers, then renders
 * one of three extracted views (§M4): detail, registry, or the
 * installed/updates list. All presentational markup lives in
 * `@/features/plugins/Plugin*View.tsx`.
 * ------------------------------------------------------------------ */

export function PluginsPage() {
  const instances = useInstanceStore((s) => s.instances)
  const plugins = useCatalogStore((s) => s.plugins)
  const versions = useCatalogStore((s) => s.versions)
  /**
   * The page needs to know *which* plugins are installing and at what stage.
   * It does not need the live byte counters — `TransferInline` subscribes to
   * those per row. Subscribing to the whole `pluginTransfers` map here made
   * the page re-render on every progress event (Rust emits one roughly every
   * 120ms per install) and rebuild all 60 market cards each time, which is
   * precisely the hazard `RegistryCard`'s own subscription exists to avoid.
   */
  const transferStages = useCatalogStore(
    useShallow((s) => {
      const stages: Record<string, PluginTransferState['stage']> = {}
      for (const [key, value] of Object.entries(s.pluginTransfers)) stages[key] = value.stage
      return stages
    }),
  )
  const latestVersions = useCatalogStore((s) => s.latestVersions)
  const pluginsOffline = useCatalogStore((s) => s.pluginsOffline)
  const pluginsError = useCatalogStore((s) => s.pluginsError)
  const pluginsOrigin = useCatalogStore((s) => s.pluginsOrigin)
  const catalogLoading = useCatalogStore((s) => s.loading)
  const install = useCatalogStore((s) => s.installPlugin)
  const setEnabled = useCatalogStore((s) => s.setPluginEnabled)
  const uninstall = useCatalogStore((s) => s.uninstallPlugin)
  const refreshLatest = useCatalogStore((s) => s.refreshLatestVersions)
  const recheckLatest = useCatalogStore((s) => s.recheckLatestVersions)

  const scope = useViewStore((s) => s.pluginScope)
  const setScope = useViewStore((s) => s.setPluginScope)
  const tab = useViewStore((s) => s.pluginTab)
  const setTab = useViewStore((s) => s.setPluginTab)
  const query = useViewStore((s) => s.pluginQuery)
  const setQuery = useViewStore((s) => s.setPluginQuery)
  const category = useViewStore((s) => s.pluginCategory)
  const setCategory = useViewStore((s) => s.setPluginCategory)
  const source = useViewStore((s) => s.pluginSource)
  const setSource = useViewStore((s) => s.setPluginSource)
  const sort = useViewStore((s) => s.pluginSort)
  const setSort = useViewStore((s) => s.setPluginSort)
  const push = useUIStore((s) => s.push)
  const { t, scale } = useMotion()
  // Animation intensity is a user setting: at 「关闭」 nothing may translate.
  const riseShift = scale === 0 ? 0 : 8

  const [detailId, setDetailId] = useState<string | null>(null)

  /**
   * Filtering runs over the full 1800-entry catalog and re-renders up to 60
   * cards, which is far too much work to do between keystrokes. Deferring it
   * keeps the input itself responsive: the field updates immediately, the
   * results catch up a frame or two later.
   */
  const deferredQuery = useDeferredValue(query)

  /**
   * The live catalog is 1800+ entries; rendering them all as cards freezes
   * the webview. Show a page at a time and let 加载更多 extend it.
   *
   * The page is mounted in two steps. Committing all 60 rows in one task
   * blocks the main thread long enough to swallow the tab transition whole —
   * the animation runs to completion while nothing can paint, so switching
   * to 插件库 looked like it had no animation at all. Only ~5 cards fit on
   * screen, so the first commit renders a couple of screenfuls and the rest
   * arrives in a transition, which React can time-slice around the frames
   * the animation needs.
   */
  const [visibleCount, setVisibleCount] = useState(REGISTRY_FIRST_PAINT)
  const [, startFill] = useTransition()
  useEffect(() => {
    setVisibleCount(REGISTRY_FIRST_PAINT)
    if (tab !== 'registry') return
    startFill(() => setVisibleCount(REGISTRY_PAGE))
  }, [tab, deferredQuery, category, source, sort])

  const instance = resolvePluginScope(instances, scope)
  const dshVersion = versions.find((v) => v.id === instance?.versionId)
  const detail = detailId ? (plugins.find((p) => p.id === detailId) ?? null) : null

  // Demo images: the registry's own `screenshots` first, then anything the
  // plugin's README embeds. Fetched lazily — one request per detail view,
  // memoized per repo for the session.
  const [repoShots, setRepoShots] = useState<string[]>([])
  const detailRepo = detail?.repoUrl?.match(/github\.com\/([A-Za-z0-9-]+\/[A-Za-z0-9._-]+)/)?.[1]
  useEffect(() => {
    let alive = true
    setRepoShots([])
    if (detailRepo) {
      void fetchRepoScreenshots(detailRepo).then((urls) => {
        if (alive && urls.length) setRepoShots(urls)
      })
    }
    return () => {
      alive = false
    }
  }, [detailRepo])

  // Resolve real npm latest versions for the updates tab — only for the
  // handful of installed plugins, never for the whole catalog.
  useEffect(() => {
    if (tab === 'updates' && instance) void refreshLatest(instance.id)
  }, [tab, instance, refreshLatest])

  const compatibility = (plugin: Plugin, version: string) => {
    const release = plugin.releases.find((r) => r.version === version)
    if (!release || !instance) return 'unknown' as const
    if (release.compatible.includes(instance.versionId)) return 'ok' as const
    if (release.incompatible?.includes(instance.versionId)) return 'bad' as const
    return 'unknown' as const
  }

  const registry = useMemo(() => {
    const q = deferredQuery.trim().toLowerCase()
    const filtered = plugins.filter((p) => {
      if (category !== 'all' && p.category !== category) return false
      if (source !== 'all' && p.source.kind !== source) return false
      if (!q) return true
      return (
        p.name.toLowerCase().includes(q) ||
        p.summary.toLowerCase().includes(q) ||
        (p.summaryEn ?? '').toLowerCase().includes(q) ||
        p.author.toLowerCase().includes(q)
      )
    })
    const byStars = (p: Plugin) => p.stars ?? -1
    return [...filtered].sort((a, b) => {
      if (sort === 'stars') return byStars(b) - byStars(a)
      if (sort === 'newest') return (b.addedAt ?? '').localeCompare(a.addedAt ?? '')
      return b.downloads - a.downloads
    })
  }, [plugins, deferredQuery, category, source, sort])

  const categories = useMemo(() => {
    const present = new Set(plugins.map((p) => p.category))
    return [...present]
  }, [plugins])

  const shown = useMemo(() => registry.slice(0, visibleCount), [registry, visibleCount])

  /* ---------------- discovery strip ---------------- */

  const picks = useViewStore((s) => s.pluginPicks)
  const picksSeen = useViewStore((s) => s.pluginPicksSeen)
  const setPicks = useViewStore((s) => s.setPluginPicks)
  const [discoverOpen, setDiscoverOpen] = useState(false)

  const pluginIndex = useMemo(() => new Map(plugins.map((p) => [p.id, p])), [plugins])
  const recommended = useMemo(
    () => picks.map((id) => pluginIndex.get(id)).filter((p): p is Plugin => !!p),
    [picks, pluginIndex],
  )

  const reroll = useCallback(() => {
    const installedIds = new Set(instance?.plugins.map((ip) => ip.pluginId) ?? [])
    const eligible = plugins.filter((p) => !installedIds.has(p.id))
    const recent = new Set(picksSeen)
    const fresh = eligible.filter((p) => !recent.has(p.id))
    // Skipping recent picks is a nicety, not a constraint: if the memory has
    // eaten most of a small catalogue, a full strip of repeats beats a short
    // one. `sampleStratified` keeps half the draw in the popular tier so the
    // block always has something worth installing in it.
    const pool = fresh.length >= RECOMMEND_COUNT ? fresh : eligible
    setPicks(sampleStratified(pool, RECOMMEND_COUNT, popularity).map((p) => p.id))
  }, [plugins, instance, picksSeen, setPicks])

  /**
   * Opening always draws a fresh batch — "随便看看" that shows yesterday's
   * twelve is not doing what its name says. The picks stay in the store so
   * 换一批 and the cursor survive while the overlay is open.
   */
  const openDiscovery = useCallback(() => {
    if (plugins.length > 0) reroll()
    setDiscoverOpen(true)
  }, [plugins.length, reroll])


  const installed = useMemo(() => {
    if (!instance) return []
    const q = query.trim().toLowerCase()
    return instance.plugins
      .map((ip) => {
        const meta = plugins.find((p) => p.id === ip.pluginId)
        const check = latestVersions[ip.pluginId]
        // Single source of truth for the update interpretation (§M4/R5): the
        // "not up to date" claim only fires when we KNOW a newer version, and
        // checking / error / unresolvable render as an explicit state below.
        const { newest, outdated } = resolveUpdate({
          check,
          seedLatest: meta ? latestRelease(meta)?.version : undefined,
          installedVersion: ip.version,
          linked: !!ip.linked,
        })
        return {
          ...ip,
          meta,
          name: meta?.name ?? linkedPluginNames[ip.pluginId] ?? ip.pluginId,
          newest,
          outdated,
          check,
          compat: meta ? compatibility(meta, ip.version) : ('unknown' as const),
        }
      })
      .filter((p) => !q || p.name.toLowerCase().includes(q))
    // `compatibility` closes over the scoped instance deliberately.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [instance, plugins, latestVersions, query])

  // Update-check health across the instance's checkable plugins (§R5), driven
  // by the explicit state — used for the re-check affordance and to stop the
  // "all up to date" claim from covering checking / failed / unresolvable sets.
  //
  // This must stay above the early return below. A hook placed after it runs
  // only on the renders that do not bail out, which is exactly the "Rendered
  // more hooks than during the previous render" crash: with no instance,
  // switching 已安装 → 插件库 blanked the whole page.
  const checkCounts = useMemo(() => {
    const c = { checking: 0, failed: 0, unresolvable: 0 }
    if (!instance) return c
    for (const ip of instance.plugins) {
      if (ip.linked) continue
      const st = latestVersions[ip.pluginId]?.status
      if (st === 'checking') c.checking++
      else if (st === 'error') c.failed++
      else if (st === 'unresolvable') c.unresolvable++
    }
    return c
  }, [instance, latestVersions])

  // Only the instance-scoped views (installed / updates) require an instance.
  // The market itself is always browsable — installs are where a target
  // instance becomes necessary.
  if (!instance && tab !== 'registry') {
    return (
      <PageShell title="插件">
        <EmptyState
          icon={<Blocks size={20} />}
          title="还没有实例"
          description="插件必须安装到某个实例里。先创建一个实例，再回来管理插件。"
          action={
            <Button variant="primary" onClick={() => push({ name: 'create' })}>
              新建实例
            </Button>
          }
        />
      </PageShell>
    )
  }

  const installState = (pluginId: string) =>
    instance ? transferStages[pluginKey(instance.id, pluginId)] : undefined
  const installedEntry = (pluginId: string) =>
    instance?.plugins.find((ip) => ip.pluginId === pluginId)

  /** Installs need a target instance; browsing never does. */
  const requireInstance = (run: (target: NonNullable<typeof instance>) => void) => {
    if (!instance) {
      useUIStore.getState().toast({
        kind: 'info',
        title: '先创建一个实例',
        message: '插件安装在实例内部；创建后即可一键安装。',
        action: { label: '新建实例', run: () => push({ name: 'create' }) },
      })
      return
    }
    run(instance)
  }

  const upgradable = installed.filter((p) => p.outdated)
  const rows = tab === 'updates' ? upgradable : installed

  /* ---------------- detail ---------------- */

  if (tab === 'registry' && detail) {
    return (
      <PluginDetailView
        detail={detail}
        instance={instance}
        instances={instances}
        scope={scope}
        repoShots={repoShots}
        installedVersion={installedEntry(detail.id)?.version}
        installing={!!installState(detail.id)}
        setDetailId={setDetailId}
        setScope={setScope}
        onInstall={(targetId) => void install(targetId, detail.id)}
        onRequireInstance={requireInstance}
        onCreateInstance={() => push({ name: 'create' })}
      />
    )
  }

  /* ---------------- registry grid ---------------- */

  return (
    <PageShell
      title="插件"
      subtitle={
        instance ? (
          <span className="flex flex-wrap items-center gap-1.5">
            当前作用于
            <span className="font-medium text-ink">{instance.name}</span>
            <Chip>DSH {dshVersion?.name}</Chip>
            <span className="text-ink-faint">插件只影响这一个实例。</span>
          </span>
        ) : (
          <span className="text-ink-faint">自由浏览插件市场，安装时再选择目标实例。</span>
        )
      }
      actions={
        <div className="flex items-center gap-2">
          <Button size="sm" variant="secondary" onClick={openDiscovery}>
            <Shuffle size={12} />
            随便看看
          </Button>
          {instance && (
            <>
              <InstanceTile name={instance.name} hue={instance.hue} size={30} />
              <Dropdown
                ariaLabel="作用实例"
                align="end"
                className="w-40"
                value={scope}
                onChange={setScope}
                options={instances.map((i) => ({ value: i.id, label: i.name }))}
              />
            </>
          )}
        </div>
      }
    >
      {/* `mode="wait"` holds the incoming tab back until the outgoing one has
          finished leaving, so the exit duration is dead time the user feels
          as lag on every switch. Keep the sequencing — it avoids two panels
          overlapping in flow — but make the exit almost instant so the new
          tab starts painting right away. */}
      <AnimatePresence mode="wait" initial={false}>
        <motion.div
          key={tab}
          initial={{ opacity: 0, y: riseShift }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, transition: t(0.06) }}
          transition={t(0.18)}
        >
      {tab === 'registry' ? (
        <PluginRegistryView
          instance={instance}
          dshVersionName={dshVersion?.name}
          pluginsOffline={pluginsOffline}
          pluginsError={pluginsError ?? null}
          pluginsOrigin={pluginsOrigin}
          catalogLoading={catalogLoading}
          catalogCount={plugins.length}
          query={query}
          setQuery={setQuery}
          category={category}
          setCategory={(v) => setCategory(v as never)}
          source={source}
          setSource={(v) => setSource(v as never)}
          sort={sort}
          setSort={(v) => setSort(v as never)}
          categories={categories}
          registryCount={registry.length}
          shown={shown}
          onLoadMore={() => startFill(() => setVisibleCount((n) => n + REGISTRY_PAGE))}
          onResetFilters={() => {
            setQuery('')
            setCategory('all')
            setSource('all')
            setSort('recommended')
          }}
          installedVersionFor={(id) => installedEntry(id)?.version}
          onSelect={setDetailId}
        />
      ) : !instance ? null : (
        <PluginInstalledView
          instance={instance}
          tab={tab === 'updates' ? 'updates' : 'installed'}
          rows={rows}
          upgradableCount={upgradable.length}
          checkCounts={checkCounts}
          installStateFor={installState}
          setEnabled={setEnabled}
          uninstall={uninstall}
          install={install}
          onGoUpdates={() => setTab('updates')}
          onGoRegistry={() => setTab('registry')}
          onRecheck={() => void recheckLatest(instance.id)}
        />
      )}
        </motion.div>
      </AnimatePresence>

      <PluginDiscovery
        open={discoverOpen}
        onClose={() => setDiscoverOpen(false)}
        picks={recommended}
        total={plugins.length}
        instance={instance}
        onReroll={reroll}
        onOpenDetail={(id) => {
          setTab('registry')
          setDetailId(id)
        }}
      />
    </PageShell>
  )
}
