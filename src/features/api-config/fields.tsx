/* Form fields and the provider form machinery (Batch F, T-105). */

import { useState, type ReactNode } from 'react'
import { motion } from 'motion/react'
import { Check, Eye, EyeOff, Plus, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { VENDOR_PRESETS, type VendorPreset } from '@/data/vendorPresets'
import { useMotion } from '@/lib/motion'
import { suggestEnvName, suggestProviderName, useApiConfigStore, useUIStore } from '@/stores'
import { ApiModelRef, ApiProvider } from '@/types'
import { Button, Field, Input, SettingRow, Switch } from '@/components/ui'
import { ModelEditor } from './ModelEditor'

export const ENV_MISSING_PREFIX = 'ENV_MISSING:'

/* ------------------------------------------------------------------ *
 * helpers
 * ------------------------------------------------------------------ */

export const KIND_LABEL: Record<ApiProvider['kind'], string> = {
  official: '官方',
  aggregator: '中转',
  custom: '自定义',
}

/** Protocol options. The `api:` value is written verbatim into settings.yaml
 * and passed through unvalidated, so this is a convenience list, not an
 * exhaustive enum: only `openai-completions` is *observed* in a real
 * settings.yaml on this machine; the others are standard wire names that DSH
 * is expected to honour but not yet verified here. "留空" omits the field and
 * lets DSH apply its own default (providers in the wild ship without `api`). */
const API_CHOICES: { value: string; label: string }[] = [
  { value: 'openai-completions', label: 'openai-completions' },
  { value: 'openai-responses', label: 'openai-responses' },
  { value: 'anthropic', label: 'anthropic' },
  { value: '', label: '留空 · DSH 默认' },
]

export function formatSynced(iso?: string): string {
  if (!iso) return '从未同步'
  const d = new Date(iso)
  return `${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/**
 * The wizard's choice-button language (see CreateInstancePage's 用途 grid):
 * a short enum picks as inline buttons, not a native dropdown. Native
 * Select stays for long lists (provider/model pickers), same as Settings.
 */
export function ChoiceGrid<T extends string>({
  options,
  value,
  onChange,
  cols = 3,
}: {
  options: { value: T; label: string }[]
  value: T
  onChange: (v: T) => void
  cols?: 2 | 3 | 4
}) {
  return (
    <div className={cn('grid gap-1.5', cols === 4 ? 'grid-cols-4' : cols === 3 ? 'grid-cols-3' : 'grid-cols-2')}>
      {options.map((o) => {
        const active = o.value === value
        return (
          <button
            key={o.value || 'default'}
            type="button"
            onClick={() => onChange(o.value)}
            className={cn(
              'rounded-md py-1.5 text-center text-sm ring-1 ring-inset transition-all duration-150',
              active
                ? 'bg-accent-soft font-medium text-accent-ink ring-accent'
                : 'bg-surface text-ink-muted ring-line hover:ring-line-strong',
            )}
          >
            {o.label}
          </button>
        )
      })}
    </div>
  )
}

/**
 * The API key box. cc-switch's arrangement, stated honestly: paste the real
 * key and PHL keeps it in its local config library (a plaintext file — same
 * exposure as cc-switch's SQLite or Cherry's DB; this is a personal desktop
 * tool, not a secret manager). At instance launch it is injected into the
 * DSH child process as the provider's env var; instance directories never
 * contain it. Leaving it empty is a valid choice when the variable already
 * exists in the system env or in DSH's own credentials.
 */
export function ApiKeyField({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const [show, setShow] = useState(false)
  return (
    <Field
      label="API Key"
      hint={
        value.trim()
          ? '将保存在 PHL 本地配置中，实例启动时自动注入 DSH 环境；系统环境变量若存在同名值则优先'
          : '可选：直接粘贴密钥；若已在系统环境或 DSH 凭据中配置，留空即可'
      }
    >
      <div className="flex items-center gap-1.5">
        <Input
          type={show ? 'text' : 'password'}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="sk-…"
          className="flex-1 font-mono"
          autoComplete="off"
          spellCheck={false}
        />
        <Button
          type="button"
          size="sm"
          variant="ghost"
          onClick={() => setShow((v) => !v)}
          aria-label={show ? '隐藏密钥' : '显示密钥'}
        >
          {show ? <EyeOff size={13} /> : <Eye size={13} />}
        </Button>
      </div>
    </Field>
  )
}

export const emptyProviderForm = () => ({
  name: '',
  displayName: '',
  // The type selector was removed (a manual entry *is* the custom kind;
  // presets carry their own). A preset pick overwrites this.
  kind: 'custom' as ApiProvider['kind'],
  api: 'openai-completions',
  baseURL: '',
  apiKey: '',
  apiKeyEnv: '',
  notes: '',
  models: [] as ApiModelRef[],
  enabled: true,
})

export type ProviderForm = ReturnType<typeof emptyProviderForm>

export function presetToForm(p: VendorPreset): ProviderForm {
  return {
    ...emptyProviderForm(),
    name: p.providerName,
    displayName: p.providerName,
    kind: p.kind,
    api: p.api,
    baseURL: p.baseURL,
    apiKeyEnv: p.apiKeyEnv,
    notes: p.notes,
  }
}

export function providerToForm(p: ApiProvider): ProviderForm {
  return {
    name: p.name,
    displayName: p.name,
    kind: p.kind,
    api: p.api ?? '',
    baseURL: p.baseURL ?? '',
    apiKey: p.apiKey ?? '',
    apiKeyEnv: p.apiKeyEnv,
    notes: p.notes ?? '',
    models: p.models,
    enabled: p.enabled,
  }
}

/** The field body shared by the new-provider card and the row editor. */
export function ProviderFields({
  form,
  patch,
  withEnabled,
  /** The edited provider's id — keys the OS credential manager lookup. */
  providerId,
}: {
  form: ProviderForm
  patch: (p: Partial<ProviderForm>) => void
  withEnabled?: boolean
  providerId?: string
}) {
  return (
    <div className="grid grid-cols-2 gap-x-3 gap-y-3">
      <Field label="名称">
        <Input
          value={form.displayName}
          onChange={(e) => patch({ displayName: e.target.value })}
          placeholder="如 DeepSeek 官方"
        />
      </Field>
      <Field
        label="标识（写入 settings.yaml 的键名）"
        hint={
          form.name.trim()
            ? undefined
            : `留空则自动使用：${suggestProviderName(form.displayName || 'provider')}`
        }
      >
        <Input
          value={form.name}
          onChange={(e) => patch({ name: e.target.value })}
          placeholder="留空按名称生成"
          className="font-mono"
        />
      </Field>
      <div className="col-span-2">
        <Field label="协议">
          <ChoiceGrid options={API_CHOICES} value={form.api} onChange={(v) => patch({ api: v })} cols={4} />
        </Field>
      </div>
      <Field label="Base URL" hint="留空使用 DSH 内置端点">
        <Input
          value={form.baseURL}
          onChange={(e) => patch({ baseURL: e.target.value })}
          placeholder="https://…/v1"
          className="font-mono"
        />
      </Field>
      <Field label="密钥环境变量名" hint="DSH 从这个变量读 key；也是本地 key 注入时的变量名">
        <Input
          value={form.apiKeyEnv}
          onChange={(e) => patch({ apiKeyEnv: e.target.value })}
          placeholder={suggestEnvName(form.name || form.displayName || 'provider')}
          className="font-mono"
        />
      </Field>
      <div className="col-span-2">
        <ApiKeyField value={form.apiKey} onChange={(v) => patch({ apiKey: v })} />
      </div>
      <div className="col-span-2">
        <ModelEditor
          models={form.models}
          onChange={(models) => patch({ models })}
          fetchCtx={{
            api: form.api || undefined,
            baseURL: form.baseURL,
            apiKeyEnv:
              form.apiKeyEnv.trim() ||
              suggestEnvName(form.name || form.displayName || 'provider'),
            apiKey: form.apiKey,
            // The id the OS credential manager entry is keyed by; absent
            // while creating a brand-new provider.
            providerId,
          }}
        />
      </div>
      <div className="col-span-2">
        <Field label="备注">
          <Input value={form.notes} onChange={(e) => patch({ notes: e.target.value })} />
        </Field>
      </div>
      {withEnabled && (
        <div className="col-span-2">
          <SettingRow
            title="启用"
            description="停用后此供应商不再下发给继承全局配置的实例"
            control={<Switch checked={form.enabled} onChange={(v) => patch({ enabled: v })} />}
          />
        </div>
      )}
    </div>
  )
}

/* ------------------------------------------------------------------ *
 * new-provider row card (same row language as provider rows)
 * ------------------------------------------------------------------ */

export function NewProviderCard({
  onCancel,
  initialPresetId,
  presetMode,
}: {
  onCancel: () => void
  /** Preset applied when the card mounts (the empty-state "从预设新建" entry). */
  initialPresetId?: string
  /** Gallery mode: the custom form only appears after 自定义 is clicked. */
  presetMode?: boolean
}) {
  const config = useApiConfigStore((s) => s.config)
  const addProvider = useApiConfigStore((s) => s.addProvider)
  const saving = useApiConfigStore((s) => s.saving)
  const toast = useUIStore((s) => s.toast)
  const { t, riseItem } = useMotion()
  const [form, setForm] = useState<ProviderForm>(() => {
    const p = VENDOR_PRESETS.find((x) => x.id === initialPresetId)
    return p ? presetToForm(p) : emptyProviderForm()
  })
  const [presetId, setPresetId] = useState(initialPresetId ?? (presetMode ? '' : 'custom'))
  const patch = (p: Partial<ProviderForm>) => setForm((f) => ({ ...f, ...p }))

  const applyPreset = (id: string) => {
    setPresetId(id)
    const preset = VENDOR_PRESETS.find((p) => p.id === id)
    if (preset) {
      setForm(presetToForm(preset))
      toast({
        kind: 'info',
        title: `已填入 ${preset.label} 预设`,
        message: '填好 API Key 后，点「获取可用模型」拉取你账号下的模型清单。',
        duration: 4000,
      })
    } else {
      // Returning to 自定义 resets only the template-owned fields; anything
      // already typed into 备注/模型 by the user is intentionally preserved
      // — wiping a half-filled form on a mis-click is worse than leftovers.
      setForm((f) => ({ ...f, name: '', displayName: '', kind: 'custom', api: 'openai-completions', baseURL: '', apiKey: '', apiKeyEnv: '', notes: '' }))
    }
  }

  const save = async () => {
    const displayName = form.displayName.trim()
    const name = (form.name.trim() || suggestProviderName(displayName)).toLowerCase()
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
      kind: form.kind,
      notes: form.notes.trim() || undefined,
      api: form.api || undefined,
      baseURL: form.baseURL.trim() || undefined,
      apiKeyEnv: form.apiKeyEnv.trim() || suggestEnvName(name),
      apiKey: form.apiKey.trim() || undefined,
      models: form.models.filter((m) => m.id.trim()),
      enabled: true,
    })
    if (created) {
      onCancel()
      toast({ kind: 'success', title: `已添加到全局库：${name}`, message: '在下方实例行或实例详情页里下发给需要的实例。' })
    }
  }

  return (
    <motion.div variants={riseItem} exit={{ opacity: 0, transition: t(0.14) }}>
      <div className="overflow-hidden rounded-lg bg-surface ring-1 ring-inset ring-line">
        <div className="flex items-center gap-3 px-3.5 pt-3">
          <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-accent-soft text-accent-ink">
            <Plus size={16} />
          </span>
          <div className="min-w-0 flex-1">
            <span className="text-md font-medium text-ink">新建供应商</span>
            <p className="text-sm text-ink-faint">加入全局库后，可下发给任意托管实例</p>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            <Button size="sm" variant="secondary" onClick={onCancel}>
              <X size={12} />
              取消
            </Button>
            <Button size="sm" variant="primary" disabled={saving} onClick={() => void save()}>
              <Check size={12} />
              {saving ? '保存中…' : '保存到全局库'}
            </Button>
          </div>
        </div>
        {/* Preset strip: pick a mainstream provider to prefill, or 自定义 for a
            blank form. Selecting a preset fills every field but keeps them
            editable — a preset is a starting value, not a lock. */}
        <div className="border-t border-line px-3.5 py-3">
          <div className="mb-2 flex items-center gap-2">
            <span className="text-sm font-medium text-ink-muted">从预设开始</span>
            <span className="text-sm text-ink-faint">选择厂商自动填入地址与模型，仍可修改</span>
          </div>
          <div className="flex flex-wrap gap-1.5">
            <PresetChip active={presetId === 'custom'} onClick={() => applyPreset('custom')}>
              自定义
            </PresetChip>
            {VENDOR_PRESETS.map((p) => (
              <PresetChip key={p.id} active={presetId === p.id} onClick={() => applyPreset(p.id)}>
                {p.label}
              </PresetChip>
            ))}
          </div>
          {presetId && presetId !== 'custom' && (() => {
            const sel = VENDOR_PRESETS.find((p) => p.id === presetId)
            return sel?.consoleUrl ? (
              <p className="mt-2 text-sm text-ink-faint">
                密钥在{' '}
                <a
                  href={sel.consoleUrl}
                  target="_blank"
                  rel="noreferrer"
                  className="text-accent-ink underline-offset-2 hover:underline"
                >
                  {sel.notes.split(' · ').pop()}
                </a>{' '}
                创建后填入 DSH，或在下方临时输入
              </p>
            ) : null
          })()}
        </div>
        <div className="border-t border-line px-3.5 py-3">
          <ProviderFields form={form} patch={patch} />
        </div>
      </div>
    </motion.div>
  )
}

/** A pill in the preset strip — same visual language as the wizard's choice
 *  grid, single-select, with an explicit "自定义" as the escape hatch. */
export function PresetChip({
  active,
  onClick,
  children,
}: {
  active: boolean
  onClick: () => void
  children: ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        'rounded-full px-2.5 py-1 text-sm ring-1 ring-inset transition-all duration-150',
        active
          ? 'bg-accent-soft font-medium text-accent-ink ring-accent'
          : 'bg-surface text-ink-muted ring-line hover:ring-line-strong',
      )}
    >
      {children}
    </button>
  )
}

/* ------------------------------------------------------------------ *
 * model editor
 * ------------------------------------------------------------------ */

/**
 * Session cache of discovered listings (per base+env-var). The raw listing is
 * never persisted to disk — a provider's model set is account- and
 * time-dependent, and a stale cache behind a manual "刷新" is fine.
 */