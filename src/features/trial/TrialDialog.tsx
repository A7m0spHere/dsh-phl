import { useEffect, useMemo, useRef, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Copy, X } from 'lucide-react'
import { useMotion, MODAL_SCRIM, MODAL_Z } from '@/lib/motion'
import { Badge, Button, Notice, Spinner } from '@/components/ui'
import { useCatalogStore, useInstanceStore, useUIStore } from '@/stores'
import { isVersionInstalled } from '@/types/version'
import { formatBytes } from '@/lib/format'
import { repository, Cancelled } from '@/services'
import { parseThrownError } from '@/lib/errorCodes'
import { resolveBoundVersion } from '@/lib/instanceVersion'
import type { TrialOutcome, TrialPreview, TrialRequest } from '@/lib/desktop'
import { useEnvCompareStore } from '@/features/environment/compareStore'
import { useTrialWizardStore } from './trialStore'
import {
  buildTrialCreateRequest,
  defaultTargetVersion,
  isPlanCurrent,
  planFromPreview,
  trialPlanKey,
  type TrialPlanRef,
} from './trialPlan'

type Step = 'configure' | 'progress' | 'result'

const READINESS_LABEL: Record<string, string> = {
  needsDependencies: '存在无法解析的依赖，副本标记为待补依赖',
  needsCredentials: '副本未绑定凭据，启动前需要配置 API',
  readyToLaunch: '副本资源齐备，可以启动',
}

/**
 * 复制并试用新版 (R1 · M3). The backend owns the plan (this dialog only
 * edits the knobs the plan documents) and the commit; the dialog's job is
 * to keep every choice legible — scope, workspace strategy, session cwd
 * warnings — and to say exactly what happened afterwards.
 */
export function TrialDialog() {
  const sourceId = useTrialWizardStore((s) => s.sourceId)
  const presetVersion = useTrialWizardStore((s) => s.presetVersion)
  const close = useTrialWizardStore((s) => s.close)
  const instances = useInstanceStore((s) => s.instances)
  const stop = useInstanceStore((s) => s.stop)
  const reload = useInstanceStore((s) => s.reload)
  const stateOf = useInstanceStore((s) => s.stateOf)
  const versions = useCatalogStore((s) => s.versions)
  const runtimes = useCatalogStore((s) => s.runtimes)
  const { pop, overlay } = useMotion()

  const [step, setStep] = useState<Step>('configure')
  const [preview, setPreview] = useState<TrialPreview | null>(null)
  const [previewError, setPreviewError] = useState<string | null>(null)
  /**
   * The plan the current knobs were previewed for. Cleared the moment a
   * preview starts, fails, or the knobs move — create is only ever enabled
   * with a plan that belongs to the options on screen (R3-01).
   */
  const [plan, setPlan] = useState<TrialPlanRef | null>(null)
  const [name, setName] = useState('')
  const [scope, setScope] = useState<TrialRequest['scope']>('config')
  const [workspace, setWorkspace] = useState<TrialRequest['workspace']>('fresh')
  const [targetVersion, setTargetVersion] = useState('')
  const [targetRuntimeId, setTargetRuntimeId] = useState('')
  /** Bumped after the source stops: the facts changed, so the plan must be re-taken. */
  const [replanNonce, setReplanNonce] = useState(0)
  const [progress, setProgress] = useState<{ progress: number; bytesDone: number; bytesTotal: number } | null>(null)
  const [outcome, setOutcome] = useState<TrialOutcome | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [stopping, setStopping] = useState(false)
  const requestRef = useRef(0)
  /** Kept alive so the progress step's 取消 button can actually abort. */
  const createControllerRef = useRef<AbortController | null>(null)

  /**
   * The dialog stays MOUNTED at App level (only `sourceId` gates the render),
   * so a reopen for a different instance would otherwise inherit the previous
   * run's result page, error, typed name and progress. Every transition
   * null → id resets the whole local state; the preview/default effects that
   * key on `sourceId` then start from a clean slate. A same-source reopen
   * resets too — landing back on the configure step is the honest restart.
   */
  useEffect(() => {
    if (!sourceId) return
    setStep('configure')
    setPreview(null)
    setPreviewError(null)
    setPlan(null)
    setName('')
    setProgress(null)
    setOutcome(null)
    setError(null)
    setStopping(false)
  }, [sourceId])

  /**
   * While open, the wizard owns Escape (it closes itself on Escape except
   * mid-copy) — mirrored into the shared overlay depth so the global go-back
   * hotkey does not ALSO navigate the page behind the dialog on the same
   * keypress.
   */
  const pushOverlay = useUIStore((s) => s.pushOverlay)
  const popOverlay = useUIStore((s) => s.popOverlay)
  useEffect(() => {
    if (!sourceId) return
    pushOverlay()
    return popOverlay
  }, [sourceId, pushOverlay, popOverlay])

  const source = instances.find((i) => i.id === sourceId)
  const runtimeStatus = sourceId ? stateOf(sourceId).status : 'stopped'
  /** Knobs the plan must cover; the source's own runtime is the default. */
  const resolvedRuntimeId = targetRuntimeId || source?.runtimeId || ''
  const planKey = sourceId && targetVersion
    ? trialPlanKey({
        sourceId,
        targetVersion,
        targetRuntimeId: resolvedRuntimeId,
        scope,
        workspace,
      })
    : ''
  const planCurrent = isPlanCurrent(plan, planKey)

  // Installed DSH versions, newest first — the trial never uses "latest",
  // the user (or the plan) names one explicitly.
  const versionOptions = useMemo(
    () =>
      versions
        .filter(isVersionInstalled)
        .filter((v) => !v.pendingPublish)
        .sort((a, b) => b.releasedAt.localeCompare(a.releasedAt)),
    [versions],
  )

  // Default target: the newest installed version that differs from the
  // source's binding — that is what "试用新版" means. `defaultTargetVersion`
  // keeps this in bare-name space (see its doc: the id spelling broke the
  // whole flow).
  //
  // Runs on OPEN only (sourceId/presetVersion), deliberately NOT on catalog
  // churn: the App's periodic version sync changes `versions.length`, and
  // re-running there silently reset the user's picked target version and
  // runtime mid-edit — dropping the plan and disabling create until a fresh
  // preview landed. The options list itself re-renders from `versionOptions`
  // either way; a user holding a knob keeps it.
  useEffect(() => {
    if (!source) return
    const current = resolveBoundVersion(versions, source.versionId)
    setTargetVersion(defaultTargetVersion(versionOptions, current?.id, presetVersion))
    setTargetRuntimeId(source.runtimeId)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sourceId, presetVersion])

  // Re-preview whenever a plan-relevant knob changes. The generation guard
  // keeps a slow older preview from overwriting the newest request, and the
  // plan is dropped for the whole in-flight window: create can never submit
  // "current options + the previous plan" (R3-01). targetRuntimeId is a
  // dependency because it is part of the plan the backend issues; stopping
  // the source bumps replanNonce for the same reason.
  useEffect(() => {
    if (!source || !targetVersion) return
    const request = ++requestRef.current
    setPreview(null)
    setPlan(null)
    setPreviewError(null)
    const key = trialPlanKey({
      sourceId: source.id,
      targetVersion,
      targetRuntimeId: resolvedRuntimeId,
      scope,
      workspace,
    })
    repository
      .previewTrial({
        sourceId: source.id,
        targetVersion,
        targetRuntimeId: resolvedRuntimeId,
        name: '',
        scope,
        workspace,
        // The preview never executes, so it carries no plan back to the
        // backend; these are placeholders for the shared request type.
        planId: '',
        targetId: '',
        sourceFingerprint: '',
      })
      .then((p) => {
        if (requestRef.current !== request) return
        setPreview(p)
        if (!p) {
          setPreviewError('试升级预览仅在桌面端可用')
          return
        }
        const issued = planFromPreview(key, p)
        setPlan(issued)
        if (!issued) setPreviewError('预览未返回可执行计划：请重试或更新 PHL')
        setName((cur) => cur || p.suggestedName || '')
      })
      .catch((err) => {
        if (requestRef.current !== request) return
        setPreviewError(parseThrownError(err).message || '预览失败')
      })
    // name deliberately excluded: editing it must not re-preview.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sourceId, targetVersion, resolvedRuntimeId, scope, workspace, replanNonce])

  useEffect(() => {
    if (!sourceId) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && step !== 'progress') close()
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [sourceId, step, close])

  if (!sourceId) return null
  const sourceName = source?.name ?? sourceId

  const create = async () => {
    if (!source) return
    // The plan gate, in the UI: without a plan that belongs to the options on
    // screen there is nothing the backend would accept (R3-01).
    if (!plan || plan.key !== planKey) {
      setError('预览尚未完成或选项已变更：请等待预览结束后再创建')
      return
    }
    setStep('progress')
    setError(null)
    setProgress({ progress: 0, bytesDone: 0, bytesTotal: 0 })
    const controller = new AbortController()
    createControllerRef.current = controller
    try {
      const result = await repository.createTrial(
        buildTrialCreateRequest(
          {
            sourceId: source.id,
            targetVersion,
            targetRuntimeId: resolvedRuntimeId,
            scope,
            workspace,
          },
          name.trim(),
          plan,
        ),
        (p) => setProgress(p),
        controller.signal,
      )
      if (!result) throw new Error('桌面端不可用')
      setOutcome(result)
      setStep('result')
      // The copy is a new instance and the store has never seen it: without
      // this refresh the result panel's 打开副本 / 比较环境 land on
      // 「实例不存在」 (found by the CDP lane, 2026-09-26). A failed re-read
      // is the same failure mode — surfaced, not left as an unhandled
      // rejection with the copy invisible.
      reload().catch((err) => {
        useUIStore.getState().toast({
          kind: 'error',
          title: '刷新实例列表失败',
          message: `副本已创建，但列表未能刷新：${parseThrownError(err).message}`,
          duration: 8000,
        })
      })
    } catch (err) {
      setStep('configure')
      if (err instanceof Cancelled) {
        useUIStore.getState().toast({ kind: 'info', title: '已取消', message: '原实例未受影响' })
      } else {
        setError(parseThrownError(err).message || '创建副本失败')
      }
    } finally {
      createControllerRef.current = null
    }
  }

  const onCancelCreate = () => {
    createControllerRef.current?.abort()
    setStep('configure')
    setProgress(null)
    useUIStore.getState().toast({ kind: 'info', title: '正在取消…', message: '取消前完成的复制不会保留半个实例' })
  }

  const onStopSource = async () => {
    setStopping(true)
    try {
      await stop(sourceId)
      // The source's facts changed (it was running a moment ago); take a
      // fresh plan instead of creating against the one previewed before.
      setReplanNonce((n) => n + 1)
    } finally {
      setStopping(false)
    }
  }

  return (
    <AnimatePresence>
      <motion.div key="trial" className={`fixed inset-0 ${MODAL_Z} flex items-center justify-center p-8`}>
        <motion.div variants={overlay} initial="hidden" animate="show" exit="out" onClick={step === 'progress' ? undefined : close} className={MODAL_SCRIM} />
        <motion.div
          variants={pop}
          initial="hidden"
          animate="show"
          exit="out"
          role="dialog"
          aria-modal="true"
          aria-label="复制并试用新版"
          tabIndex={-1}
          className="relative flex max-h-[82vh] w-[620px] flex-col overflow-hidden rounded-xl bg-surface-raised shadow-pop ring-1 ring-inset ring-line outline-none"
        >
          <div className="flex items-center gap-2.5 border-b border-line px-4 py-3">
            <Copy size={15} className="text-ink-faint" />
            <h2 className="text-md font-medium text-ink">复制并试用新版</h2>
            <span className="text-sm text-ink-faint">源：{sourceName}</span>
            <Button size="sm" variant="ghost" onClick={close} disabled={step === 'progress'} className="ml-auto" aria-label="关闭">
              <X size={14} />
            </Button>
          </div>

          <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-4 py-3">
            {step === 'configure' && (
              <>
                <label className="block space-y-1">
                  <span className="text-sm text-ink-muted">副本名称</span>
                  <input
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder={preview?.suggestedName}
                    className="w-full rounded-md bg-surface-sunken px-2.5 py-1.5 text-ink ring-1 ring-inset ring-line-strong/60"
                  />
                </label>

                <label className="block space-y-1">
                  <span className="text-sm text-ink-muted">目标 DSH 版本</span>
                  <select
                    value={targetVersion}
                    onChange={(e) => setTargetVersion(e.target.value)}
                    className="w-full rounded-md bg-surface-sunken px-2.5 py-1.5 text-ink ring-1 ring-inset ring-line-strong/60"
                  >
                    {versionOptions.length === 0 && <option value="">（没有已安装的版本）</option>}
                    {versionOptions.map((v) => (
                      <option key={v.id} value={v.name}>
                        {v.name}
                      </option>
                    ))}
                  </select>
                </label>

                <label className="block space-y-1">
                  <span className="text-sm text-ink-muted">目标 Runtime</span>
                  <select
                    value={targetRuntimeId}
                    onChange={(e) => setTargetRuntimeId(e.target.value)}
                    className="w-full rounded-md bg-surface-sunken px-2.5 py-1.5 text-ink ring-1 ring-inset ring-line-strong/60"
                  >
                    {runtimes
                      .filter((r) => r.state.kind === 'installed')
                      .map((r) => (
                        <option key={r.id} value={r.id}>
                          {r.name} · v{r.version}
                        </option>
                      ))}
                  </select>
                </label>

                <div className="grid grid-cols-2 gap-3">
                  <div className="space-y-1">
                    <span className="text-sm text-ink-muted">复制范围</span>
                    <select
                      value={scope}
                      onChange={(e) => setScope(e.target.value as TrialRequest['scope'])}
                      className="w-full rounded-md bg-surface-sunken px-2.5 py-1.5 text-ink ring-1 ring-inset ring-line-strong/60"
                    >
                      <option value="config">环境配置（不含会话）</option>
                      <option value="config+sessions">环境＋会话</option>
                    </select>
                  </div>
                  <div className="space-y-1">
                    <span className="text-sm text-ink-muted">工作目录</span>
                    <select
                      value={workspace}
                      onChange={(e) => setWorkspace(e.target.value as TrialRequest['workspace'])}
                      className="w-full rounded-md bg-surface-sunken px-2.5 py-1.5 text-ink ring-1 ring-inset ring-line-strong/60"
                    >
                      <option value="fresh">新建副本专用空目录</option>
                      <option value="shared">使用原工作目录（两实例共写）</option>
                    </select>
                  </div>
                </div>

                {previewError && <Notice tone="danger">{previewError}</Notice>}

                {!preview && !previewError && (
                  <p className="flex items-center gap-2 text-sm text-ink-faint">
                    <Spinner size={13} /> 正在计算预览与计划…
                  </p>
                )}

                {preview && (
                  <div className="space-y-2 rounded-lg bg-surface-sunken/60 p-3 ring-1 ring-inset ring-line">
                    <p className="text-sm text-ink-muted">
                      复制配置与插件到独立 HOME，绑定 <span className="font-mono">dsh-{preview.targetVersion}</span>
                      {preview.targetInstalled ? '（已安装）' : '（未安装）'}，Runtime {preview.targetRuntimeId}
                      {preview.targetRuntimeInstalled ? '（已安装）' : '（未安装）'}。预计 {formatBytes(preview.estimatedBytes)}。
                    </p>
                    {preview.pendingDownloads.length > 0 && (
                      <Notice tone="warn">
                        请先通过对应页面安装：{preview.pendingDownloads.join('；')}
                      </Notice>
                    )}
                    {preview.conflicts.length > 0 && (
                      <Notice tone="warn">已知插件冲突：{preview.conflicts.join('；')}</Notice>
                    )}
                    {preview.mismatchedPackages?.length > 0 && (
                      <Notice tone="info">
                        <p className="font-medium">
                          {preview.mismatchedPackages.length} 个 profile 内实体插件与目标版本不一致：
                        </p>
                        <p className="mt-1">
                          创建时会自动处理：目标版本自带的包替换为目标版本，目标版本不再携带的包按
                          profile 原样保留（见各条说明）。源实例不受影响。
                        </p>
                        <ul className="mt-1 list-disc pl-5">
                          {preview.mismatchedPackages.slice(0, 4).map((m) => (
                            <li key={m} className="break-all">{m}</li>
                          ))}
                          {preview.mismatchedPackages.length > 4 && (
                            <li>……等 {preview.mismatchedPackages.length} 项</li>
                          )}
                        </ul>
                      </Notice>
                    )}
                    {preview.sessionCount !== null && (
                      <Notice tone={preview.sessionCwds.length ? 'warn' : 'info'}>
                        将携带 {preview.sessionCount} 个会话。
                        {preview.sessionCwds.length > 0 &&
                          `会话引用原工作目录（如 ${preview.sessionCwds[0]}）；副本不会自动执行任何任务。`}
                      </Notice>
                    )}
                    {workspace === 'shared' && (
                      <Notice tone="warn">两个实例会操作同一批项目文件，请确认这是你想要的。</Notice>
                    )}
                    {preview.blocked.map((b) => (
                      <Notice key={b} tone="danger">{b}</Notice>
                    ))}
                  </div>
                )}

                {runtimeStatus === 'running' && (
                  <Notice tone="warn">
                    <div className="flex items-center gap-2">
                      <span className="flex-1">源实例正在运行：开始复制前必须停止它。</span>
                      <Button size="sm" variant="secondary" disabled={stopping} onClick={() => void onStopSource()}>
                        {stopping ? <Spinner size={12} weight={2.6} /> : null}
                        停止原实例
                      </Button>
                    </div>
                  </Notice>
                )}
                {error && <Notice tone="danger">{error}</Notice>}
              </>
            )}

            {step === 'progress' && (
              <div className="space-y-2 py-6">
                <p className="flex items-center gap-2 text-sm text-ink-muted">
                  <Spinner size={14} /> 正在复制环境（源已锁定，复制完成前不会修改）…
                </p>
                <div className="h-1.5 overflow-hidden rounded-full bg-surface-sunken">
                  <div
                    className="h-full bg-accent transition-[width] duration-200"
                    style={{ width: `${Math.round((progress?.progress ?? 0) * 100)}%` }}
                  />
                </div>
                <p className="text-sm text-ink-faint">
                  {formatBytes(progress?.bytesDone ?? 0)}{progress?.bytesTotal ? ` / ${formatBytes(progress.bytesTotal)}` : ''}
                </p>
                <div className="flex justify-end">
                  <Button variant="secondary" onClick={onCancelCreate}>
                    取消
                  </Button>
                </div>
              </div>
            )}

            {step === 'result' && outcome && (
              <div className="space-y-3">
                <Notice tone="info">副本已创建：「{outcome.record.name}」</Notice>
                <ul className="space-y-1 text-sm text-ink-muted">
                  <li>
                    <Badge tone={outcome.readiness === 'readyToLaunch' ? 'ok' : 'warn'}>状态</Badge>{' '}
                    {READINESS_LABEL[outcome.readiness] ?? outcome.readiness}
                  </li>
                  <li>内部链接重定向 {outcome.linkRedirects} 个，已验证指向新版本。</li>
                  {outcome.linkFailures.length > 0 && (
                    <li className="text-warn">
                      {outcome.linkFailures.length} 个链接在新版本下无法解析（副本标记为待补依赖）：
                      <ul className="mt-1 list-disc pl-5">
                        {outcome.linkFailures.slice(0, 5).map((f) => (
                          <li key={f} className="break-all">{f}</li>
                        ))}
                      </ul>
                    </li>
                  )}
                  {outcome.mismatchedPackages?.length > 0 && (
                    <li className="text-warn">
                      {outcome.mismatchedPackages.length} 个 profile 内实体插件与目标版本不一致的处理（已在副本内替换或保留，见各条说明；启动报错时按指引重装）：
                      <ul className="mt-1 list-disc pl-5">
                        {outcome.mismatchedPackages.slice(0, 5).map((m) => (
                          <li key={m} className="break-all">{m}</li>
                        ))}
                        {outcome.mismatchedPackages.length > 5 && (
                          <li>……等 {outcome.mismatchedPackages.length} 项</li>
                        )}
                      </ul>
                    </li>
                  )}
                  {outcome.sessionsImported > 0 && <li>已携带 {outcome.sessionsImported} 个会话。</li>}
                  {outcome.notes.map((n) => (
                    <li key={n}>{n}</li>
                  ))}
                  <li className="text-ink-faint">原实例未被修改；插件加载与实际任务在启动后仍未验证。</li>
                </ul>
              </div>
            )}
          </div>

          <div className="flex items-center gap-2 border-t border-line px-4 py-3">
            {step === 'configure' && (
              <>
                <span className="text-xs text-ink-faint">复制过程中源实例保持锁定</span>
                <div className="ml-auto flex gap-2">
                  <Button variant="ghost" onClick={close}>取消</Button>
                  <Button
                    variant="primary"
                    disabled={
                      !preview ||
                      !planCurrent ||
                      preview.pendingDownloads.length > 0 ||
                      preview.blocked.length > 0 ||
                      runtimeStatus === 'running' ||
                      runtimeStatus === 'starting'
                    }
                    onClick={() => void create()}
                  >
                    创建副本
                  </Button>
                </div>
              </>
            )}
            {step === 'result' && outcome && (
              <div className="ml-auto flex gap-2">
                <Button variant="ghost" onClick={close}>关闭</Button>
                <Button
                  variant="secondary"
                  onClick={() => {
                    useEnvCompareStore.getState().open(sourceId, outcome.record.id)
                    close()
                  }}
                >
                  比较环境
                </Button>
                <Button
                  variant="primary"
                  onClick={() => {
                    useUIStore.getState().push({ name: 'instance', id: outcome.record.id })
                    close()
                  }}
                >
                  打开副本
                </Button>
              </div>
            )}
          </div>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  )
}
