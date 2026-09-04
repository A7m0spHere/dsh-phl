import { useEffect, useMemo, useState } from 'react'
import { Check, KeyRound, Pencil, Plus, RefreshCw, Unplug, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { classifyInstance, planAdoption } from '@/lib/apiDiff'
import { useApiConfigStore, useInstanceStore, useUIStore } from '@/stores'
import type { ApiBinding, ApiInheritance, Instance, ProviderLiveView, ProviderState } from '@/types'
import {
  Badge,
  Button,
  Field,
  Notice,
  SectionCard,
  Select,
  Tooltip,
} from '@/components/ui'
import { Segmented } from '@/components/ui/Segmented'

const STATE_LABEL: Record<ProviderState, string> = {
  matched: '一致',
  modified: '已改',
  missing: '已删',
  'local-only': '本地新增',
  unbound: '未绑定',
}

const STATE_TONE: Record<ProviderState, 'ok' | 'warn' | 'accent' | 'neutral'> = {
  matched: 'ok',
  modified: 'warn',
  missing: 'warn',
  'local-only': 'accent',
  unbound: 'neutral',
}

/** Compact provider pill with its live-vs-library state. */
function ProviderPill({ view }: { view: ProviderLiveView }) {
  const tone = STATE_TONE[view.state]
  const hint =
    view.state === 'modified'
      ? `实例内已修改：${view.diff.join('、')}。同步会按全局库覆盖。`
      : view.state === 'missing'
        ? '该供应商已被实例自行移除。同步会写回。'
        : view.state === 'local-only'
          ? '实例内自建的供应商，不在全局库中。可通过「采纳」加入全局库。'
          : view.state === 'unbound'
            ? '存在于全局库，但此实例当前未绑定。'
            : null
  const badge = (
    <Badge tone={view.state === 'matched' ? undefined : tone}>
      {view.provider.name}
      {view.state !== 'matched' && ` · ${STATE_LABEL[view.state]}`}
    </Badge>
  )
  // Only hintable states get the wrapper — an undefined-content Tooltip
  // would paint an empty bubble on hover. Pills live inside overflow-hidden
  // cards, so the wide hints go through the viewport portal.
  if (!hint) return badge
  return (
    <Tooltip content={hint} allowOverflow>
      {badge}
    </Tooltip>
  )
}

/**
 * "模型与 API" card. The read view shows the instance's ACTUAL effective
 * config (from its settings.yaml), not what PHL last pushed — the file is
 * the truth. 同步 overwrites it from the library (with an explicit
 * confirmation when local changes exist); 采纳 folds it back into the
 * library. Both directions are human decisions.
 */
export function ApiBindingCard({ instance }: { instance: Instance }) {
  const config = useApiConfigStore((s) => s.config)
  const loaded = useApiConfigStore((s) => s.loaded)
  const syncing = useApiConfigStore((s) => s.syncing) === instance.id
  const snapshot = useApiConfigStore((s) => s.snapshots[instance.id])
  const syncInstance = useApiConfigStore((s) => s.syncInstance)
  const refreshSnapshots = useApiConfigStore((s) => s.refreshSnapshots)
  const adoptFromInstance = useApiConfigStore((s) => s.adoptFromInstance)
  const updateInstance = useInstanceStore((s) => s.updateInstance)
  const navigate = useUIStore((s) => s.navigate)
  const confirm = useUIStore((s) => s.confirm)
  const toast = useUIStore((s) => s.toast)

  const running = useInstanceStore((s) => s.states[instance.id]?.status) === 'running'

  const binding: ApiBinding = instance.api ?? { inheritance: 'none', providerIds: [] }
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState<ApiBinding>(binding)

  // Keyed on `instance.api` itself: `binding` is a fresh object every render
  // when the manifest has no api field, and depending on it would loop.
  useEffect(() => setDraft(instance.api ?? { inheritance: 'none', providerIds: [] }), [instance.api])

  // Pull the live file on mount and whenever the library changes — the
  // badges are library-relative (a provider rename must re-classify), and
  // every save bumps updatedAt.
  useEffect(() => {
    if (loaded && config) void refreshSnapshots({ [instance.id]: binding })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded, config])

  const views = useMemo(
    () => (config ? classifyInstance(binding, config, snapshot?.live ?? null) : []),
    [config, binding, snapshot?.live],
  )
  const localChanges = !!snapshot?.localChanges
  const hasLocalAdds = views.some((v) => v.state === 'local-only')
  const neverSynced = binding.inheritance !== 'none' && !binding.syncedHash

  const onApply = async () => {
    setEditing(false)
    const next = draft
    updateInstance(instance.id, { api: next })
    if (next.inheritance !== 'none' && config) {
      const applied = await syncInstance(instance.id, next)
      if (applied) {
        void refreshSnapshots({ [instance.id]: applied })
      } else {
        // The binding saved but the push failed (or raced) — say so; the
        // launch path will retry materialization, but only for instances
        // with no file at all, so a stale-but-present file needs a manual
        // sync and the user should know that's the state they are in.
        toast({
          kind: 'warn',
          title: '绑定已保存，下发未成功',
          message: '实例内设置尚未按新绑定更新，可点击「同步」重试。',
        })
      }
    }
  }

  const onSync = async () => {
    if (binding.inheritance === 'none' || !config) return
    // The push overwrites managed content — make what it eats explicit.
    if (localChanges) {
      const lines = [
        ...views
          .filter((v) => v.state === 'modified')
          .map((v) => `已改：${v.provider.name}（${v.diff.join('、')}）`),
        ...views.filter((v) => v.state === 'missing').map((v) => `已删：${v.provider.name}（将被写回）`),
        snapshot?.defaultModelChanged ? '默认模型：实例内改动将被写回' : '',
      ].filter(Boolean)
      const ok = await confirm({
        title: '用全局配置覆盖实例内修改',
        message: '这个实例的托管配置在 DSH 侧有过改动，同步会按全局库把它们重写回去（非托管的其他配置不受影响）。',
        detail:
          lines.join('\n') || '托管内容有改动（可能包含 PHL 不识别的字段），具体无法枚举；覆盖后以全局库为准。',
        confirmLabel: '覆盖并同步',
      })
      if (!ok) return
    }
    const applied = await syncInstance(instance.id, binding)
    if (applied) void refreshSnapshots({ [instance.id]: applied })
  }

  const onAdopt = async () => {
    if (!config) return
    const plan = planAdoption(binding, config, snapshot?.live ?? null)
    const lines = [
      ...plan.add.map((p) => `新增：${p.name}（${p.models.length} 个模型）`),
      ...plan.update.map((u) => `更新：${u.name}`),
      ...plan.removeMissing.map((r) => `实例已删除：${r.name}（可选是否也从库中移除）`),
      plan.defaultModel
        ? `默认模型：${plan.defaultModel.providerName}/${plan.defaultModel.model}`
        : '',
    ].filter(Boolean)
    const ok = await confirm({
      title: '采纳到全局库',
      message: '把该实例的实际配置合入全局库。采纳只写库，不会改动任何实例的文件。',
      detail: lines.join('\n'),
      confirmLabel: '采纳',
    })
    if (!ok) return
    // Deletions inside one instance are not automatically facts about the
    // library; only opt in here.
    const applied = await adoptFromInstance(instance.id, {
      removeMissing: false,
      updateDefault: true,
    })
    if (applied) {
      toast({
        kind: 'success',
        title: '已采纳到全局库',
        message: `新增 ${applied.add.length} 项 · 更新 ${applied.update.length} 项`,
      })
      void refreshSnapshots({ [instance.id]: binding })
    }
  }

  const hasAdoptable = hasLocalAdds || views.some((v) => v.state === 'modified')

  return (
    <SectionCard
      title="模型与 API"
      icon={<KeyRound size={14} />}
      description={
        binding.inheritance === 'none'
          ? '未托管：配置完全由该实例的 DSH 管理'
          : neverSynced
            ? '已绑定全局配置，尚未同步'
            : `同步于 ${binding.syncedAt ? new Date(binding.syncedAt).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '—'}`
      }
      collapsible
      defaultOpen
      extra={
        !editing && (
          <Button size="xs" variant="ghost" onClick={() => setEditing((v) => !v)}>
            <Pencil size={11} />
            编辑
          </Button>
        )
      }
    >
      {!loaded ? null : !config ? (
        <Notice tone="info" title="还没有全局配置库">
          <Button size="sm" variant="secondary" onClick={() => navigate({ name: 'apiConfig' })}>
            去创建
          </Button>
        </Notice>
      ) : !editing ? (
        <div className="space-y-2.5">
          {views.length > 0 && (
            <div className="flex flex-wrap gap-1.5">
              {views.map((v) => (
                <ProviderPill key={`${v.provider.name}:${v.state}`} view={v} />
              ))}
            </div>
          )}
          {binding.inheritance === 'none' && (
            <div className="flex items-center gap-2">
              <Button
                size="sm"
                variant="secondary"
                onClick={() => {
                  setDraft({ ...binding, inheritance: 'default' })
                  setEditing(true)
                }}
              >
                接管本实例
              </Button>
              {snapshot?.live && hasAdoptable && (
                <Button size="sm" variant="ghost" onClick={() => void onAdopt()}>
                  <Plus size={12} />
                  采纳其配置到全局库
                </Button>
              )}
            </div>
          )}
          {binding.inheritance !== 'none' && (
            <div className="flex flex-wrap items-center gap-2">
              <Button
                size="sm"
                variant={localChanges || neverSynced ? 'primary' : 'secondary'}
                disabled={running || syncing}
                onClick={() => void onSync()}
              >
                <RefreshCw size={12} className={cn(syncing && 'animate-spin')} />
                {running ? '停止后可同步' : syncing ? '同步中…' : localChanges ? '覆盖同步' : '同步'}
              </Button>
              {hasAdoptable && (
                <Button size="sm" variant="ghost" onClick={() => void onAdopt()}>
                  <Plus size={12} />
                  采纳到全局库
                </Button>
              )}
              {snapshot && snapshot.missingKeys.length > 0 && (
                <Tooltip allowOverflow content={`环境变量未设置：${snapshot.missingKeys.join('、')}`}>
                  <Badge tone="warn">密钥待配置</Badge>
                </Tooltip>
              )}
              {localChanges && (
                <Tooltip allowOverflow content="实例内改动会被保留，直到你点同步——PHL 不再自动重写。">
                  <Badge tone="warn">
                    <Unplug size={10} />
                    本地改动
                  </Badge>
                </Tooltip>
              )}
            </div>
          )}
        </div>
      ) : (
        <div className="space-y-3">
          <Field label="配置来源">
            <Segmented
              size="sm"
              value={draft.inheritance}
              onChange={(v) => setDraft((d) => ({ ...d, inheritance: v as ApiInheritance }))}
              options={[
                { value: 'default', label: '继承全局' },
                { value: 'custom', label: '自选供应商' },
                { value: 'none', label: '不托管' },
              ]}
            />
          </Field>

          {draft.inheritance === 'custom' && (
            <Field label="供应商" hint="勾选要下发给这个实例的供应商">
              <div className="space-y-1 rounded-md bg-surface-sunken p-2 ring-1 ring-inset ring-line">
                {config.providers.map((p) => (
                  <label key={p.id} className="flex cursor-pointer items-center gap-2 py-0.5 text-base">
                    <input
                      type="checkbox"
                      className="accent-accent"
                      checked={draft.providerIds.includes(p.id)}
                      onChange={(e) =>
                        setDraft((d) => ({
                          ...d,
                          providerIds: e.target.checked
                            ? [...d.providerIds, p.id]
                            : d.providerIds.filter((id) => id !== p.id),
                        }))
                      }
                    />
                    <span className="font-mono">{p.name}</span>
                    {!p.enabled && <Badge tone="neutral">已停用</Badge>}
                    <span className="text-sm text-ink-faint">{p.models.length} 个模型</span>
                  </label>
                ))}
                {config.providers.length === 0 && (
                  <div className="py-1 text-sm text-ink-faint">全局库还没有供应商。</div>
                )}
              </div>
            </Field>
          )}

          {draft.inheritance !== 'none' && (
            <Field label="默认模型" hint="留空使用全局默认">
              <div className="flex items-center gap-2">
                <Select
                  value={draft.defaultModel?.providerName ?? ''}
                  onChange={(e) => {
                    const p = config.providers.find((pp) => pp.name === e.target.value)
                    setDraft((d) => ({
                      ...d,
                      defaultModel: p
                        ? { providerName: p.name, model: p.models[0]?.id ?? '', reasoningEffort: undefined }
                        : undefined,
                    }))
                  }}
                  className="w-[150px]"
                >
                  <option value="">全局默认</option>
                  {config.providers.map((p) => (
                    <option key={p.id} value={p.name}>
                      {p.name}
                    </option>
                  ))}
                </Select>
                {draft.defaultModel && (
                  <Select
                    value={draft.defaultModel.model}
                    onChange={(e) =>
                      setDraft((d) => ({
                        ...d,
                        defaultModel: d.defaultModel && { ...d.defaultModel, model: e.target.value },
                      }))
                    }
                    className="w-[200px]"
                  >
                    {(config.providers.find((p) => p.name === draft.defaultModel?.providerName)?.models ?? []).map(
                      (m) => (
                        <option key={m.id} value={m.id}>
                          {m.name && m.name !== m.id ? `${m.name} (${m.id})` : m.id}
                        </option>
                      ),
                    )}
                  </Select>
                )}
              </div>
            </Field>
          )}

          <div className="flex justify-end gap-2">
            <Button size="sm" variant="secondary" onClick={() => { setDraft(binding); setEditing(false) }}>
              <X size={12} />
              取消
            </Button>
            <Button size="sm" variant="primary" onClick={() => void onApply()}>
              <Check size={12} />
              应用
            </Button>
          </div>
        </div>
      )}
    </SectionCard>
  )
}
