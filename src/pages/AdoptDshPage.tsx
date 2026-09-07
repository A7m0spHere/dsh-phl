import { useEffect } from 'react'
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  Check,
  Copy,
  FolderSearch,
  HardDriveDownload,
  Link2,
  RefreshCw,
  ScanSearch,
  X,
} from 'lucide-react'
import { formatBytes, formatDateTime } from '@/lib/format'
import { isDesktop } from '@/lib/desktopCore'
import type { RemoteDshCandidate } from '@/lib/desktop'
import { useAdoptionStore } from '@/stores/adoptionStore'
import { useUIStore } from '@/stores'
import {
  Badge,
  Button,
  Card,
  Checkbox,
  EmptyState,
  Input,
  ProgressBar,
  SectionCard,
  Skeleton,
  Spinner,
} from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelGroup, PanelShell } from '@/components/layout/Panel'

/* ------------------------------------------------------------------ *
 * context panel — a live stepper; the main tree and this panel share
 * `step` through the store, so it always reads "where am I in the flow".
 * ------------------------------------------------------------------ */

const STEPS: { key: string; label: string }[] = [
  { key: 'discover', label: '发现环境' },
  { key: 'configure', label: '接入方式' },
  { key: 'preview', label: '预览确认' },
  { key: 'progress', label: '执行接入' },
]

export function AdoptDshPanel() {
  const step = useAdoptionStore((s) => s.step)
  const index = STEPS.findIndex((st) => st.key === step)
  return (
    <PanelShell>
      <PanelGroup title="接入步骤">
        <div className="mt-1 flex flex-col gap-0.5">
          {STEPS.map((st, i) => {
            const done = i < index
            const active = i === index
            return (
              <div
                key={st.key}
                className={`flex items-center gap-2.5 rounded-sm px-1.5 py-1.5 text-base ${
                  active ? 'text-ink' : 'text-ink-muted'
                }`}
              >
                <span
                  className={`flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-full text-[11px] ring-1 transition-colors ${
                    done
                      ? 'bg-ok/15 text-ok ring-ok/40'
                      : active
                        ? 'bg-accent/10 text-accent ring-accent/40'
                        : 'bg-surface text-ink-faint ring-line-strong'
                  }`}
                >
                  {done ? '✓' : i + 1}
                </span>
                {st.label}
              </div>
            )
          })}
        </div>
      </PanelGroup>
      <div className="p-3 text-sm text-ink-faint">
        推荐“复制到 PHL”：原 DSH 保持不变，接入只多占磁盘，不会动你的环境。
      </div>
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ *
 * page — one screen per wizard step
 * ------------------------------------------------------------------ */

export function AdoptDshPage() {
  const step = useAdoptionStore((s) => s.step)
  const reset = useAdoptionStore((s) => s.reset)
  const push = useUIStore((s) => s.push)

  // Leaving the flow clears the transient draft so the next entry starts clean.
  useEffect(() => () => reset(), [reset])

  return (
    <PageShell
      title="接入本机 DSH"
      subtitle="扫描这台机器上已有的 DSH，选一个接入成 PHL 实例。推荐复制到 PHL，原环境保持不变。"
      actions={
        <Button variant="ghost" onClick={() => push({ name: 'instances' })}>
          <ArrowLeft size={13} />
          返回
        </Button>
      }
    >
      {step === 'discover' && <DiscoverStep />}
      {step === 'configure' && <ConfigureStep />}
      {step === 'preview' && <PreviewStep />}
      {step === 'progress' && <ProgressStep />}
    </PageShell>
  )
}

/* ------------------------------ discover ------------------------------ */

function DiscoverStep() {
  const {
    candidates,
    scanState,
    scanError,
    selectedId,
    rescan,
    addHomeManually,
    addExecutableManually,
    select,
    toConfigure,
  } = useAdoptionStore()
  useEffect(() => {
    if (isDesktop) void rescan()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <div className="flex flex-col gap-3">
      {!isDesktop && (
        <EmptyState
          icon={<FolderSearch size={20} />}
          title="桌面端功能"
          description="自动发现与接入需要桌面版 PHL 访问本机文件系统。在浏览器模式（npm run dev）下此流程不可用。"
        />
      )}

      {isDesktop && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <Button variant="secondary" onClick={() => void rescan()} disabled={scanState === 'scanning'}>
              <RefreshCw size={13} className={scanState === 'scanning' ? 'animate-spin' : undefined} />
              {scanState === 'scanning' ? '扫描中…' : '重新扫描'}
            </Button>
            <Button variant="ghost" onClick={() => void addHomeManually()}>
              <FolderSearch size={13} />
              选择 DSH_HOME
            </Button>
            <Button variant="ghost" onClick={() => void addExecutableManually()}>
              <ScanSearch size={13} />
              选择 DSH 可执行文件
            </Button>
          </div>

          {scanError && (
            <Card tone="danger" className="p-3 text-sm text-ink">
              <div className="flex items-start gap-2">
                <AlertTriangle size={14} className="mt-0.5 shrink-0 text-danger" />
                <span>扫描失败：{scanError}</span>
              </div>
            </Card>
          )}

          {scanState === 'scanning' && candidates.length === 0 ? (
            <div className="flex flex-col gap-2">
              {[0, 1].map((i) => (
                <Skeleton key={i} className="h-[92px]" />
              ))}
            </div>
          ) : candidates.length === 0 ? (
            <EmptyState
              icon={<HardDriveDownload size={20} />}
              title="未发现本机 DSH"
              description="没有扫描到已存在的 DSH 环境。如果你的 DSH 不在默认位置，用上方“选择 DSH_HOME”手动指定它。"
            />
          ) : (
            <>
              <p className="text-xs text-ink-faint">
                发现 {candidates.length} 个 DSH 环境
                {candidates.some((c) => c.alreadyManaged) && '（灰色为已被 PHL 管理的实例）'}
              </p>
              {candidates.map((c) => (
                <CandidateRow
                  key={c.id}
                  candidate={c}
                  selected={c.id === selectedId}
                  onSelect={() => select(c.id)}
                />
              ))}
              <div className="mt-1 flex justify-end">
                <Button
                  variant="primary"
                  onClick={() => void toConfigure()}
                  disabled={!selectedId}
                >
                  下一步
                  <ArrowRight size={13} />
                </Button>
              </div>
            </>
          )}
        </>
      )}
    </div>
  )
}

const SOURCE_LABEL: Record<RemoteDshCandidate['source'], string> = {
  env: '环境变量',
  defaultHome: '默认目录',
  path: 'PATH 命令',
  manual: '手动选择',
}

function CandidateRow({
  candidate,
  selected,
  onSelect,
}: {
  candidate: RemoteDshCandidate
  selected: boolean
  onSelect: () => void
}) {
  const dim = candidate.alreadyManaged
  return (
    <Card
      interactive
      tone={selected && !dim ? 'accent' : 'default'}
      className={`p-3 ${dim ? 'opacity-60' : ''}`}
      onClick={() => !dim && onSelect()}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h3 className="truncate text-sm font-semibold text-ink">{candidate.displayName}</h3>
            <Badge tone="neutral">{SOURCE_LABEL[candidate.source]}</Badge>
            {candidate.confidence !== 'high' && <Badge tone="warn">低置信度</Badge>}
          </div>
          <p className="mt-0.5 truncate font-mono text-[11px] text-ink-faint">{candidate.dshHome}</p>
        </div>
        {candidate.alreadyManaged ? (
          <span className="shrink-0 text-xs text-ink-faint">
            {candidate.managedInstance ? `已由「${candidate.managedInstance.name}」管理` : '已被 PHL 管理'}
          </span>
        ) : (
          selected && <Badge tone="accent">已选择</Badge>
        )}
      </div>

      <div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-ink-muted">
        <span>版本 {candidate.detectedVersion ?? '未知'}</span>
        <span>插件 {candidate.pluginCount}</span>
        <span>历史对话 {candidate.sessionCount}</span>
        <span>约 {formatBytes(candidate.sizeBytes)}</span>
      </div>

      {candidate.warnings.length > 0 && (
        <ul className="mt-2 flex flex-col gap-0.5 text-[11px] text-ink-faint">
          {candidate.warnings.map((w, i) => (
            <li key={i}>· {w}</li>
          ))}
        </ul>
      )}
    </Card>
  )
}

/* ------------------------------ configure ----------------------------- */

function ConfigureStep() {
  const name = useAdoptionStore((s) => s.name)
  const mode = useAdoptionStore((s) => s.mode)
  const strategy = useAdoptionStore((s) => s.sessionStrategy)
  const setName = useAdoptionStore((s) => s.setName)
  const setMode = useAdoptionStore((s) => s.setMode)
  const setStrategy = useAdoptionStore((s) => s.setSessionStrategy)
  const selectedDirs = useAdoptionStore((s) => s.selectedSessionDirs)
  const back = useAdoptionStore((s) => s.back)
  const toPreview = useAdoptionStore((s) => s.toPreview)
  const candidate = useAdoptionStore((s) => s.selected())

  if (!candidate) {
    // A reload of a deep hash can drop the transient draft (nothing persists
    // it). Never mutate the store during render — show a way back instead.
    return (
      <EmptyState
        icon={<AlertTriangle size={20} />}
        title="未选择 DSH 环境"
        description="会话状态已丢失，请回到发现步骤重新选择一个环境。"
        action={
          <Button variant="secondary" onClick={back}>
            <ArrowLeft size={13} />
            返回发现
          </Button>
        }
      />
    )
  }
  const copy = mode === 'managed-copy'

  return (
    <div className="flex flex-col gap-3">
      <SectionCard title="实例名称" description="接入后在 PHL 里显示的名字。">
        <Input value={name} onChange={(e) => setName(e.target.value)} placeholder={candidate.displayName} />
      </SectionCard>

      <SectionCard title="接入模式" description="两种模式都让原 DSH 可以继续在 PHL 之外单独使用。">
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          <ModeCard
            active={copy}
            icon={<Copy size={15} />}
            title="复制到 PHL"
            recommended
            description="把环境复制进 PHL 实例目录，原 DSH 完全不变。误操作只损失磁盘。"
            onSelect={() => setMode('managed-copy')}
          />
          <ModeCard
            active={!copy}
            icon={<Link2 size={15} />}
            title="原地接入"
            advanced
            description="PHL 直接管理这个目录：能启动、能看，但快照还原、插件改动等写操作会被禁用。"
            onSelect={() => setMode('external')}
          />
        </div>
        {!copy && (
          <div className="mt-2 flex items-start gap-2 rounded-lg bg-sunken p-2.5 text-xs text-ink-muted">
            <AlertTriangle size={14} className="mt-0.5 shrink-0 text-warn" />
            <span>原地接入会继续使用现有 DSH_HOME，因此现有历史对话仍然存在——PHL 不会、也无法隐藏它。</span>
          </div>
        )}
      </SectionCard>

      {copy && (
        <SectionCard title="历史对话" description="决定是否把源 DSH 的会话数据一起复制进来。">
          <div className="flex flex-col gap-2">
            <label className="flex items-center gap-2 text-sm text-ink">
              <input
                type="radio"
                name="strategy"
                checked={strategy === 'all'}
                onChange={() => setStrategy('all')}
              />
              全部迁移（含 sessions 数据）
            </label>
            <label className="flex items-center gap-2 text-sm text-ink">
              <input
                type="radio"
                name="strategy"
                checked={strategy === 'none'}
                onChange={() => setStrategy('none')}
              />
              不迁移（只复制配置与插件环境）
            </label>
            <label className="flex items-center gap-2 text-sm text-ink">
              <input
                type="radio"
                name="strategy"
                checked={strategy === 'selected'}
                onChange={() => setStrategy('selected')}
              />
              按对话选择迁移（只带走勾选的对话）
            </label>
          </div>
          {strategy === 'selected' && <SessionPicker />}
        </SectionCard>
      )}

      <SectionCard title="将随环境一起接入" description="这些不单独可选——复制保持环境完整可用。">
        <div className="flex flex-col gap-1.5 text-sm text-ink-muted">
          <span className="flex items-center gap-2"><Check size={13} className="text-ok" /> 配置（settings.yaml、profile）</span>
          <span className="flex items-center gap-2"><Check size={13} className="text-ok" /> 插件（{candidate.pluginCount} 个已安装）</span>
          <span className="flex items-center gap-2"><Check size={13} className="text-ok" /> 会话数据（按上方策略）</span>
          <span className="flex items-center gap-2"><X size={13} className="text-ink-faint" /> API Key / 凭据不自动迁移，接入后需重新确认</span>
          <span className="flex items-center gap-2"><X size={13} className="text-ink-faint" /> 不复制工作区（workspace）里的项目文件</span>
        </div>
      </SectionCard>

      <div className="flex justify-between">
        <Button variant="ghost" onClick={back}>
          <ArrowLeft size={13} />
          上一步
        </Button>
        <Button
          variant="primary"
          disabled={copy && strategy === 'selected' && selectedDirs.length === 0}
          onClick={() => void toPreview()}
        >
          预览
          <ArrowRight size={13} />
        </Button>
      </div>
    </div>
  )
}

/* --------------------------- session picker ---------------------------- */

/**
 * The "选择对话" list for the copy-mode wizard. Rows mirror the instance
 * detail's migration panel (id prefix, cwd, date) so a conversation looks
 * the same in both places. Listing happens against the *source* home via
 * `list_adoption_sessions`; a session the user picks but that has since
 * vanished fails the backend preview with a clear message, never silently.
 */
function SessionPicker() {
  const sessions = useAdoptionStore((s) => s.sourceSessions)
  const error = useAdoptionStore((s) => s.sourceSessionsError)
  const selected = useAdoptionStore((s) => s.selectedSessionDirs)
  const toggle = useAdoptionStore((s) => s.toggleSessionDir)
  const reload = useAdoptionStore((s) => s.loadSourceSessions)

  return (
    <div className="mt-2 rounded-lg bg-surface-sunken p-2.5 ring-1 ring-inset ring-line">
      {error && (
        <div className="flex items-center justify-between gap-2 text-xs text-warn">
          <span>读取源对话失败：{error}</span>
          <Button size="sm" variant="ghost" onClick={() => void reload()}>
            <RefreshCw size={12} />
            重试
          </Button>
        </div>
      )}
      {!error && sessions === null && (
        <div className="flex items-center gap-2 text-xs text-ink-faint">
          <Spinner size={13} /> 正在列出历史对话…
        </div>
      )}
      {!error && sessions !== null && sessions.length === 0 && (
        <p className="text-xs text-ink-faint">源目录中没有可迁移的历史对话。</p>
      )}
      {!error && sessions !== null && sessions.length > 0 && (
        <>
          <p className="mb-1 text-xs font-medium text-ink-muted">
            选择对话（{sessions.length}）· 已选 {selected.length}
          </p>
          <div className="max-h-44 space-y-1 overflow-y-auto">
            {sessions.map((s) => (
              <label key={s.sessionDir} className="flex cursor-pointer items-center gap-2 text-sm">
                <Checkbox
                  checked={selected.includes(s.sessionDir)}
                  onChange={() => toggle(s.sessionDir)}
                />
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
        </>
      )}
    </div>
  )
}

function ModeCard({
  active,
  icon,
  title,
  description,
  recommended,
  advanced,
  onSelect,
}: {
  active: boolean
  icon: React.ReactNode
  title: string
  description: string
  recommended?: boolean
  advanced?: boolean
  onSelect: () => void
}) {
  return (
    <Card
      interactive
      tone={active ? 'accent' : 'default'}
      className="p-3"
      onClick={onSelect}
    >
      <div className="flex items-center gap-2">
        <span className="text-ink-muted">{icon}</span>
        <h4 className="text-sm font-semibold text-ink">{title}</h4>
        {recommended && <Badge tone="accent">推荐</Badge>}
        {advanced && <Badge tone="neutral">高级</Badge>}
      </div>
      <p className="mt-1 text-xs leading-relaxed text-ink-muted">{description}</p>
    </Card>
  )
}

/* ------------------------------- preview ------------------------------ */

function PreviewStep() {
  const preview = useAdoptionStore((s) => s.preview)
  const previewState = useAdoptionStore((s) => s.previewState)
  const previewError = useAdoptionStore((s) => s.previewError)
  const back = useAdoptionStore((s) => s.back)
  const commit = useAdoptionStore((s) => s.commit)
  const mode = useAdoptionStore((s) => s.mode)

  if (previewState === 'loading') {
    return <Skeleton className="h-[220px]" />
  }
  if (previewState === 'error') {
    return (
      <Card tone="danger" className="p-4">
        <p className="text-sm text-ink">预览失败：{previewError}</p>
        <Button variant="secondary" className="mt-3" onClick={back}>
          返回修改
        </Button>
      </Card>
    )
  }
  if (!preview) return null

  const copy = preview.mode === 'managed-copy'
  return (
    <div className="flex flex-col gap-3">
      <SectionCard title="接入预览" description={preview.sourceHome}>
        <div className="grid grid-cols-2 gap-3 text-sm">
          <Field label="模式" value={copy ? '复制到 PHL' : '原地接入'} />
          <Field label="DSH 版本" value={preview.detectedVersion ?? '未知'} />
          <Field label="Profile" value={preview.profile} />
          <Field label="插件" value={`${preview.pluginCount} 个`} />
          <Field
            label="历史对话"
            value={
              preview.keepsExistingSessions
                ? '保持现状（原地）'
                : preview.sessionStrategy === 'none'
                  ? '不迁移'
                  : preview.sessionStrategy === 'selected'
                    ? `迁移选定的 ${preview.selectedSessionCount} / ${preview.sessionCount} 条`
                    : `迁移 ${preview.sessionCount} 条`
            }
          />
          <Field label="预计复制" value={copy ? formatBytes(preview.copyBytes) : '0 B（原地）'} />
        </div>
      </SectionCard>

      {preview.warnings.length > 0 && (
        <Card className="p-3">
          <ul className="flex flex-col gap-1 text-xs text-ink-muted">
            {preview.warnings.map((w, i) => (
              <li key={i} className="flex items-start gap-1.5">
                <AlertTriangle size={12} className="mt-0.5 shrink-0 text-warn" />
                {w}
              </li>
            ))}
          </ul>
        </Card>
      )}

      {mode === 'external' && (
        <div className="flex items-start gap-2 rounded-lg bg-sunken p-2.5 text-xs text-ink-muted">
          <AlertTriangle size={14} className="mt-0.5 shrink-0 text-warn" />
          <span>原地接入：删除该实例时只从 PHL 移除登记，绝不删除你的 DSH_HOME。</span>
        </div>
      )}

      <div className="flex justify-between">
        <Button variant="ghost" onClick={back}>
          <ArrowLeft size={13} />
          上一步
        </Button>
        <Button variant="primary" onClick={() => void commit()}>
          {copy ? '复制并接入' : '原地接入'}
        </Button>
      </div>
    </div>
  )
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col">
      <span className="text-xs text-ink-faint">{label}</span>
      <span className="text-sm text-ink">{value}</span>
    </div>
  )
}

/* ------------------------------ progress ------------------------------ */

function ProgressStep() {
  const submitting = useAdoptionStore((s) => s.submitting)
  const submitError = useAdoptionStore((s) => s.submitError)
  const progress = useAdoptionStore((s) => s.progress)
  const mode = useAdoptionStore((s) => s.mode)
  const push = useUIStore((s) => s.push)
  const reset = useAdoptionStore((s) => s.reset)

  // A successful commit flips `submitting` off with no error; the store already
  // admitted + reloaded the instance, so leave the flow for the instances list.
  useEffect(() => {
    if (!submitting && !submitError) {
      const t = setTimeout(() => {
        reset()
        push({ name: 'instances' })
      }, 600)
      return () => clearTimeout(t)
    }
  }, [submitting, submitError, reset, push])

  if (submitError) {
    return (
      <Card tone="danger" className="p-4">
        <p className="text-sm text-ink">接入失败：{submitError}</p>
        <p className="mt-1 text-xs text-ink-faint">已回滚，未留下半个实例。</p>
        <Button variant="secondary" className="mt-3" onClick={() => useAdoptionStore.getState().back()}>
          返回修改
        </Button>
      </Card>
    )
  }

  const copy = mode === 'managed-copy'
  return (
    <Card className="p-5">
      <h3 className="text-sm font-semibold text-ink">
        {copy ? '正在复制环境到 PHL 实例…' : '正在登记原地接入…'}
      </h3>
      <p className="mt-1 text-xs text-ink-muted">
        {copy ? '大环境可能需要一会儿；完成后原 DSH 保持不变。' : '即将完成。'}
      </p>
      <div className="mt-4">
        <ProgressBar value={submitting ? progress : 1} active={submitting} />
      </div>
    </Card>
  )
}
