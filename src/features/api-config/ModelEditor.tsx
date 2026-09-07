import { parseThrownError } from '@/lib/errorCodes'
/** The model-catalog editor: per-provider /models discovery + manual entries. */
import { useEffect, useMemo, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { CloudDownload, Download, Loader2, Plus, RefreshCw, Search, WandSparkles } from 'lucide-react'
import { cn } from '@/lib/cn'
import { fetchProviderModels, isDesktop } from '@/lib/desktop'
import { ApiModelRef, RemoteModel } from '@/types'
import { Badge, Button, Input, Tooltip } from '@/components/ui'
import { repository } from '@/services'
import { useMotion } from '@/lib/motion'
import { metadataSummary } from '@/lib/modelMetadata'
import { ModelRow } from './ModelRow'
import {
  ENV_MISSING_PREFIX,
} from './fields'


const discoveryCache = new Map<string, RemoteModel[]>()

export function ModelEditor({
  models,
  onChange,
  onEnrichingChange,
  fetchCtx,
}: {
  models: ApiModelRef[]
  onChange: (m: ApiModelRef[]) => void
  /** The owner must not save its draft before pending models have been added. */
  onEnrichingChange: (pending: boolean) => void
  fetchCtx?: {
    api?: string
    baseURL: string
    apiKeyEnv: string
    apiKey?: string
    providerId?: string
    providerName?: string
  }
}) {
  const [pickerOpen, setPickerOpen] = useState(false)
  const [listing, setListing] = useState<RemoteModel[] | null>(null)
  const [query, setQuery] = useState('')
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [fetching, setFetching] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [needKey, setNeedKey] = useState(false)
  const [tempKey, setTempKey] = useState('')
  const [enriching, setEnriching] = useState(false)
  const [metadataMessage, setMetadataMessage] = useState('')
  const { t } = useMotion()
  const latest = useRef({ models, fetchCtx })
  latest.current = { models, fetchCtx }
  const isCurrent = (snapshot: typeof latest.current) =>
    latest.current.models === snapshot.models &&
    JSON.stringify(latest.current.fetchCtx) === JSON.stringify(snapshot.fetchCtx)
  const alive = useRef(true)
  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
      onEnrichingChange(false)
    }
  }, [onEnrichingChange])
  const setEnrichmentPending = (pending: boolean) => {
    setEnriching(pending)
    onEnrichingChange(pending)
  }
  // Cancel guard: a slow response must not reopen a picker the user already
  // closed (or paint a stale listing after a baseURL change).
  const fetchSeq = useRef(0)

  const canFetch = isDesktop && !!fetchCtx?.baseURL.trim()
  const existing = useMemo(
    () => new Set(models.map((m) => m.id.trim()).filter(Boolean)),
    [models],
  )

  // A listing fetched from endpoint A must not be addable after the user
  // edits the base/env/protocol out from under it — reset the picker on any
  // context change instead of silently merging a stale remote view.
  const ctxKey = fetchCtx ? `${fetchCtx.baseURL.trim()}|${fetchCtx.apiKeyEnv}|${fetchCtx.api ?? ''}` : ''
  useEffect(() => {
    fetchSeq.current += 1
    setFetching(false)
    setPickerOpen(false)
    setListing(null)
    setError(null)
    setNeedKey(false)
    setSelected(new Set())
  }, [ctxKey, fetchCtx?.providerName, fetchCtx?.providerId, fetchCtx?.apiKey])

  const doFetch = async (force = false) => {
    if (!fetchCtx || !isDesktop) return
    // Listings are account-specific, so the cache key carries *which*
    // credential produced them (temp paste wins over the stored key, and a
    // bare env resolution is a third identity of its own).
    const who = tempKey.trim() ? 'temp' : fetchCtx.apiKey?.trim() ? 'saved' : 'env'
    const cacheKey = `${fetchCtx.baseURL.trim()}|${fetchCtx.apiKeyEnv}|${fetchCtx.api ?? ''}|${who}`
    // A pasted temp key means a different account than whoever filled the
    // cache last: its listings are never reusable from cache.
    const cached = !tempKey.trim() && discoveryCache.get(cacheKey)
    if (!force && cached) {
      setListing(cached)
      setError(null)
      setNeedKey(false)
      setSelected(new Set())
      setPickerOpen(true)
      return
    }
    const seq = ++fetchSeq.current
    setFetching(true)
    setError(null)
    try {
      const out = await fetchProviderModels({
        baseURL: fetchCtx.baseURL,
        api: fetchCtx.api,
        apiKeyEnv: fetchCtx.apiKeyEnv,
        // The form's just-typed / stored key is tried first; the backend
        // then consults the OS credential store and finally the environment.
        apiKey: tempKey.trim() || fetchCtx.apiKey?.trim() || undefined,
        providerId: fetchCtx.providerId,
      })
      if (seq !== fetchSeq.current) return
      discoveryCache.set(cacheKey, out)
      setListing(out)
      setNeedKey(false)
      setSelected(new Set())
      setPickerOpen(true)
    } catch (e) {
      if (seq !== fetchSeq.current) return
      const msg = typeof e === 'string' ? e : parseThrownError(e).message
      const missing = msg.startsWith(ENV_MISSING_PREFIX)
      setNeedKey(missing)
      setError(missing ? msg.slice(ENV_MISSING_PREFIX.length) : msg)
      setListing(null)
      setPickerOpen(true)
    } finally {
      if (seq === fetchSeq.current) setFetching(false)
    }
  }

  const visible = (listing ?? []).filter((m) => {
    if (!query.trim()) return true
    const q = query.trim().toLowerCase()
    return m.id.toLowerCase().includes(q) || m.name?.toLowerCase().includes(q)
  })

  const toggle = (id: string) =>
    setSelected((s) => {
      const next = new Set(s)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const closePicker = () => {
    fetchSeq.current += 1
    setFetching(false)
    setPickerOpen(false)
    setSelected(new Set())
    setQuery('')
    // tempKey intentionally survives close/reopen in the same form session —
    // it is component state, dropped when the form unmounts.
  }

  const resolve = async (candidates: ApiModelRef[]) => {
    try {
      const batch = await repository.enrichModelMetadata({ models: candidates, provider: fetchCtx?.providerName })
      if (alive.current) setMetadataMessage(metadataSummary(batch))
      return batch.results.map((r) => r.model)
    } catch {
      if (alive.current) setMetadataMessage('补全暂不可用，模型仍可添加和保存；稍后可重试。')
      return candidates
    }
  }

  const completeModels = async (index?: number) => {
    if (enriching) return
    const snapshot = latest.current
    setEnrichmentPending(true)
    try {
      const candidates = index === undefined ? models : [models[index]]
      const completed = await resolve(candidates)
      if (!alive.current) return
      if (!isCurrent(snapshot)) {
        setMetadataMessage('配置已发生变化，本次补全未应用，请重试。')
        return
      }
      onChange(index === undefined ? completed : models.map((m, i) => i === index ? completed[0] : m))
    } finally { if (alive.current) setEnrichmentPending(false) }
  }

  const confirmAdd = async () => {
    if (enriching) return
    const snapshot = latest.current
    const picked = (listing ?? []).filter((m) => selected.has(m.id) && !existing.has(m.id))
    if (picked.length) {
      setEnrichmentPending(true)
      try {
        const completed = await resolve(picked)
        if (!alive.current) return
        if (!isCurrent(snapshot)) {
          setMetadataMessage('配置已发生变化，请重新选择模型。')
          return
        }
        onChange([...models, ...completed])
      } finally { if (alive.current) setEnrichmentPending(false) }
    }
    closePicker()
  }

  return (
    <fieldset disabled={enriching} className="min-w-0">
      <div className="mb-1.5 flex flex-wrap items-center justify-between gap-1">
        <Tooltip allowOverflow content="补全只填写缺失字段；保存后可同步到实例。来源记录仅保存在 PHL。">
          <span className="text-sm font-medium text-ink-muted">模型</span>
        </Tooltip>
        <div className="flex flex-wrap items-center gap-1">
          {models.some((m) => m.id.trim()) && <Button size="sm" variant="ghost" onClick={() => void completeModels()}>
            {enriching ? <Loader2 size={12} className="animate-spin" /> : <WandSparkles size={12} />}补全模型信息
          </Button>}
          {isDesktop &&
            (() => {
              const btn = (
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={!canFetch || fetching}
                  onClick={() => void doFetch(false)}
                >
                  {fetching ? <Loader2 size={12} className="animate-spin" /> : <CloudDownload size={12} />}
                  获取可用模型
                </Button>
              )
              // The Tooltip wrapper listens on the span, so wrapping an
              // enabled button would show an empty bubble on hover — only
              // the disabled (no-baseURL) case has something to explain.
              return canFetch ? (
                btn
              ) : (
                <Tooltip allowOverflow content="填写 Base URL 后可从端点获取模型列表">{btn}</Tooltip>
              )
            })()}
          <Button size="sm" variant="ghost" onClick={() => onChange([...models, { id: '' }])}>
            <Plus size={12} />
            添加
          </Button>
        </div>
      </div>
      {metadataMessage && <p role="status" className="mb-2 text-sm text-ink-muted">{metadataMessage}</p>}

      <AnimatePresence initial={false}>
        {pickerOpen && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={t()}
            className="overflow-hidden"
          >
            <div className="mb-1.5 rounded-md bg-surface-sunken ring-1 ring-inset ring-line">
              {error ? (
                <div className="p-2.5">
                  <p className="text-sm leading-relaxed text-warn">获取失败：{error}</p>
                  {needKey && (
                    <div className="mt-2 space-y-1.5">
                      <Input
                        type="password"
                        value={tempKey}
                        onChange={(e) => setTempKey(e.target.value)}
                        placeholder="临时粘贴密钥（仅本次请求使用，不会保存）"
                        className="max-w-sm font-mono"
                      />
                      <div className="flex items-center gap-1.5">
                        <Button
                          size="sm"
                          variant="primary"
                          disabled={!tempKey.trim() || fetching}
                          onClick={() => void doFetch(true)}
                        >
                          {fetching ? <Loader2 size={12} className="animate-spin" /> : <CloudDownload size={12} />}
                          使用该密钥获取
                        </Button>
                        <Button size="sm" variant="ghost" onClick={() => { closePicker(); setError(null) }}>
                          取消
                        </Button>
                      </div>
                    </div>
                  )}
                  {!needKey && (
                    <div className="mt-1.5">
                      <Button size="sm" variant="ghost" onClick={() => void doFetch(true)}>
                        <RefreshCw size={12} />
                        重试
                      </Button>
                    </div>
                  )}
                </div>
              ) : (
                <>
                  <div className="flex items-center gap-2 px-2.5 py-2">
                    <Input
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
                      placeholder="搜索模型"
                      prefix={<Search size={12} />}
                      className="w-52"
                    />
                    <span className="text-sm text-ink-faint">
                      {listing?.length ?? 0} 个可选 · 已添加 {existing.size} 个 · 已选 {selected.size} 个
                    </span>
                    <div className="ml-auto flex items-center gap-1">
                      <Button size="sm" variant="ghost" onClick={() => void doFetch(true)} disabled={fetching}>
                        <RefreshCw size={12} className={cn(fetching && 'animate-spin')} />
                        刷新
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => setSelected(new Set(visible.map((m) => m.id).filter((id) => !existing.has(id))))}>
                        全选
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
                        清空
                      </Button>
                    </div>
                  </div>
                  {(listing?.length ?? 0) === 0 ? (
                    <p className="px-2.5 pb-2.5 text-sm text-ink-faint">该端点未返回模型列表。</p>
                  ) : (
                    <div className="max-h-64 overflow-y-auto px-2.5">
                      {visible.map((m) => {
                        const added = existing.has(m.id)
                        return (
                          <label
                            key={m.id}
                            className={cn(
                              'flex items-center gap-2 border-b border-line/60 py-1.5 text-sm last:border-b-0',
                              added ? 'opacity-50' : 'cursor-pointer',
                            )}
                          >
                            <input
                              type="checkbox"
                              className="accent-accent"
                              disabled={added}
                              checked={added || selected.has(m.id)}
                              onChange={() => toggle(m.id)}
                            />
                            <span className="min-w-0 flex-1 truncate font-mono text-ink">{m.id}</span>
                            {m.name && m.name !== m.id && (
                              <span className="max-w-[200px] truncate text-ink-faint">{m.name}</span>
                            )}
                            {added && <Badge tone="neutral">已添加</Badge>}
                          </label>
                        )
                      })}
                      {visible.length === 0 && (
                        <p className="py-2 text-sm text-ink-faint">没有匹配「{query}」的模型。</p>
                      )}
                    </div>
                  )}
                  <div className="flex items-center justify-end gap-1.5 border-t border-line px-2.5 py-1.5">
                    <Button size="sm" variant="ghost" onClick={closePicker}>
                      取消
                    </Button>
                    <Button size="sm" variant="primary" disabled={selected.size === 0} onClick={confirmAdd}>
                      <Download size={12} />
                      添加所选（{selected.size}）
                    </Button>
                  </div>
                </>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {models.length === 0 ? (
        <div className="rounded-md bg-surface-sunken px-3 py-2 text-sm leading-relaxed text-ink-faint ring-1 ring-inset ring-line">
          不填任何模型时，DSH 使用其内置目录
        </div>
      ) : (
        <div className="space-y-1.5">
          {models.map((m, i) => (
            <ModelRow key={`form-row-${i}`} model={m}
              onChange={(value) => onChange(models.map((mm, j) => j === i ? value : mm))}
              onRemove={() => onChange(models.filter((_, j) => j !== i))}
              onEnrich={() => void completeModels(i)} />
          ))}
        </div>
      )}
    </fieldset>
  )
}

/* ------------------------------------------------------------------ *
 * provider row
 * ------------------------------------------------------------------ */
