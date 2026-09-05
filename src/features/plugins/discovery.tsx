/** The 随机发现 modal: a stratified sample of the catalog with its own paging. */
import { useEffect, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Download, Github, Package2, RotateCcw, Shuffle, X } from 'lucide-react'
import { openExternal } from '@/lib/desktop'
import { cn } from '@/lib/cn'
import { formatCount, formatDate } from '@/lib/format'
import { useMotion, MODAL_SCRIM, MODAL_Z } from '@/lib/motion'
import { useCatalogStore, useUIStore } from '@/stores'
import { PLUGIN_CATEGORY_LABELS, type Instance, type Plugin, type PluginCategory } from '@/types'
import { Badge, Button } from '@/components/ui'
import {
  SourceBadge,
  Popularity,
  PluginAvatar,
  AuthorBadge,
} from './visuals'


export function PluginDiscovery({
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