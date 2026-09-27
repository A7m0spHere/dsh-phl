import { useEffect, useMemo, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { GitCompare, X } from 'lucide-react'
import { useMotion, MODAL_SCRIM, MODAL_Z } from '@/lib/motion'
import { Badge, Button, Spinner } from '@/components/ui'
import { useInstanceStore, useUIStore } from '@/stores'
import { isDesktop } from '@/lib/desktopCore'
import { repository } from '@/services'
import { formatDateTime } from '@/lib/format'
import type { EnvironmentDiff, EnvironmentDiffItem } from '@/lib/desktop'
import { useEnvCompareStore } from './compareStore'

const STATE_BADGE: Record<EnvironmentDiffItem['state'], { tone: 'neutral' | 'warn' | 'accent' | 'danger' | 'ok'; label: string }> = {
  same: { tone: 'neutral', label: '相同' },
  different: { tone: 'warn', label: '不同' },
  'left-only': { tone: 'danger', label: '仅左侧' },
  'right-only': { tone: 'accent', label: '仅右侧' },
  unknown: { tone: 'danger', label: '未知' },
}

const CATEGORY_ORDER = ['DSH', 'Node', 'Profile', '插件', 'API 配置来源', '工作目录', '共享边界', '已知冲突']

/**
 * Environment compare (R1 · M2): a modal, read-only diff of two instances'
 * collected facts. Responses carry a request generation so switching the
 * counterpart quickly can never render a stale instance's result (B05).
 */
export function EnvironmentCompareDialog() {
  const leftId = useEnvCompareStore((s) => s.leftId)
  const preselected = useEnvCompareStore((s) => s.rightId)
  const close = useEnvCompareStore((s) => s.close)
  const instances = useInstanceStore((s) => s.instances)
  const { pop, overlay } = useMotion()
  const panelRef = useRef<HTMLDivElement>(null)

  const [rightId, setRightId] = useState<string | null>(null)
  const [diff, setDiff] = useState<EnvironmentDiff | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [showSame, setShowSame] = useState(false)
  /** Generation guard: only the newest request may render. */
  const requestRef = useRef(0)

  useEffect(() => {
    if (leftId) {
      setRightId(preselected && preselected !== leftId ? preselected : null)
      setDiff(null)
      setError(null)
      setShowSame(false)
    }
  }, [leftId, preselected])

  useEffect(() => {
    if (!leftId || !rightId) return
    const request = ++requestRef.current
    setLoading(true)
    setError(null)
    repository
      .compareEnvironments(leftId, rightId)
      .then((result) => {
        if (requestRef.current !== request) return // stale response — drop it
        setDiff(result)
        if (!result) setError('环境比较仅在桌面端可用（浏览器为模拟模式）')
      })
      .catch((err) => {
        if (requestRef.current !== request) return
        setError(err instanceof Error ? err.message : String(err))
      })
      .finally(() => {
        if (requestRef.current === request) setLoading(false)
      })
  }, [leftId, rightId])

  // While open, this dialog owns Escape — mirrored into the shared overlay
  // depth so the global go-back hotkey does not also navigate the page
  // behind it on the same keypress.
  const pushOverlay = useUIStore((s) => s.pushOverlay)
  const popOverlay = useUIStore((s) => s.popOverlay)
  useEffect(() => {
    if (!leftId) return
    pushOverlay()
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close()
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
      popOverlay()
    }
  }, [leftId, close, pushOverlay, popOverlay])

  const others = useMemo(() => instances.filter((i) => i.id !== leftId), [instances, leftId])

  const grouped = useMemo(() => {
    if (!diff) return []
    const visible = diff.items.filter((i) => showSame || i.state !== 'same')
    const map = new Map<string, EnvironmentDiffItem[]>()
    for (const item of visible) {
      map.set(item.category, [...(map.get(item.category) ?? []), item])
    }
    return [...map.entries()].sort(
      (a, b) =>
        (CATEGORY_ORDER.indexOf(a[0]) + 1 || CATEGORY_ORDER.length + 1) -
        (CATEGORY_ORDER.indexOf(b[0]) + 1 || CATEGORY_ORDER.length + 1),
    )
  }, [diff, showSame])

  if (!leftId) return null
  const leftName = instances.find((i) => i.id === leftId)?.name ?? leftId

  return (
    <AnimatePresence>
      <motion.div key="env-compare" className={`fixed inset-0 ${MODAL_Z} flex items-center justify-center p-8`}>
        <motion.div variants={overlay} initial="hidden" animate="show" exit="out" onClick={close} className={MODAL_SCRIM} />
        <motion.div
          ref={panelRef}
          variants={pop}
          initial="hidden"
          animate="show"
          exit="out"
          role="dialog"
          aria-modal="true"
          aria-label="比较环境"
          tabIndex={-1}
          onKeyDown={(e) => {
            if (e.key !== 'Tab') return
            const focusable = Array.from(
              panelRef.current?.querySelectorAll<HTMLElement>('button:not([disabled]), select, [tabindex]:not([tabindex="-1"])') ?? [],
            ).filter((node) => node.offsetParent !== null)
            if (focusable.length === 0) return
            const first = focusable[0]
            const last = focusable[focusable.length - 1]
            if (!panelRef.current?.contains(document.activeElement)) {
              e.preventDefault()
              first.focus()
            } else if (e.shiftKey && document.activeElement === first) {
              e.preventDefault()
              last.focus()
            } else if (!e.shiftKey && document.activeElement === last) {
              e.preventDefault()
              first.focus()
            }
          }}
          className="relative flex max-h-[80vh] w-[680px] flex-col overflow-hidden rounded-xl bg-surface-raised shadow-pop ring-1 ring-inset ring-line outline-none"
        >
          <div className="flex items-center gap-2.5 border-b border-line px-4 py-3">
            <GitCompare size={15} className="text-ink-faint" />
            <h2 className="text-md font-medium text-ink">比较环境</h2>
            <Button size="sm" variant="ghost" onClick={close} className="ml-auto" aria-label="关闭">
              <X size={14} />
            </Button>
          </div>

          <div className="space-y-3 border-b border-line px-4 py-3">
            <div className="flex items-center gap-2 text-sm">
              <span className="min-w-0 flex-1 truncate rounded-md bg-surface-sunken px-2.5 py-1.5 text-ink" title={leftName}>
                {leftName}
              </span>
              <GitCompare size={13} className="shrink-0 text-ink-faint" />
              <select
                value={rightId ?? ''}
                onChange={(e) => setRightId(e.target.value || null)}
                className="min-w-0 flex-1 rounded-md bg-surface-sunken px-2.5 py-1.5 text-ink ring-1 ring-inset ring-line-strong/60"
                aria-label="选择对照实例"
              >
                <option value="">选择对照实例…</option>
                {others.map((i) => (
                  <option key={i.id} value={i.id}>
                    {i.name}
                  </option>
                ))}
              </select>
            </div>
            {diff && (
              <div className="flex items-center gap-2 text-sm text-ink-faint">
                <span>检查时间：{formatDateTime(diff.leftObservedAt)} / {formatDateTime(diff.rightObservedAt)}</span>
                {diff.hasUnknown && <Badge tone="danger">含未知项</Badge>}
              </div>
            )}
          </div>

          <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
            {!rightId ? (
              <p className="py-8 text-center text-sm text-ink-faint">选择一个对照实例开始比较。</p>
            ) : loading ? (
              <p className="flex items-center justify-center gap-2 py-8 text-sm text-ink-faint">
                <Spinner size={14} /> 正在采集环境事实…
              </p>
            ) : error ? (
              <p className="py-8 text-center text-sm text-danger">{error}</p>
            ) : diff ? (
              grouped.length === 0 ? (
                <p className="py-8 text-center text-sm text-ink-faint">
                  没有可见差异 — 所有已比较项都相同。
                </p>
              ) : (
                <div className="space-y-4">
                  {grouped.map(([category, items]) => (
                    <section key={category}>
                      <h3 className="mb-1.5 text-xs font-medium uppercase tracking-wide text-ink-faint">{category}</h3>
                      <ul className="space-y-1">
                        {items.map((item) => {
                          const badge = STATE_BADGE[item.state]
                          return (
                            <li
                              key={item.key}
                              className="rounded-md bg-surface px-2.5 py-2 ring-1 ring-inset ring-line"
                            >
                              <div className="flex items-center gap-2">
                                <Badge tone={badge.tone}>{badge.label}</Badge>
                                {item.note && <span className="text-sm text-ink-faint">{item.note}</span>}
                              </div>
                              <div className="mt-1.5 grid grid-cols-2 gap-2 text-sm">
                                <div className="min-w-0">
                                  <span className="block text-xs text-ink-faint">左侧</span>
                                  <span className="block break-words text-ink-muted">{item.left ?? '—'}</span>
                                </div>
                                <div className="min-w-0">
                                  <span className="block text-xs text-ink-faint">右侧</span>
                                  <span className="block break-words text-ink-muted">{item.right ?? '—'}</span>
                                </div>
                              </div>
                            </li>
                          )
                        })}
                      </ul>
                    </section>
                  ))}
                </div>
              )
            ) : null}
          </div>

          <div className="flex items-center gap-3 border-t border-line px-4 py-2.5">
            <label className="flex cursor-pointer items-center gap-1.5 text-sm text-ink-muted">
              <input type="checkbox" checked={showSame} onChange={(e) => setShowSame(e.target.checked)} />
              显示相同项
            </label>
            <span className="ml-auto text-xs text-ink-faint">
              {isDesktop ? '只读比较，不修改任何实例' : '浏览器为模拟展示'}
            </span>
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  )
}
