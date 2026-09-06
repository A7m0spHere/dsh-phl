import { useMemo, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  Boxes,
  CirclePause,
  CirclePlay,
  FileUp,
  FolderInput,
  LayoutGrid,
  Package,
  Plus,
  Rows3,
  Search,
  Star,
  X,
} from 'lucide-react'
import { formatBytes } from '@/lib/format'
import { useMotion } from '@/lib/motion'
import {
  chooseBundleFile,
  importInstanceBundle,
  readInstanceBundle,
  type RemoteInstanceManifest,
} from '@/lib/desktop'
import { instanceFromRecord, newInstanceId } from '@/services/tauriInstances'
import {
  useCatalogStore,
  useInstanceStore,
  useUIStore,
  useViewStore,
  type InstanceFilter,
  type InstanceSort,
} from '@/stores'
import { Button, EmptyState, Input, Menu, Segmented, Skeleton } from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelDivider, PanelGroup, PanelItem, PanelShell, PanelStat } from '@/components/layout/Panel'
import { InstanceCard } from '@/components/instance/InstanceCard'
import { LaunchDock } from '@/components/instance/LaunchDock'

/* ------------------------------------------------------------------ *
 * context panel
 * ------------------------------------------------------------------ */

export function InstancesPanel() {
  const instances = useInstanceStore((s) => s.instances)
  const states = useInstanceStore((s) => s.states)
  const versions = useCatalogStore((s) => s.versions)
  const query = useViewStore((s) => s.instanceQuery)
  const setQuery = useViewStore((s) => s.setInstanceQuery)
  const filter = useViewStore((s) => s.instanceFilter)
  const setFilter = useViewStore((s) => s.setInstanceFilter)
  const sort = useViewStore((s) => s.instanceSort)
  const setSort = useViewStore((s) => s.setInstanceSort)
  const push = useUIStore((s) => s.push)

  const counts = useMemo(() => {
    const running = instances.filter((i) => states[i.id]?.status === 'running').length
    return {
      all: instances.length,
      running,
      stopped: instances.length - running,
      favorite: instances.filter((i) => i.favorite).length,
    }
  }, [instances, states])

  const disk = instances.reduce((sum, i) => sum + i.diskUsage, 0)
  const installedVersions = versions.filter((v) => v.state.kind === 'installed').length

  const filters: { id: InstanceFilter; label: string; icon: ReactNode; count: number }[] = [
    { id: 'all', label: '全部实例', icon: <Boxes size={14} />, count: counts.all },
    { id: 'running', label: '运行中', icon: <CirclePlay size={14} />, count: counts.running },
    { id: 'stopped', label: '已停止', icon: <CirclePause size={14} />, count: counts.stopped },
    { id: 'favorite', label: '已置顶', icon: <Star size={14} />, count: counts.favorite },
  ]

  return (
    <PanelShell>
      <div className="p-2.5 pb-1">
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索实例"
          prefix={<Search size={13} />}
          suffix={
            query ? (
              <button onClick={() => setQuery('')} aria-label="清除" className="hover:text-ink">
                <X size={12} />
              </button>
            ) : undefined
          }
        />
      </div>

      <PanelGroup>
        {filters.map((f) => (
          <PanelItem
            key={f.id}
            groupId="instance-filter"
            icon={f.icon}
            label={f.label}
            count={f.count}
            active={filter === f.id}
            onClick={() => setFilter(f.id)}
          />
        ))}
      </PanelGroup>

      <PanelDivider />

      <PanelGroup title="排序">
        <Segmented
          block
          size="sm"
          value={sort}
          onChange={(v) => setSort(v as InstanceSort)}
          options={[
            { value: 'recent', label: '最近' },
            { value: 'name', label: '名称' },
            { value: 'version', label: '版本' },
          ]}
        />
      </PanelGroup>

      <PanelDivider />

      <PanelGroup title="概览">
        <PanelStat label="实例" value={counts.all} />
        <PanelStat label="运行中" value={counts.running} tone={counts.running ? 'ok' : 'default'} />
        <PanelStat label="已安装版本" value={installedVersions} />
        <PanelStat label="占用空间" value={formatBytes(disk)} />
      </PanelGroup>

      <div className="mt-auto p-2.5">
        <Button block variant="secondary" onClick={() => push({ name: 'create' })}>
          <Plus size={13} />
          新建实例
        </Button>
      </div>
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ *
 * page
 * ------------------------------------------------------------------ */

export function InstancesPage() {
  const instances = useInstanceStore((s) => s.instances)
  const states = useInstanceStore((s) => s.states)
  const loaded = useInstanceStore((s) => s.loaded)
  const versions = useCatalogStore((s) => s.versions)
  const query = useViewStore((s) => s.instanceQuery)
  const filter = useViewStore((s) => s.instanceFilter)
  const setFilter = useViewStore((s) => s.setInstanceFilter)
  const sort = useViewStore((s) => s.instanceSort)
  const layout = useUIStore((s) => s.layout)
  const setLayout = useUIStore((s) => s.setLayout)
  const showDock = useUIStore((s) => s.showLaunchDock)
  const push = useUIStore((s) => s.push)
  const confirm = useUIStore((s) => s.confirm)
  const suggestPort = useInstanceStore((s) => s.suggestPort)
  const admitInstance = useInstanceStore((s) => s.admitInstance)
  const { stagger } = useMotion()

  const importBundle = async () => {
    const ui = useUIStore.getState()
    const path = await chooseBundleFile()
    if (!path) return
    let preview: Awaited<ReturnType<typeof readInstanceBundle>>
    try {
      preview = await readInstanceBundle(path)
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '无法读取 Bundle',
        message: err instanceof Error ? err.message : String(err),
      })
      return
    }
    // 凭据值从不随 Bundle 携带:预览里列出导入后需要重新配置的变量,
    // 机器本地变量的值同样会在导入时丢弃。
    const notes: string[] = ['Bundle 携带配置与插件记录；插件文件请在导入后通过插件页重新安装。']
    if (preview.credentials.length > 0) {
      notes.push(`导入后需重新配置凭据：${preview.credentials.join('、')}。`)
    }
    if (preview.machineOnly.length > 0) {
      notes.push(`机器本地变量（${preview.machineOnly.join('、')}）的值不随 Bundle 携带。`)
    }
    const ok = await confirm({
      title: `导入「${preview.name}」`,
      message: `版本 ${preview.versionId} · Runtime ${preview.runtimeId} · 端口 ${preview.port} · ${preview.pluginCount} 条插件记录。`,
      detail: notes.join(' '),
      confirmLabel: '导入',
    })
    if (!ok) return

    // Identity fields are the importer's (fresh id, free port); the Rust side
    // overwrites the environment fields with the bundle's own values.
    const manifest: RemoteInstanceManifest = {
      id: newInstanceId(preview.name),
      name: preview.name,
      note: null,
      kind: 'sandbox',
      hue: 0,
      versionId: preview.versionId,
      runtimeId: preview.runtimeId,
      port: suggestPort(),
      autoPort: true,
      profile: 'web',
      createdAt: new Date().toISOString(),
      lastRunAt: null,
      totalRuntime: 0,
      favorite: false,
      env: {},
      args: [],
      // Same promise as the create wizard: an imported instance boots
      // configured. The Rust import applies it through the shared path.
      api: { inheritance: 'default', providerIds: [] },
    }
    try {
      const outcome = await importInstanceBundle(path, manifest)
      admitInstance(instanceFromRecord(outcome.record))
      const notes: string[] = []
      if (preview.pluginCount > 0) {
        notes.push(`包含 ${preview.pluginCount} 条插件记录，可在插件页重新安装。`)
      }
      if (outcome.credentials.length > 0) {
        notes.push(`需重新配置凭据：${outcome.credentials.join('、')}。`)
      }
      ui.toast({
        kind: 'success',
        title: `已导入「${outcome.record.name}」`,
        message: notes.length > 0 ? notes.join(' ') : undefined,
      })
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '导入 Bundle 失败',
        message: err instanceof Error ? err.message : String(err),
      })
    }
  }

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase()
    const versionName = (id: string) => versions.find((v) => v.id === id)?.name ?? id

    let list = instances.filter((i) => {
      if (filter === 'running' && states[i.id]?.status !== 'running') return false
      if (filter === 'stopped' && states[i.id]?.status === 'running') return false
      if (filter === 'favorite' && !i.favorite) return false
      if (!q) return true
      return (
        i.name.toLowerCase().includes(q) ||
        (i.note ?? '').toLowerCase().includes(q) ||
        versionName(i.versionId).toLowerCase().includes(q) ||
        String(i.port).includes(q)
      )
    })

    list = [...list].sort((a, b) => {
      // Running instances always float to the top: what is live is what the
      // user is most likely to act on next.
      const ra = states[a.id]?.status === 'running' ? 1 : 0
      const rb = states[b.id]?.status === 'running' ? 1 : 0
      if (ra !== rb) return rb - ra
      if (a.favorite !== b.favorite) return a.favorite ? -1 : 1
      switch (sort) {
        case 'name':
          return a.name.localeCompare(b.name)
        case 'version':
          return versionName(b.versionId).localeCompare(versionName(a.versionId))
        case 'created':
          return new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
        default:
          return new Date(b.lastRunAt ?? 0).getTime() - new Date(a.lastRunAt ?? 0).getTime()
      }
    })
    return list
  }, [instances, states, versions, query, filter, sort])

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1">
        <PageShell
          title="实例"
          subtitle="每个实例固定自己的 DSH 版本、Runtime、插件与 DSH_HOME，可以同时运行、互不污染。"
          actions={
            <>
              {/* The three primary doors are 新建 / 接入 / 安装整合包; the
                  legacy Bundle importer stays reachable but demoted (spec
                  §26). Menu first, so a future second "other" format slots
                  in without re-growing the header. */}
              <Menu
                align="start"
                trigger={({ open, toggle }) => (
                  <Button variant="ghost" onClick={toggle} aria-expanded={open}>
                    更多导入方式
                  </Button>
                )}
                items={[
                  {
                    id: 'bundle',
                    label: '导入 Bundle',
                    icon: <FileUp size={13} />,
                    onSelect: () => void importBundle(),
                  },
                ]}
              />
              <Button variant="secondary" onClick={() => push({ name: 'adopt' })}>
                <FolderInput size={13} />
                接入本机 DSH
              </Button>
              <Button variant="secondary" onClick={() => push({ name: 'installPack' })}>
                <Package size={13} />
                安装整合包
              </Button>
              <Button variant="primary" onClick={() => push({ name: 'create' })}>
                <Plus size={13} />
                新建实例
              </Button>
            </>
          }
          toolbar={
            <>
              <span className="text-sm text-ink-faint">
                共 {visible.length} 个{filter !== 'all' && ' · 已筛选'}
              </span>
              <div className="ml-auto">
                <Segmented
                  size="sm"
                  value={layout}
                  onChange={(v) => setLayout(v as 'grid' | 'list')}
                  options={[
                    { value: 'grid', label: '', icon: <LayoutGrid size={12} />, title: '卡片视图' },
                    { value: 'list', label: '', icon: <Rows3 size={12} />, title: '列表视图' },
                  ]}
                />
              </div>
            </>
          }
        >
          {!loaded ? (
            <div className="grid grid-cols-1 gap-2.5 lg:grid-cols-2">
              {[0, 1, 2, 3].map((i) => (
                <Skeleton key={i} className="h-[104px]" />
              ))}
            </div>
          ) : visible.length === 0 ? (
            <EmptyState
              icon={<Boxes size={20} />}
              title={instances.length === 0 ? '还没有实例' : '没有匹配的实例'}
              description={
                instances.length === 0
                  ? '创建第一个实例：选择一个 DSH 版本和 Node Runtime，PHL 会为它准备独立的 DSH_HOME、插件目录与端口。'
                  : '试试更换筛选条件，或清空搜索关键字。'
              }
              action={
                instances.length === 0 ? (
                  <Button variant="primary" onClick={() => push({ name: 'create' })}>
                    <Plus size={13} />
                    新建实例
                  </Button>
                ) : (
                  <Button variant="secondary" onClick={() => setFilter('all')}>
                    显示全部实例
                  </Button>
                )
              }
            />
          ) : (
            <motion.div
              variants={stagger()}
              initial="hidden"
              animate="show"
              className={
                layout === 'grid' ? 'grid grid-cols-1 gap-2.5 lg:grid-cols-2' : 'flex flex-col gap-1.5'
              }
            >
              <AnimatePresence mode="popLayout">
                {visible.map((instance) => (
                  <InstanceCard key={instance.id} instance={instance} layout={layout} />
                ))}
              </AnimatePresence>
            </motion.div>
          )}
        </PageShell>
      </div>
      {showDock && loaded && <LaunchDock />}
    </div>
  )
}
