/**
 * Inline "对话迁移" panel for the instance detail 数据 card.
 *
 * It lets the user copy one or more of *this* instance's DSH sessions into
 * other managed instances — the spec's part-5分发 model (each target gets a
 * brand-new forked session, the source is untouched). Kept inline (not a
 * modal) because the only dialog primitive here is confirm/prompt, and a
 * multi-select target list deserves to stay in the instance's context.
 *
 * Only instances PHL owns a writable home for are selectable: an external
 * (in-place) instance's home is the user's own directory, so the backend
 * refuses writes there and we don't offer it as a target.
 */
import { useEffect, useState } from 'react'
import { ArrowRight, Check, Copy, Loader2, X } from 'lucide-react'
import {
  copySessions,
  listSessions,
  type RemoteSessionCopyOutcome,
  type RemoteSessionInfo,
} from '@/lib/desktop'
import { isDesktop } from '@/lib/desktopCore'
import { formatDateTime } from '@/lib/format'
import { useInstanceStore } from '@/stores/instanceStore'
import { useUIStore } from '@/stores/uiStore'
import { Badge, Button, Checkbox, Spinner } from '@/components/ui'

export function SessionCopyPanel({
  instanceId,
  onCopied,
}: {
  instanceId: string
  /** Fired after a successful copy so the host card re-measures its count. */
  onCopied?: () => void
}) {
  const instances = useInstanceStore((s) => s.instances)
  const states = useInstanceStore((s) => s.states)
  const confirm = useUIStore((s) => s.confirm)
  const toast = useUIStore((s) => s.toast)

  const [sessions, setSessions] = useState<RemoteSessionInfo[] | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [targets, setTargets] = useState<Set<string>>(new Set())
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let alive = true
    setSessions(null)
    if (isDesktop) {
      listSessions(instanceId)
        .then((list) => alive && setSessions(list))
        .catch((e) => alive && setError(e instanceof Error ? e.message : String(e)))
    } else {
      setSessions([])
    }
    return () => {
      alive = false
    }
  }, [instanceId])

  // Targets: other instances PHL owns a copy home for, and that aren't running
  // (the backend also refuses a running target, so we pre-filter the obvious).
  const selectable = instances.filter(
    (i) =>
      i.id !== instanceId &&
      i.managementMode !== 'external' &&
      states[i.id]?.status !== 'running' &&
      states[i.id]?.status !== 'starting',
  )

  const toggle = (set: Set<string>, key: string) => {
    const next = new Set(set)
    next.has(key) ? next.delete(key) : next.add(key)
    return next
  }

  const run = async () => {
    const dirs = [...selected]
    const tgts = [...targets]
    if (!dirs.length || !tgts.length) return
    const ok = await confirm({
      title: '复制历史对话',
      message: `把选中的 ${dirs.length} 条对话复制到 ${tgts.length} 个实例？`,
      detail:
        '每个目标都会得到一条全新的对话（保留原内容与工作目录，并记录来源血缘）；本实例的原对话不受影响。',
      confirmLabel: '复制',
    })
    if (!ok) return
    setBusy(true)
    setError(null)
    try {
      const out: RemoteSessionCopyOutcome[] = await copySessions(instanceId, dirs, tgts)
      toast({
        kind: 'success',
        title: '历史对话已复制',
        message: `已生成 ${out.length} 条新对话；在目标实例里启动 DSH 即可继续。`,
      })
      setSelected(new Set())
      setTargets(new Set())
      // The source keeps its own count (copies never consume it), but every
      // touched instance's on-disk number moved — tell the host to re-measure.
      onCopied?.()
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      setError(msg)
      toast({ kind: 'error', title: '复制失败', message: msg })
    } finally {
      setBusy(false)
    }
  }

  if (!isDesktop) {
    return <p className="text-xs text-ink-faint">对话迁移需要桌面版 PHL 访问本地会话文件。</p>
  }
  if (sessions === null) {
    return error ? (
      <p className="text-xs text-warn">{error}</p>
    ) : (
      <div className="flex items-center gap-2 text-xs text-ink-faint">
        <Spinner size={13} /> 正在列出会话…
      </div>
    )
  }
  if (sessions.length === 0) {
    return <p className="text-xs text-ink-faint">该实例暂无可迁移的历史对话。</p>
  }
  if (selectable.length === 0) {
    return (
      <p className="text-xs text-ink-faint">
        没有可作为目标的其他受管实例。先新建实例或用「接入本机 DSH」添加一个，再来复制。
      </p>
    )
  }

  return (
    <div className="mt-2 flex flex-col gap-2 rounded-lg bg-surface-sunken p-2.5 ring-1 ring-inset ring-line">
      <div>
        <p className="mb-1 text-xs font-medium text-ink-muted">选择对话（{sessions.length}）</p>
        <div className="max-h-44 space-y-1 overflow-y-auto">
          {sessions.map((s) => (
            <label
              key={s.sessionDir}
              className="flex cursor-pointer items-center gap-2 text-sm"
            >
              <Checkbox checked={selected.has(s.sessionDir)} onChange={() => setSelected(toggle(selected, s.sessionDir))} />
              <span className="min-w-0 flex-1 truncate">
                <span className="block truncate text-ink">
                  {s.id.replace(/^session-/, '').slice(0, 8)}
                  {s.parent ? ' · 由复制而来' : ''}
                </span>
                <span className="block truncate text-xs text-ink-faint">
                  {s.cwd ?? s.project} · {formatDateTime(s.createdAtMs)}
                </span>
              </span>
              {s.originSubagent && <Badge tone="neutral">子代理</Badge>}
            </label>
          ))}
        </div>
      </div>
      <div>
        <p className="mb-1 text-xs font-medium text-ink-muted">目标实例</p>
        <div className="flex flex-wrap gap-1.5">
          {selectable.map((i) => {
            const on = targets.has(i.id)
            return (
              <button
                key={i.id}
                onClick={() => setTargets(toggle(targets, i.id))}
                className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs ring-1 transition-colors ${
                  on ? 'bg-accent text-white ring-accent' : 'bg-surface text-ink-muted ring-line hover:ring-line-strong'
                }`}
              >
                {on ? <Check size={11} /> : <ArrowRight size={11} />}
                {i.name}
              </button>
            )
          })}
        </div>
      </div>
      {error && sessions.length > 0 && <p className="text-xs text-warn">{error}</p>}
      <div className="flex items-center justify-end gap-2">
        {(selected.size > 0 || targets.size > 0) && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setSelected(new Set())
              setTargets(new Set())
            }}
          >
            <X size={12} />
            清空
          </Button>
        )}
        <Button
          size="sm"
          variant="primary"
          disabled={!selected.size || !targets.size || busy}
          onClick={() => void run()}
        >
          {busy ? <Loader2 size={12} className="animate-spin" /> : <Copy size={12} />}
          复制到其他实例（{selected.size}→{targets.size}）
        </Button>
      </div>
    </div>
  )
}
