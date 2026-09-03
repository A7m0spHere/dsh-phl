import { useEffect, useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  Check,
  CloudDownload,
  KeyRound,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
  Unplug,
  X,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { classifyInstance, planAdoption } from '@/lib/apiDiff'
import { useMotion } from '@/lib/motion'
import {
  suggestEnvName,
  suggestProviderName,
  useApiConfigStore,
  useInstanceStore,
  useUIStore,
} from '@/stores'
import type { ApiBinding, ApiDefaultModel, ApiModelRef, ApiProvider } from '@/types'
import {
  Badge,
  Button,
  EmptyState,
  Field,
  Input,
  Notice,
  SectionCard,
  Select,
  SettingRow,
  Skeleton,
  Switch,
  Tooltip,
} from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelDivider, PanelGroup, PanelShell, PanelStat } from '@/components/layout/Panel'
// (PanelItem is not needed here: the panel is informational, no filters)

/* ------------------------------------------------------------------ *
 * helpers
 * ------------------------------------------------------------------ */

const KIND_LABEL: Record<ApiProvider['kind'], string> = {
  official: '官方',
  aggregator: '中转',
  custom: '自定义',
}

function formatSynced(iso?: string): string {
  if (!iso) return '从未同步'
  const d = new Date(iso)
  return `${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

const emptyProviderForm = () => ({
  name: '',
  displayName: '',
  kind: 'aggregator' as ApiProvider['kind'],
  api: 'openai-completions',
  baseURL: '',
  apiKeyEnv: '',
  notes: '',
  models: [] as ApiModelRef[],
  enabled: true,
})

type ProviderForm = ReturnType<typeof emptyProviderForm>

function providerToForm(p: ApiProvider): ProviderForm {
  return {
    name: p.name,
    displayName: p.name,
    kind: p.kind,
    api: p.api ?? '',
    baseURL: p.baseURL ?? '',
    apiKeyEnv: p.apiKeyEnv,
    notes: p.notes ?? '',
    models: p.models,
    enabled: p.enabled,
  }
}

/* ------------------------------------------------------------------ *
 * provider editor (inline, expanding)
 * ------------------------------------------------------------------ */

function ProviderCard({
  provider,
  isDefault,
  usedBy,
  onDelete,
  onEdit,
}: {
  provider: ApiProvider
  isDefault: boolean
  usedBy: number
  onDelete: () => void
  onEdit: (patch: Partial<ApiProvider>) => void
}) {
  const [editing, setEditing] = useState(false)
  const [form, setForm] = useState<ProviderForm>(() => providerToForm(provider))
  const { t, riseItem } = useMotion()

  const patch = (p: Partial<ProviderForm>) => setForm((f) => ({ ...f, ...p }))

  const save = () => {
    const name = (form.name.trim() || suggestProviderName(form.displayName)).toLowerCase()
    onEdit({
      name,
      kind: form.kind,
      notes: form.notes.trim() || undefined,
      api: form.api || undefined,
      baseURL: form.baseURL.trim() || undefined,
      apiKeyEnv: form.apiKeyEnv.trim() || suggestEnvName(name),
      models: form.models.filter((m) => m.id.trim()),
      enabled: form.enabled,
    })
    setEditing(false)
  }

  return (
    <motion.li variants={riseItem} layout="position" transition={t(0.24)}>
      <div
        className={cn(
          'group/row relative overflow-hidden rounded-lg bg-surface px-3.5 py-3 ring-1 ring-inset transition-[box-shadow,background-color] duration-200',
          'ring-line hover:ring-line-strong/70',
        )}
      >
        <div className="flex items-center gap-3">
          <span
            className={cn(
              'flex h-9 w-9 shrink-0 items-center justify-center rounded-lg',
              provider.enabled ? 'bg-accent-soft text-accent-ink' : 'bg-surface-sunken text-ink-faint',
            )}
          >
            <KeyRound size={16} />
          </span>
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-md font-medium text-ink">{provider.name}</span>
              <Badge tone={provider.kind === 'official' ? 'ok' : provider.kind === 'aggregator' ? 'accent' : 'neutral'}>
                {KIND_LABEL[provider.kind]}
              </Badge>
              {isDefault && <Badge tone="accent">默认</Badge>}
              {!provider.enabled && <Badge tone="neutral">已停用</Badge>}
            </div>
            <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-ink-faint">
              <Tooltip content={`密钥由环境变量 ${provider.apiKeyEnv}（或 DSH 凭据）提供，PHL 不存储密钥本身`}>
                <span className="font-mono">{provider.apiKeyEnv}</span>
              </Tooltip>
              <span className="text-ink-faint/50">·</span>
              <span>{provider.baseURL || '默认端点'}</span>
              <span className="text-ink-faint/50">·</span>
              <span>{provider.models.length} 个模型</span>
              {usedBy > 0 && (
                <>
                  <span className="text-ink-faint/50">·</span>
                  <span className="text-accent-ink">{usedBy} 个实例绑定</span>
                </>
              )}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            {!editing ? (
              <>
                <Button
                  size="sm"
                  variant="ghost"
                  className="opacity-0 transition-opacity group-hover/row:opacity-100 focus:opacity-100"
                  onClick={() => setEditing((v) => !v)}
                >
                  <Pencil size={12} />
                  编辑
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="opacity-0 transition-opacity group-hover/row:opacity-100 focus:opacity-100"
                  onClick={onDelete}
                >
                  <Trash2 size={12} />
                </Button>
              </>
            ) : (
              <>
                <Button size="sm" variant="secondary" onClick={() => setEditing(false)}>
                  <X size={12} />
                  取消
                </Button>
                <Button size="sm" variant="primary" onClick={save}>
                  <Check size={12} />
                  保存
                </Button>
              </>
            )}
          </div>
        </div>

        <AnimatePresence initial={false}>
          {editing && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: 'auto', opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={t(0.24)}
              className="overflow-hidden"
            >
              <div className="mt-3 grid grid-cols-2 gap-3 border-t border-line pt-3">
                <Field label="供应商标识（写入 settings.yaml 的键名）">
                  <Input
                    value={form.name}
                    onChange={(e) => patch({ name: e.target.value })}
                    placeholder={suggestProviderName(form.displayName || 'provider')}
                    className="font-mono"
                  />
                </Field>
                <Field label="类型">
                  <Select value={form.kind} onChange={(e) => patch({ kind: e.target.value as ApiProvider['kind'] })}>
                    <option value="official">官方</option>
                    <option value="aggregator">中转</option>
                    <option value="custom">自定义</option>
                  </Select>
                </Field>
                <Field label="协议">
                  <Select value={form.api} onChange={(e) => patch({ api: e.target.value })}>
                    <option value="">DSH 默认</option>
                    <option value="openai-completions">openai-completions</option>
                    <option value="openai-responses">openai-responses</option>
                    <option value="anthropic">anthropic</option>
                  </Select>
                </Field>
                <Field label="Base URL" hint="留空使用 DSH 内置端点">
                  <Input
                    value={form.baseURL}
                    onChange={(e) => patch({ baseURL: e.target.value })}
                    placeholder="https://…/v1"
                    className="font-mono"
                  />
                </Field>
                <Field
                  label="密钥环境变量名"
                  hint="PHL 只记录变量名；密钥留在 DSH 凭据或系统环境中"
                >
                  <Input
                    value={form.apiKeyEnv}
                    onChange={(e) => patch({ apiKeyEnv: e.target.value })}
                    className="font-mono"
                  />
                </Field>
                <Field label="备注">
                  <Input value={form.notes} onChange={(e) => patch({ notes: e.target.value })} />
                </Field>
                <div className="col-span-2">
                  <ModelEditor models={form.models} onChange={(models) => patch({ models })} />
                </div>
                <div className="col-span-2">
                  <SettingRow
                    title="启用"
                    description="停用后此供应商不会下发到继承全局配置的实例"
                    control={<Switch checked={form.enabled} onChange={(v) => patch({ enabled: v })} />}
                  />
                </div>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </motion.li>
  )
}

function ModelEditor({
  models,
  onChange,
}: {
  models: ApiModelRef[]
  onChange: (m: ApiModelRef[]) => void
}) {
  return (
    <div>
      <div className="mb-1.5 flex items-center justify-between">
        <span className="text-sm font-medium text-ink-muted">模型</span>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => onChange([...models, { id: '' }])}
        >
          <Plus size={12} />
          添加
        </Button>
      </div>
      {models.length === 0 ? (
        <div className="rounded-md bg-surface-sunken px-3 py-2 text-sm text-ink-faint ring-1 ring-inset ring-line">
          不填任何模型时，DSH 使用其内置目录
        </div>
      ) : (
        <div className="space-y-1.5">
          {models.map((m, i) => (
            <div key={i} className="flex items-center gap-2">
              <Input
                value={m.id}
                placeholder="模型 id（如 deepseek-v4-flash）"
                className="flex-1 font-mono"
                onChange={(e) =>
                  onChange(models.map((mm, j) => (j === i ? { ...mm, id: e.target.value } : mm)))
                }
              />
              <Input
                value={m.name ?? ''}
                placeholder="显示名"
                className="w-36"
                onChange={(e) =>
                  onChange(models.map((mm, j) => (j === i ? { ...mm, name: e.target.value || undefined } : mm)))
                }
              />
              <Input
                value={m.contextWindow ? String(m.contextWindow) : ''}
                placeholder="上下文"
                className="w-24"
                onChange={(e) =>
                  onChange(
                    models.map((mm, j) =>
                      j === i ? { ...mm, contextWindow: Number(e.target.value) || undefined } : mm,
                    ),
                  )
                }
              />
              <Button size="sm" variant="ghost" onClick={() => onChange(models.filter((_, j) => j !== i))}>
                <X size={12} />
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

/* ------------------------------------------------------------------ *
 * panel
 * ------------------------------------------------------------------ */

export function ApiConfigPanel() {
  const config = useApiConfigStore((s) => s.config)
  const instances = useInstanceStore((s) => s.instances)
  const snapshots = useApiConfigStore((s) => s.snapshots)
  const navigate = useUIStore((s) => s.navigate)

  const managed = instances.filter((i) => i.api && i.api.inheritance !== 'none')
  // Local edits are no longer errors to fix at launch; they are information.
  const dirty = managed.filter((i) => snapshots[i.id]?.localChanges).length

  return (
    <PanelShell>
      <PanelGroup title="配置库">
        <PanelStat label="供应商" value={`${config?.providers.length ?? 0} 个`} />
        <PanelStat label="托管实例" value={`${managed.length} / ${instances.length}`} />
        <PanelStat
          label="有本地改动"
          value={dirty ? `${dirty} 个` : '无'}
        />
      </PanelGroup>
      <PanelDivider />
      <div className="px-2.5">
        <Button
          size="sm"
          variant="secondary"
          className="w-full justify-center"
          onClick={() => navigate({ name: 'apiConfig' })}
        >
          <Plus size={12} />
          管理全局配置
        </Button>
      </div>
      <div className="mt-auto p-3">
        <div className="flex items-start gap-2 rounded-lg bg-surface-sunken p-3 text-sm leading-relaxed text-ink-faint ring-1 ring-inset ring-line">
          <KeyRound size={13} className="mt-[2px] shrink-0" />
          <span>PHL 只保存供应商地址与模型清单；API 密钥留在 DSH 或系统环境变量中，不出现在 PHL 的存储里。</span>
        </div>
      </div>
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ *
 * instance bindings section
 * ------------------------------------------------------------------ */

function InstanceBindingRow({
  instanceId,
  name,
}: {
  instanceId: string
  name: string
}) {
  const config = useApiConfigStore((s) => s.config)
  const snapshot = useApiConfigStore((s) => s.snapshots[instanceId])
  const syncing = useApiConfigStore((s) => s.syncing === instanceId)
  const syncInstance = useApiConfigStore((s) => s.syncInstance)
  const adoptFromInstance = useApiConfigStore((s) => s.adoptFromInstance)
  const instance = useInstanceStore((s) => s.byId(instanceId))
  const updateInstance = useInstanceStore((s) => s.updateInstance)
  const running = useInstanceStore((s) => s.states[instanceId]?.status === 'running')
  const confirm = useUIStore((s) => s.confirm)
  const toast = useUIStore((s) => s.toast)

  if (!instance || !config) return null
  const binding: ApiBinding = instance.api ?? { inheritance: 'none', providerIds: [] }
  const views = classifyInstance(binding, config, snapshot?.live ?? null)
  const localChanges = !!snapshot?.localChanges
  const adoptable = views.some((v) => v.state === 'local-only' || v.state === 'modified')

  const onSync = async () => {
    if (localChanges) {
      const ok = await confirm({
        title: `覆盖「${name}」的本地修改`,
        message: '该实例的托管配置在 DSH 侧有过改动，同步会按全局库重写回去；非托管内容不受影响。',
        confirmLabel: '覆盖并同步',
      })
      if (!ok) return
    }
    const applied = await syncInstance(instanceId, binding)
    if (applied) void useApiConfigStore.getState().refreshSnapshots({ [instanceId]: applied })
  }

  const onAdopt = async () => {
    const plan = planAdoption(binding, config, snapshot?.live ?? null)
    const lines = [
      ...plan.add.map((p) => `新增：${p.name}`),
      ...plan.update.map((u) => `更新：${u.name}`),
    ]
    const ok = await confirm({
      title: `采纳「${name}」到全局库`,
      message: '把实例的实际配置合入全局库；只写库，不改动任何实例文件。',
      detail: lines.join('\n') || undefined,
      confirmLabel: '采纳',
    })
    if (!ok) return
    const applied = await adoptFromInstance(instanceId, { removeMissing: false, updateDefault: true })
    if (applied) {
      toast({ kind: 'success', title: '已采纳到全局库', message: `新增 ${applied.add.length} 项 · 更新 ${applied.update.length} 项` })
    }
  }

  const onTakeover = async () => {
    if (binding.inheritance !== 'none') return
    const next: ApiBinding = { ...binding, inheritance: 'default' }
    updateInstance(instanceId, { api: next })
    const applied = await syncInstance(instanceId, next)
    if (applied) void useApiConfigStore.getState().refreshSnapshots({ [instanceId]: applied })
  }

  return (
    <div className="flex items-center gap-3 border-t border-line px-4 py-2.5 first:border-t-0">
      <span className="min-w-0 flex-1">
        <span className="text-base text-ink">{name}</span>
        <span className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-ink-faint">
          {binding.inheritance === 'none' ? (
            <span className="inline-flex items-center gap-1">
              <Unplug size={11} /> 未托管
            </span>
          ) : (
            <>
              <span>生效 {views.filter((v) => v.state !== 'missing').length} 个</span>
              {views
                .filter((v) => v.state !== 'matched' && v.state !== 'missing')
                .map((v) => (
                  <Badge key={v.provider.name} tone={v.state === 'unbound' ? 'neutral' : 'accent'}>
                    {v.provider.name}
                    {v.state === 'local-only' && ' 新增'}
                    {v.state === 'modified' && ' 改'}
                  </Badge>
                ))}
              <span>· 同步于 {formatSynced(binding.syncedAt)}</span>
              {localChanges && <span className="text-warn-ink">· 有本地改动</span>}
            </>
          )}
        </span>
      </span>
      {snapshot && snapshot.missingKeys.length > 0 && (
        <Tooltip content={`环境变量未设置：${snapshot.missingKeys.join('、')}。密钥需在该实例的 DSH 中配置一次。`}>
          <Badge tone="warn">密钥待配置</Badge>
        </Tooltip>
      )}
      {adoptable && binding.inheritance === 'none' && (
        <Button size="sm" variant="ghost" onClick={() => void onAdopt()}>
          采纳
        </Button>
      )}
      {binding.inheritance === 'none' ? (
        <Button size="sm" variant="secondary" disabled={!config} onClick={() => void onTakeover()}>
          接管本实例
        </Button>
      ) : (
        <>
          {adoptable && (
            <Button size="sm" variant="ghost" onClick={() => void onAdopt()}>
              采纳到库
            </Button>
          )}
          <Button
            size="sm"
            variant={localChanges || !binding.syncedHash ? 'primary' : 'secondary'}
            disabled={running || syncing || !config}
            onClick={() => void onSync()}
          >
            <RefreshCw size={12} className={cn(syncing && 'animate-spin')} />
            {running ? '运行中' : syncing ? '同步中' : localChanges ? '覆盖同步' : '同步'}
          </Button>
        </>
      )}
    </div>
  )
}

/* ------------------------------------------------------------------ *
 * page
 * ------------------------------------------------------------------ */

export function ApiConfigPage() {
  const config = useApiConfigStore((s) => s.config)
  const loaded = useApiConfigStore((s) => s.loaded)
  const saving = useApiConfigStore((s) => s.saving)
  const addProvider = useApiConfigStore((s) => s.addProvider)
  const updateProvider = useApiConfigStore((s) => s.updateProvider)
  const removeProvider = useApiConfigStore((s) => s.removeProvider)
  const setDefaults = useApiConfigStore((s) => s.setDefaults)
  const importFromInstance = useApiConfigStore((s) => s.importFromInstance)
  const refreshSnapshots = useApiConfigStore((s) => s.refreshSnapshots)
  const syncInstance = useApiConfigStore((s) => s.syncInstance)
  const syncing = useApiConfigStore((s) => s.syncing)
  // Subscribed (not getState) so the batch-sync counts re-render as
  // snapshots land — a count frozen at page-load would be a lie by the
  // first 采纳/同步.
  const snapshots = useApiConfigStore((s) => s.snapshots)
  const instanceStates = useInstanceStore((s) => s.states)
  const instances = useInstanceStore((s) => s.instances)
  const confirm = useUIStore((s) => s.confirm)
  const toast = useUIStore((s) => s.toast)
  const { stagger, t } = useMotion()

  const [adding, setAdding] = useState(false)
  const [newForm, setNewForm] = useState<ProviderForm>(emptyProviderForm())
  const [importTarget, setImportTarget] = useState('')

  // Live badges are library-relative too: a provider rename is invisible to
  // `bindingsKey`, so the effect also rides the library's updatedAt stamp —
  // every save (including adoption and provider edits) re-snapshots the list.
  const bindingsKey = useMemo(
    () => instances.map((i) => `${i.id}:${i.api?.inheritance}:${i.api?.syncedHash ?? ''}`).join('|'),
    [instances],
  )
  useEffect(() => {
    if (!loaded || !config || instances.length === 0) return
    void refreshSnapshots(Object.fromEntries(instances.map((i) => [i.id, i.api ?? { inheritance: 'none', providerIds: [] }])))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded, !!config, bindingsKey, config])

  /**
   * Batch sync pushes only the *clean* managed instances (no local edits, not
   * running). Instances whose files carry user edits are deliberately left to
   * the per-row 覆盖同步 — "一键覆盖所有实例" that silently eats 11 instances'
   * tinkering is the failure mode we removed launch-time rewriting for.
   */
  const cleanTargets = instances.filter((i) => {
    const b = i.api
    if (!b || b.inheritance === 'none') return false
    if (instanceStates[i.id]?.status === 'running') return false
    return !snapshots[i.id]?.localChanges
  })

  const onBulkSync = async () => {
    if (cleanTargets.length === 0) return
    // Only instances the push actually skips are worth reporting: running
    // ones are un-syncable by policy and not part of the 覆盖 story.
    const dirtyCount = instances.filter((i) => {
      const b = i.api
      if (!b || b.inheritance === 'none') return false
      if (instanceStates[i.id]?.status === 'running') return false
      return snapshots[i.id]?.localChanges
    }).length
    const ok = await confirm({
      title: `同步 ${cleanTargets.length} 个实例`,
      message:
        dirtyCount > 0
          ? `将把全局库写入 ${cleanTargets.length} 个无本地改动的实例；另有 ${dirtyCount} 个存在本地改动，已跳过——请在它们各自的选择里逐条覆盖。`
          : '将把全局库写入以下所有托管且无本地改动的实例。',
      detail: cleanTargets.map((i) => i.name).join('、'),
      confirmLabel: '同步',
    })
    if (!ok) return
    // syncInstance surfaces its own per-instance error toast and returns
    // null; count the successes so the summary does not over-report.
    let done = 0
    for (const i of cleanTargets) {
      const applied = await syncInstance(i.id, i.api!)
      if (!applied) continue
      done += 1
      await useApiConfigStore.getState().refreshSnapshots({ [i.id]: applied })
    }
    toast(
      done === cleanTargets.length
        ? { kind: 'success', title: `已同步 ${done} 个实例` }
        : {
            kind: 'warn',
            title: `同步完成 ${done} 个，失败 ${cleanTargets.length - done} 个`,
            message: '失败的具体原因见各自弹出的提示。',
          },
    )
  }

  const usedByCount = (providerId: string) =>
    instances.filter((i) => {
      const b = i.api
      if (!b || b.inheritance === 'none') return false
      if (b.inheritance === 'default')
        return !!config?.providers.find((p) => p.id === providerId && p.enabled)
      return b.providerIds.includes(providerId)
    }).length

  const onEditProvider = async (id: string, patch: Partial<ApiProvider>) => {
    const old = config?.providers.find((p) => p.id === id)
    // Renaming the env-var reference orphans the key DSH already stored under
    // the old name — make that choice explicit rather than silent.
    if (
      old &&
      patch.apiKeyEnv &&
      patch.apiKeyEnv !== old.apiKeyEnv &&
      usedByCount(id) > 0
    ) {
      const ok = await confirm({
        title: '更换密钥环境变量',
        message: `${usedByCount(id)} 个已绑定实例此前按 ${old.apiKeyEnv} 解析密钥。改名后需要在 DSH 中为新变量重新配置一次密钥，除非系统环境里已有同名变量。`,
        detail: `${old.apiKeyEnv} → ${patch.apiKeyEnv}`,
        confirmLabel: '仍然更换',
      })
      if (!ok) return
    }
    if (
      patch.name &&
      config?.providers.some((p) => p.id !== id && p.name === patch.name)
    ) {
      toast({ kind: 'error', title: '供应商标识重复', message: `settings.yaml 按名称键控，${patch.name} 已被占用。` })
      return
    }
    await updateProvider(id, patch)
  }

  const onDeleteProvider = async (id: string) => {
    const used = usedByCount(id)
    const ok = await confirm({
      title: '删除供应商',
      message: used
        ? `${used} 个实例绑定了这个供应商。下发采用增量合并：它已写入实例的部分会保留在实例配置里（PHL 不再更新它），如需彻底删除请在该实例的 DSH 中操作。`
        : '将从全局库中移除；已下发到实例的副本保留不变。',
      tone: 'danger',
      confirmLabel: '删除',
    })
    if (ok) await removeProvider(id)
  }

  const onSaveNew = async () => {
    const displayName = newForm.displayName.trim()
    const name = (newForm.name.trim() || suggestProviderName(displayName)).toLowerCase()
    if (!displayName) {
      toast({ kind: 'error', title: '请填写供应商名称' })
      return
    }
    if (config?.providers.some((p) => p.name === name)) {
      toast({ kind: 'error', title: '供应商标识重复', message: `${name} 已存在。` })
      return
    }
    const created = await addProvider({
      name,
      kind: newForm.kind,
      notes: newForm.notes.trim() || undefined,
      api: newForm.api || undefined,
      baseURL: newForm.baseURL.trim() || undefined,
      apiKeyEnv: newForm.apiKeyEnv.trim() || suggestEnvName(name),
      models: newForm.models.filter((m) => m.id.trim()),
      enabled: true,
    })
    if (created) setAdding(false)
  }

  const onImport = async () => {
    if (!importTarget) return
    const seeded = await importFromInstance(importTarget)
    if (seeded) {
      setImportTarget('')
      toast({
        kind: 'success',
        title: '已从实例导入',
        message: `${seeded.providers.length} 个供应商进入全局库。保存后生效——先检查名称与密钥变量。`,
      })
    }
  }

  if (!loaded) {
    return (
      <PageShell title="模型与 API" subtitle="维护一份全局供应商配置，按实例下发到 DSH。">
        <div className="space-y-2">
          {[0, 1].map((i) => (
            <Skeleton key={i} className="h-[92px]" />
          ))}
        </div>
      </PageShell>
    )
  }

  const providerCount = config?.providers.length ?? 0

  return (
    <PageShell
      title="模型与 API"
      subtitle="在这里配置一次，新建实例开箱即用；存量实例可随时同步或接管。密钥由 DSH 管理，PHL 只引用环境变量名。"
      actions={
        providerCount > 0 ? (
          <Button
            size="sm"
            variant="primary"
            disabled={saving}
            onClick={() => setAdding((v) => !v)}
          >
            <Plus size={12} />
            新增供应商
          </Button>
        ) : undefined
      }
    >
      {providerCount === 0 && (
        <Notice tone="info" title="还没有全局配置库">
          <div className="flex flex-wrap items-center gap-2">
            <span>两种起步方式：</span>
            <Button size="sm" variant="primary" onClick={() => setAdding(true)}>
              <Plus size={12} />
              新建供应商
            </Button>
            {instances.length > 0 && (
              <>
                <span>或</span>
                <Select
                  value={importTarget}
                  onChange={(e) => setImportTarget(e.target.value)}
                  className="w-[180px]"
                >
                  <option value="">从实例导入…</option>
                  {instances.map((i) => (
                    <option key={i.id} value={i.id}>
                      {i.name}
                    </option>
                  ))}
                </Select>
                <Button size="sm" variant="secondary" disabled={!importTarget} onClick={() => void onImport()}>
                  <CloudDownload size={12} />
                  导入
                </Button>
              </>
            )}
          </div>
        </Notice>
      )}

      <AnimatePresence initial={false}>
        {adding && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={t(0.24)}
            className="overflow-hidden"
          >
            <SectionCard title="新建供应商" className="mb-4">
              <div className="grid grid-cols-2 gap-3">
                <Field label="名称">
                  <Input
                    value={newForm.displayName}
                    onChange={(e) => setNewForm((f) => ({ ...f, displayName: e.target.value }))}
                    placeholder="如 DeepSeek 官方"
                  />
                </Field>
                <Field label="标识（settings.yaml 键名）" hint={suggestProviderName(newForm.displayName || 'provider')}>
                  <Input
                    value={newForm.name}
                    onChange={(e) => setNewForm((f) => ({ ...f, name: e.target.value }))}
                    placeholder="留空按名称生成"
                    className="font-mono"
                  />
                </Field>
                <Field label="类型">
                  <Select
                    value={newForm.kind}
                    onChange={(e) => setNewForm((f) => ({ ...f, kind: e.target.value as ApiProvider['kind'] }))}
                  >
                    <option value="official">官方</option>
                    <option value="aggregator">中转</option>
                    <option value="custom">自定义</option>
                  </Select>
                </Field>
                <Field label="协议">
                  <Select value={newForm.api} onChange={(e) => setNewForm((f) => ({ ...f, api: e.target.value }))}>
                    <option value="">DSH 默认</option>
                    <option value="openai-completions">openai-completions</option>
                    <option value="openai-responses">openai-responses</option>
                    <option value="anthropic">anthropic</option>
                  </Select>
                </Field>
                <Field label="Base URL" hint="留空使用 DSH 内置端点">
                  <Input
                    value={newForm.baseURL}
                    onChange={(e) => setNewForm((f) => ({ ...f, baseURL: e.target.value }))}
                    placeholder="https://…/v1"
                    className="font-mono"
                  />
                </Field>
                <Field label="密钥环境变量名" hint="PHL 不存密钥，DSH 按此变量解析">
                  <Input
                    value={newForm.apiKeyEnv}
                    onChange={(e) => setNewForm((f) => ({ ...f, apiKeyEnv: e.target.value }))}
                    placeholder={suggestEnvName(newForm.name || newForm.displayName || 'provider')}
                    className="font-mono"
                  />
                </Field>
                <div className="col-span-2">
                  <ModelEditor
                    models={newForm.models}
                    onChange={(models) => setNewForm((f) => ({ ...f, models }))}
                  />
                </div>
                <div className="col-span-2 flex justify-end gap-2">
                  <Button variant="secondary" size="sm" onClick={() => setAdding(false)}>
                    取消
                  </Button>
                  <Button variant="primary" size="sm" disabled={saving} onClick={() => void onSaveNew()}>
                    {saving ? '保存中…' : '保存到全局库'}
                  </Button>
                </div>
              </div>
            </SectionCard>
          </motion.div>
        )}
      </AnimatePresence>

      {providerCount > 0 && (
        <SectionCard title="默认模型" description="下发给所有「继承全局」实例的默认配置" className="mb-4">
          <SettingRow
            title="默认供应商"
            control={
              <Select
                value={config?.defaultProviderId ?? ''}
                onChange={(e) =>
                  void setDefaults(e.target.value || undefined, config?.defaultModel)
                }
                className="w-[220px]"
              >
                <option value="">未选择</option>
                {(config?.providers ?? []).map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </Select>
            }
          />
          <SettingRow
            title="默认模型"
            description="按 供应商/模型 记录；实例详情页可单独覆盖"
            control={
              <DefaultModelPicker
                providers={config?.providers ?? []}
                value={config?.defaultModel}
                onChange={(dm) => void setDefaults(config?.defaultProviderId, dm)}
              />
            }
          />
        </SectionCard>
      )}

      {providerCount > 0 ? (
        <motion.ul variants={stagger()} initial="hidden" animate="show" className="space-y-2">
          {(config?.providers ?? []).map((p) => (
            <ProviderCard
              key={p.id}
              provider={p}
              isDefault={config?.defaultProviderId === p.id}
              usedBy={usedByCount(p.id)}
              onDelete={() => void onDeleteProvider(p.id)}
              onEdit={(patch) => void onEditProvider(p.id, patch)}
            />
          ))}
        </motion.ul>
      ) : (
        !adding && <EmptyState icon={<KeyRound size={20} />} title="全局库为空" description="新建一个供应商，或从已配置好的实例导入。" />
      )}

      {providerCount > 0 && (
        <SectionCard
          title="实例绑定"
          description="展示每个实例当前真实生效的配置。实例内改动会被保留；同步按全局库覆盖（覆盖前有确认），采纳把实例配置合入全局库。"
          className="mt-6"
          extra={
            cleanTargets.length > 0 ? (
              <Button size="xs" variant="secondary" disabled={!!syncing} onClick={() => void onBulkSync()}>
                <RefreshCw size={11} className={cn(syncing && 'animate-spin')} />
                批量同步（{cleanTargets.length}）
              </Button>
            ) : undefined
          }
        >
          {instances.length === 0 ? (
            <div className="px-4 py-3 text-sm text-ink-faint">还没有实例。</div>
          ) : (
            instances.map((i) => (
              <InstanceBindingRow key={i.id} instanceId={i.id} name={i.name} />
            ))
          )}
        </SectionCard>
      )}
    </PageShell>
  )
}

/* ------------------------------------------------------------------ *
 * default-model picker (provider + model + effort)
 * ------------------------------------------------------------------ */

function DefaultModelPicker({
  providers,
  value,
  onChange,
}: {
  providers: ApiProvider[]
  value?: ApiDefaultModel
  onChange: (dm: ApiDefaultModel | undefined) => void
}) {
  const provider = providers.find((p) => p.name === value?.providerName)
  return (
    <div className="flex items-center gap-2">
      <Select
        value={value?.providerName ?? ''}
        onChange={(e) => {
          const p = providers.find((pp) => pp.name === e.target.value)
          onChange(
            p
              ? { providerName: p.name, model: p.models[0]?.id ?? '', reasoningEffort: undefined }
              : undefined,
          )
        }}
        className="w-[160px]"
      >
        <option value="">未选择</option>
        {providers.map((p) => (
          <option key={p.id} value={p.name}>
            {p.name}
          </option>
        ))}
      </Select>
      {provider && (
        <Select
          value={value?.model ?? ''}
          onChange={(e) =>
            value && onChange({ ...value, model: e.target.value })
          }
          className="w-[200px]"
        >
          {provider.models.length === 0 && <option value="">（无模型清单）</option>}
          {provider.models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.name && m.name !== m.id ? `${m.name} (${m.id})` : m.id}
            </option>
          ))}
        </Select>
      )}
    </div>
  )
}

