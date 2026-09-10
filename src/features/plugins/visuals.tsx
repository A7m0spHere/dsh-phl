import {
  useState
} from 'react'

import {
  Github,
  Package2,
  Star,
} from 'lucide-react'
import { AnimatePresence, motion } from 'motion/react'
import { openExternal } from '@/lib/desktop'
import { formatBytes, formatCount, formatSpeed } from '@/lib/format'
import { hueTone, initials } from '@/lib/hue'
import { useMotion } from '@/lib/motion'
import { useCatalogStore, pluginKey } from '@/stores'
import { useIsDark } from '@/stores/uiStore'
import { Badge, Button, ProgressBar, Tooltip } from '@/components/ui'
import type { Plugin } from '@/types'

/* Shared presentational pieces for the plugin domain (Batch F, T-105):
 * avatars, source/trust badges, popularity and the inline transfer widget.
 */

export const REGISTRY_PAGE = 60

/**
 * Rows committed in the tab's first render. About five fit on screen, so two
 * screenfuls is plenty to look complete while keeping that commit small
 * enough not to stall the tab transition.
 */
export const REGISTRY_FIRST_PAINT = 12

/** How many plugins the discovery strip shows per draw. */
export const RECOMMEND_COUNT = 12

/**
 * Ranking signal for the strip's popular tier. Stars are weighted up because
 * plenty of registry entries are GitHub-source and carry no install count at
 * all — ranking those purely on `downloads` would bury every one of them in
 * the long tail regardless of how well regarded they are.
 */
export const popularity = (p: Plugin) => p.downloads + (p.stars ?? 0) * 10

/** PCL-style stage labels — every phase of the pipeline gets a name. */
export const STAGE_LABEL: Record<string, string> = {
  queued: '排队中…',
  preparing: '准备中',
  downloading: '下载中',
  verifying: '校验中',
  installing: '安装中',
  deps: '安装依赖（pnpm）…',
  committing: '提交变更…',
  removing: '正在卸载…',
}

/** Stages with no trustworthy ratio — npm/pnpm only draw their bar on a TTY,
 *  so the card shows an indeterminate pulse instead of freezing at 100%. */
const INDETERMINATE_STAGES = new Set([
  'queued',
  'preparing',
  'verifying',
  'deps',
  'committing',
  'removing',
])

export function SourceBadge({ plugin }: { plugin: Plugin }) {
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

export function Popularity({ plugin }: { plugin: Plugin }) {
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
export const hueFor = (id: string) => {
  let h = 0
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0
  return h
}

/** The GitHub owner behind a plugin (author / org), or empty when unknown. */
export const ownerOf = (plugin: Plugin): string => {
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
export function PluginAvatar({ plugin, size = 42 }: { plugin: Plugin; size?: number }) {
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
export function AuthorBadge({ plugin }: { plugin: Plugin }) {
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
export function TransferInline({ instanceId, pluginId }: { instanceId: string; pluginId: string }) {
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
              indeterminate={INDETERMINATE_STAGES.has(transfer.stage)}
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
                    : INDETERMINATE_STAGES.has(transfer.stage)
                      ? // A fabricated percentage under "安装中" read as progress
                        // the pipeline never had — show elapsed work, not a number.
                        ''
                      : `${Math.round(transfer.progress * 100)}%`}
                </span>
                {/* Uninstalls commit irreversibly after the transaction
                    recovery — a dead "取消" would lie about that. */}
                {transfer.stage === 'removing' ? null : (
                  <Button size="sm" variant="ghost" onClick={() => cancel(instanceId, pluginId)}>
                    取消
                  </Button>
                )}
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