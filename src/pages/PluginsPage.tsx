import {
  useEffect,
  Fragment,
  memo,
  useCallback,
  useDeferredValue,
  useMemo,
  useRef,
  useState,
  useTransition,
} from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  ArrowLeft,
  ArrowUp,
  ArrowUpCircle,
  Blocks,
  Check,
  Download,

  Github,
  Link2,
  Package2,
  RotateCcw,
  Search,
  Shuffle,
  Star,
  Trash2,
  X,
} from 'lucide-react'
import { openExternal } from '@/lib/desktop'
import { cn } from '@/lib/cn'
import { formatBytes, formatCount, formatDate, formatRelative, formatSpeed } from '@/lib/format'
import { hueTone, initials } from '@/lib/hue'
import { useMotion, MODAL_SCRIM, MODAL_Z } from '@/lib/motion'
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
import { useIsDark } from '@/stores/uiStore'
import {
  latestRelease,
  PLUGIN_CATEGORY_LABELS,
  type Instance,
  type Plugin,
  type PluginCategory,
} from '@/types'
import {
  Dropdown,
  Badge,
  Button,
  Card,
  Chip,
  EmptyState,
  Input,
  Notice,
  ProgressBar,
  Skeleton,
  Switch,
  Tooltip,
} from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelDivider, PanelGroup, PanelItem, PanelShell } from '@/components/layout/Panel'
import { InstanceTile } from '@/components/instance'

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
      const resolved = latestVersions[ip.pluginId]
      if (resolved) return resolved !== ip.version
      const seedLatest = latestRelease(meta)?.version
      return !!seedLatest && seedLatest !== ip.version
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
 * shared bits
 * ------------------------------------------------------------------ */

/** How many market cards render before 加载更多 takes over. */
const REGISTRY_PAGE = 60

/**
 * Rows committed in the tab's first render. About five fit on screen, so two
 * screenfuls is plenty to look complete while keeping that commit small
 * enough not to stall the tab transition.
 */
const REGISTRY_FIRST_PAINT = 12

/** How many plugins the discovery strip shows per draw. */
const RECOMMEND_COUNT = 12

/**
 * Ranking signal for the strip's popular tier. Stars are weighted up because
 * plenty of registry entries are GitHub-source and carry no install count at
 * all — ranking those purely on `downloads` would bury every one of them in
 * the long tail regardless of how well regarded they are.
 */
const popularity = (p: Plugin) => p.downloads + (p.stars ?? 0) * 10

/** PCL-style stage labels — every phase of the pipeline gets a name. */
const STAGE_LABEL: Record<string, string> = {
  preparing: '准备中',
  downloading: '下载中',
  verifying: '校验中',
  installing: '安装中',
}

function SourceBadge({ plugin }: { plugin: Plugin }) {
  if (plugin.source.kind === 'npm') {
    return (
      <Tooltip content={plugin.source.pkg}>
        <Badge tone="accent" icon={<Package2 size={9} />}>
          npm
        </Badge>
      </Tooltip>
    )
  }
  if (plugin.source.kind === 'tarball') {
    return <Badge tone="outline">预构建包</Badge>
  }
  return (
    <Badge tone="outline" icon={<Github size={9} />}>
      源码
    </Badge>
  )
}

function Popularity({ plugin }: { plugin: Plugin }) {
  return (
    <>
      {plugin.stars !== undefined && (
        <span className="inline-flex items-center gap-0.5">
          <Star size={11} className="text-warn" />
          {formatCount(plugin.stars)}
        </span>
      )}
      {plugin.downloads > 0 && <span>{formatCount(plugin.downloads)} 次安装</span>}
    </>
  )
}

/** Stable hue index from the plugin id, so a plugin keeps its colour everywhere. */
const hueFor = (id: string) => {
  let h = 0
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0
  return h
}

/** The GitHub owner behind a plugin (author / org), or empty when unknown. */
const ownerOf = (plugin: Plugin): string => {
  const fromUrl = plugin.repoUrl?.match(/github\.com\/([A-Za-z0-9-]+)(\/|$)/)?.[1]
  const fromId = plugin.id.split('/')[0]
  const owner = fromUrl ?? fromId
  return owner && /^[A-Za-z0-9-]+$/.test(owner) ? owner : ''
}

/**
 * The plugin's identity tile. Prefers the repo owner's GitHub avatar — it
 * exists for every valid owner and is what the user recognises from the
 * plugin's README — and falls back to the coloured initials tile when the
 * owner can't be resolved or the image fails (offline, blocked CDN).
 * The initials render underneath, so a slow load never leaves a hole.
 */
function PluginAvatar({ plugin, size = 42 }: { plugin: Plugin; size?: number }) {
  const dark = useIsDark()
  const tone = hueTone(hueFor(plugin.id), dark)
  const [imgOk, setImgOk] = useState(true)
  const owner = ownerOf(plugin)
  const avatarUrl = owner
    ? `https://github.com/${owner}.png?size=${Math.max(88, size * 2)}`
    : null
  return (
    <span
      className="relative flex shrink-0 select-none items-center justify-center overflow-hidden rounded-lg font-medium tracking-tight"
      style={{
        width: size,
        height: size,
        background: tone.soft,
        color: tone.text,
        boxShadow: `inset 0 0 0 1px ${tone.ring}`,
        fontSize: size * 0.34,
      }}
    >
      {initials(plugin.name)}
      {avatarUrl && imgOk && (
        <img
          src={avatarUrl}
          alt=""
          loading="lazy"
          decoding="async"
          onError={() => setImgOk(false)}
          className="absolute inset-0 h-full w-full object-cover"
        />
      )}
    </span>
  )
}

/**
 * dsh-market-style author pill leading the meta line, so authorship is the
 * first thing read after the title. No avatar here on purpose — the plugin
 * tile on the left already *is* the owner's GitHub avatar, and doubling it
 * reads as a rendering bug. Clicking opens the author's GitHub profile.
 */
function AuthorBadge({ plugin }: { plugin: Plugin }) {
  const owner = ownerOf(plugin)
  if (!owner) {
    return <span className="text-xs font-medium text-ink-muted">{plugin.author}</span>
  }
  return (
    <button
      className="group/author inline-flex items-center gap-1 rounded-full bg-surface-sunken px-2 py-0.5 text-xs font-medium text-ink-muted ring-1 ring-inset ring-line transition-colors duration-150 hover:text-accent-ink hover:ring-accent-ink/30"
      title={`查看 ${plugin.author} 的 GitHub 主页`}
      onClick={(e) => {
        e.stopPropagation()
        void openExternal(`https://github.com/${owner}`)
      }}
    >
      <span className="max-w-40 truncate">{plugin.author}</span>
    </button>
  )
}

/**
 * Inline five-phase progress: 准备 → 下载 → 校验 → 安装 → 完成. The bar and
 * caption belong to the card that started the install, so several plugins
 * can install at once without a global queue view.
 */
function TransferInline({ instanceId, pluginId }: { instanceId: string; pluginId: string }) {
  const transfer = useCatalogStore((s) => s.pluginTransfers[pluginKey(instanceId, pluginId)])
  const cancel = useCatalogStore((s) => s.cancelPlugin)
  const { t, scale } = useMotion()

  return (
    <AnimatePresence initial={false}>
      {transfer && (
        <motion.div
          initial={scale === 0 ? false : { height: 0, opacity: 0 }}
          animate={{ height: 'auto', opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={t(0.24)}
          className="overflow-hidden"
        >
          <div className="pt-3">
            <ProgressBar
              value={transfer.progress}
              height={4}
              active={transfer.stage === 'downloading'}
            />
            <div className="mt-1.5 flex items-center justify-between gap-2 text-sm text-ink-faint">
              <span>
                {STAGE_LABEL[transfer.stage] ?? transfer.stage}
                {transfer.stage === 'downloading' && transfer.bytesDone > 0 && (
                  <span className="ml-1.5">{formatBytes(transfer.bytesDone)}</span>
                )}
              </span>
              <span className="flex items-center gap-2">
                <span className="num">
                  {transfer.stage === 'downloading' && transfer.bytesPerSec > 0
                    ? formatSpeed(transfer.bytesPerSec)
                    : `${Math.round(transfer.progress * 100)}%`}
                </span>
                <Button size="sm" variant="ghost" onClick={() => cancel(instanceId, pluginId)}>
                  取消
                </Button>
              </span>
            </div>
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}

/**
 * The discovery overlay: a drawn batch on the left, the highlighted plugin
 * read in full on the right.
 *
 * This lives in an overlay rather than above the market list for two
 * reasons. A block at the top of the registry pushed the actual list down —
 * discovery got in the way of the browsing it was meant to support. And the
 * market row only has space for a truncated one-liner, which is exactly the
 * complaint: you cannot tell what a plugin is *for* from the list. Here the
 * summary is never clipped, both languages are shown, and the stats that say
 * whether a plugin is alive sit next to it.
 */
function PluginDiscovery({
  open,
  onClose,
  picks,
  total,
  instance,
  onReroll,
  onOpenDetail,
}: {
  open: boolean
  onClose: () => void
  picks: Plugin[]
  total: number
  instance?: Instance
  onReroll: () => void
  onOpenDetail: (id: string) => void
}) {
  const install = useCatalogStore((s) => s.installPlugin)
  const { overlay, pop } = useMotion()
  const [cursor, setCursor] = useState(0)
  const listRef = useRef<HTMLDivElement>(null)

  // A fresh batch always starts at the top; keep the cursor in range when the
  // batch shrinks (a pick can vanish if the catalog reloads under us).
  useEffect(() => setCursor((c) => (c < picks.length ? c : 0)), [picks])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose()
      } else if (e.key === 'ArrowDown') {
        e.preventDefault()
        setCursor((c) => Math.min(picks.length - 1, c + 1))
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setCursor((c) => Math.max(0, c - 1))
      }
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [open, picks.length, onClose])

  useEffect(() => {
    listRef.current?.querySelectorAll('[data-row]')[cursor]?.scrollIntoView({ block: 'nearest' })
  }, [cursor])

  const current = picks[cursor]
  const installedVersion = instance?.plugins.find((ip) => ip.pluginId === current?.id)?.version
  const npmPkg = current?.source.kind === 'npm' ? current.source.pkg : null

  return (
    <AnimatePresence>
      {open && (
        <motion.div key="discover" className={`fixed inset-0 ${MODAL_Z} flex items-center justify-center p-6`}>
          <motion.div
            variants={overlay}
            initial="hidden"
            animate="show"
            exit="out"
            onClick={onClose}
            className={MODAL_SCRIM}
          />
          <motion.div
            variants={pop}
            initial="hidden"
            animate="show"
            exit="out"
            className="relative flex h-[min(620px,82vh)] w-[880px] max-w-full flex-col overflow-hidden rounded-xl bg-surface-raised shadow-pop ring-1 ring-inset ring-line"
          >
            <div className="flex shrink-0 items-center gap-2.5 border-b border-line px-4 py-3">
              <Shuffle size={15} className="shrink-0 text-ink-faint" />
              <div className="min-w-0 flex-1">
                <div className="text-base font-medium text-ink">随便看看</div>
                <div className="text-sm text-ink-faint">
                  从 {formatCount(total)} 个插件里挑了 {picks.length} 个
                </div>
              </div>
              <Button size="sm" variant="secondary" onClick={onReroll}>
                <RotateCcw size={12} />
                换一批
              </Button>
              <Button size="sm" variant="ghost" onClick={onClose} aria-label="关闭">
                <X size={14} />
              </Button>
            </div>

            <div className="flex min-h-0 flex-1">
              <div ref={listRef} className="w-[248px] shrink-0 overflow-y-auto border-r border-line p-1.5">
                {picks.map((p, i) => (
                  <button
                    key={p.id}
                    data-row
                    onClick={() => setCursor(i)}
                    className={cn(
                      'flex w-full min-w-0 items-center gap-2.5 rounded-sm px-2 py-1.5 text-left transition-colors duration-100',
                      i === cursor ? 'bg-accent-soft/70' : 'hover:bg-surface-hover',
                    )}
                  >
                    <PluginAvatar plugin={p} size={26} />
                    <div className="min-w-0 flex-1">
                      <div
                        className={cn(
                          'truncate text-base',
                          i === cursor ? 'text-accent-ink' : 'text-ink',
                        )}
                      >
                        {p.name}
                      </div>
                      <div className="truncate text-xs text-ink-faint">{p.summary}</div>
                    </div>
                  </button>
                ))}
              </div>

              {current && (
                <div key={current.id} className="min-w-0 flex-1 overflow-y-auto p-5">
                  <div className="flex items-start gap-3">
                    <PluginAvatar plugin={current} size={44} />
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <h2 className="text-lg font-medium text-ink">{current.name}</h2>
                        {current.official && <Badge tone="accent">官方</Badge>}
                        <SourceBadge plugin={current} />
                        <Badge tone="outline">
                          {PLUGIN_CATEGORY_LABELS[current.category as PluginCategory] ??
                            current.category}
                        </Badge>
                      </div>
                      <div className="mt-2 flex flex-wrap items-center gap-2 text-sm text-ink-faint">
                        <AuthorBadge plugin={current} />
                        <Popularity plugin={current} />
                        {current.addedAt && <span>收录于 {formatDate(current.addedAt)}</span>}
                      </div>
                    </div>
                  </div>

                  {/* The whole point of this panel: never truncated. */}
                  <p className="mt-4 text-base leading-relaxed text-ink">{current.summary}</p>
                  {current.summaryEn && (
                    <p className="mt-2 text-sm leading-relaxed text-ink-faint">
                      {current.summaryEn}
                    </p>
                  )}

                  <div className="mt-4 flex flex-wrap items-center gap-3">
                    {current.repoUrl && (
                      <button
                        className="inline-flex items-center gap-1 text-sm text-accent-ink hover:underline"
                        onClick={() => void openExternal(current.repoUrl!)}
                      >
                        <Github size={12} />
                        仓库主页
                      </button>
                    )}
                    {npmPkg && (
                      <button
                        className="inline-flex items-center gap-1 text-sm text-accent-ink hover:underline"
                        onClick={() =>
                          void openExternal(`https://www.npmjs.com/package/${npmPkg}`)
                        }
                      >
                        <Package2 size={12} />
                        npm 页面
                      </button>
                    )}
                  </div>

                  <div className="mt-5 flex items-center gap-2 border-t border-line pt-4">
                    {installedVersion ? (
                      <Badge tone="neutral">已安装 {installedVersion}</Badge>
                    ) : (
                      <Button
                        variant="primary"
                        onClick={() => {
                          if (!instance) {
                            useUIStore.getState().toast({
                              kind: 'info',
                              title: '先创建一个实例',
                              message: '插件安装在实例内部；创建后即可一键安装。',
                            })
                            return
                          }
                          void install(instance.id, current.id)
                        }}
                      >
                        <Download size={12} />
                        安装到{instance ? `「${instance.name}」` : '实例'}
                      </Button>
                    )}
                    <Button
                      variant="secondary"
                      onClick={() => {
                        onOpenDetail(current.id)
                        onClose()
                      }}
                    >
                      完整详情
                    </Button>
                    <span className="ml-auto text-sm text-ink-faint">
                      {cursor + 1} / {picks.length}
                    </span>
                  </div>
                </div>
              )}
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

/**
 * One market result row, PCL-download-page style: identity tile, title with
 * a faint source subtitle, category tag + one-line summary, then a meta line
 * (compat · downloads · added · source) and the install action on the right.
 * Memoized, and — critically — owning its own transfer subscription: with
 * the live catalog (1800+ entries) a page-level progress subscription
 * re-renders the whole grid on every progress tick, which freezes the
 * renderer outright.
 */
const RegistryCard = memo(function RegistryCard({
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
        <button
          className="flex min-w-0 flex-1 items-start gap-3 text-left"
          onClick={() => onSelect(plugin.id)}
        >
          <PluginAvatar plugin={plugin} />
          <div className="min-w-0 flex-1">
            <div className="flex min-w-0 flex-wrap items-baseline gap-x-1.5">
              <span className="truncate text-base font-medium text-ink transition-colors group-hover/row:text-accent-ink">
                {plugin.name}
              </span>
              {subtitle && (
                <span className="truncate text-sm text-ink-faint">｜ {subtitle}</span>
              )}
              {plugin.official && <Badge tone="accent">官方</Badge>}
            </div>
            <div className="mt-1 flex min-w-0 items-center gap-1.5">
              <Badge tone="outline">
                {PLUGIN_CATEGORY_LABELS[plugin.category as PluginCategory] ?? plugin.category}
              </Badge>
              <p className="min-w-0 flex-1 truncate text-sm text-ink-muted">{plugin.summary}</p>
            </div>
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
        </button>
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
  const catalogLoading = useCatalogStore((s) => s.loading)
  const install = useCatalogStore((s) => s.installPlugin)
  const setEnabled = useCatalogStore((s) => s.setPluginEnabled)
  const uninstall = useCatalogStore((s) => s.uninstallPlugin)
  const refreshLatest = useCatalogStore((s) => s.refreshLatestVersions)

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
  const { stagger, riseItem, t, scale } = useMotion()
  // Animation intensity is a user setting: at 「关闭」 nothing may translate.
  const riseShift = scale === 0 ? 0 : 8

  const [detailId, setDetailId] = useState<string | null>(null)
  const [confirmUninstall, setConfirmUninstall] = useState<string | null>(null)

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
  // Narrowed outside the JSX closures — TS loses `kind === 'npm'` inside callbacks.
  const detailNpmPkg = detail?.source.kind === 'npm' ? detail.source.pkg : null

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
        // A checked npm latest beats the catalog's static release list.
        const resolved = latestVersions[ip.pluginId]
        const newest =
          resolved !== undefined
            ? (resolved ?? undefined)
            : meta
              ? latestRelease(meta)?.version
              : undefined
        return {
          ...ip,
          meta,
          name: meta?.name ?? linkedPluginNames[ip.pluginId] ?? ip.pluginId,
          newest,
          outdated: !!newest && !ip.linked && newest !== ip.version,
          compat: meta ? compatibility(meta, ip.version) : ('unknown' as const),
        }
      })
      .filter((p) => !q || p.name.toLowerCase().includes(q))
    // `compatibility` closes over the scoped instance deliberately.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [instance, plugins, latestVersions, query])

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
            <span className="max-w-[320px] truncate text-lg font-medium text-ink">
              {detail.name}
            </span>
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
                          onClick={() => void openExternal(`https://www.npmjs.com/package/${detailNpmPkg}`)}
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
                    {installedEntry(detail.id) ? (
                      <Badge tone="neutral">已安装</Badge>
                    ) : installState(detail.id) ? (
                      <Badge tone="warn">正在安装…</Badge>
                    ) : (
                      <Button
                        variant="primary"
                        block
                        onClick={() => requireInstance((target) => void install(target.id, detail.id))}
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
                  <Button
                    variant="primary"
                    block
                    className="mt-3"
                    onClick={() => push({ name: 'create' })}
                  >
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
        <>
          {pluginsOffline && (
            <Notice
              tone="warn"
              title="在线插件目录暂时不可用"
              className="mb-4"
              action={
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => void useCatalogStore.getState().load()}
                >
                  重试
                </Button>
              }
            >
              {pluginsError || '无法连接任何发布源'}。已安装的插件不受影响，恢复连接后重试即可浏览插件库。
            </Notice>
          )}
          {catalogLoading && plugins.length === 0 ? (
            // First catalog fetch in flight — skeleton rows keep the market's
            // shape instead of a blank page, then the real grid fades in.
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
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
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
                      value={category}
                      onChange={setCategory}
                      options={[
                        { value: 'all', label: '全部分类' },
                        ...categories.map((c) => ({
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
                      value={source}
                      onChange={setSource}
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
                      value={sort}
                      onChange={setSort}
                      options={[
                        { value: 'recommended', label: '最多安装' },
                        { value: 'stars', label: '最多星标' },
                        { value: 'newest', label: '最近收录' },
                      ]}
                    />
                  </label>
                </div>
                <div className="mt-3 flex items-center justify-between border-t border-line/70 pt-3">
                  <span className="text-sm text-ink-faint">
                    共 {formatCount(registry.length)} 个插件
                    {registry.length > shown.length && `，当前显示 ${shown.length} 个`}
                  </span>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => {
                      setQuery('')
                      setCategory('all')
                      setSource('all')
                      setSort('recommended')
                    }}
                  >
                    <RotateCcw size={12} />
                    重置条件
                  </Button>
                </div>
              </motion.div>

              {/* The grid fades in as one block rather than staggering its
                  children. At 60 rows a per-card cascade means 60 concurrent
                  transform animations spanning ~0.7s — the market kept
                  arriving long after it was readable, which is exactly what
                  read as sluggish. One opacity tween costs nothing and lets
                  the list be usable immediately. */}
              <motion.div
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                transition={t(0.16)}
                className="space-y-2"
              >
                {shown.map((p) => (
                  <RegistryCard
                    key={p.id}
                    plugin={p}
                    instanceId={instance?.id}
                    instanceVersionId={instance?.versionId}
                    dshVersionName={dshVersion?.name}
                    installedVersion={installedEntry(p.id)?.version}
                    onSelect={setDetailId}
                  />
                ))}
              </motion.div>
              {registry.length > shown.length && (
                <div className="mt-3 flex justify-center">
                  <Button
                    variant="secondary"
                    onClick={() => startFill(() => setVisibleCount((n) => n + REGISTRY_PAGE))}
                  >
                    加载更多（还有 {formatCount(registry.length - shown.length)} 个）
                  </Button>
                </div>
              )}
              {registry.length === 0 && (
                <EmptyState
                  icon={<Blocks size={20} />}
                  title="没有匹配的插件"
                  description="换个关键词，或清除分类筛选。"
                />
              )}
            </>
          )}
        </>
      ) : !instance ? null : rows.length === 0 ? (
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
          {rows.map((p) => {
            const stage = installState(p.pluginId)
            return (
              <motion.li
                key={p.pluginId}
                variants={riseItem}
                className="group/row rounded-lg bg-surface px-3.5 py-2.5 ring-1 ring-inset ring-line transition-shadow duration-200 hover:ring-line-strong/70"
              >
                <div className="flex items-center gap-3">
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
                      {p.meta && <SourceBadge plugin={p.meta} />}
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
                    {stage ? (
                      <Badge tone="warn">{STAGE_LABEL[stage] ?? '安装中'}</Badge>
                    ) : (
                      p.outdated &&
                      p.newest && (
                        <Button
                          size="sm"
                          variant="primary"
                          onClick={() => void install(instance.id, p.pluginId)}
                        >
                          <ArrowUpCircle size={12} />
                          更新
                        </Button>
                      )
                    )}
                    {!stage && (
                      <Tooltip content={p.enabled ? '停用' : '启用'}>
                        <Switch
                          checked={p.enabled}
                          onChange={(v) => void setEnabled(instance.id, p.pluginId, v)}
                          label={`启用 ${p.name}`}
                        />
                      </Tooltip>
                    )}
                    {!stage && !p.linked && confirmUninstall !== p.pluginId && (
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => setConfirmUninstall(p.pluginId)}
                        className="opacity-0 transition-opacity group-hover/row:opacity-100 focus:opacity-100"
                      >
                        <Trash2 size={12} />
                      </Button>
                    )}
                    {confirmUninstall === p.pluginId && (
                      <div className="flex items-center gap-1.5">
                        <Button
                          size="sm"
                          variant="danger"
                          onClick={() => {
                            setConfirmUninstall(null)
                            void uninstall(instance.id, p.pluginId)
                          }}
                        >
                          确认卸载
                        </Button>
                        <Button size="sm" variant="ghost" onClick={() => setConfirmUninstall(null)}>
                          <X size={12} />
                        </Button>
                      </div>
                    )}
                  </div>
                </div>
                <TransferInline instanceId={instance.id} pluginId={p.pluginId} />
              </motion.li>
            )
          })}
        </motion.ul>
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
