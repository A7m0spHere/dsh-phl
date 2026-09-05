/** One provider row in the library: form-in-place, key handling, model editor. */
import { useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { Check, KeyRound, Pencil, Trash2, X } from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'
import { suggestEnvName, suggestProviderName } from '@/stores'
import { ApiProvider } from '@/types'
import { Badge, Button, Tooltip } from '@/components/ui'
import {
  KIND_LABEL,
  providerToForm,
  ProviderFields,
  ProviderForm,
} from './fields'


export function ProviderCard({
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

  // Rows carry the same layout="position" + rise-in language as version /
  // runtime rows; entering edit mode re-reads the saved values so a stale
  // draft from a cancelled session never shows up.
  const openEdit = () => {
    setForm(providerToForm(provider))
    setEditing(true)
  }

  const save = () => {
    const name = (form.name.trim() || suggestProviderName(form.displayName)).toLowerCase()
    onEdit({
      name,
      kind: form.kind,
      notes: form.notes.trim() || undefined,
      api: form.api || undefined,
      baseURL: form.baseURL.trim() || undefined,
      apiKeyEnv: form.apiKeyEnv.trim() || suggestEnvName(name),
      apiKey: form.apiKey.trim() || undefined,
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
          editing ? 'ring-accent/40' : 'ring-line hover:ring-line-strong/70',
        )}
      >
        <div className="flex items-center gap-3">
          <span
            className={cn(
              'flex h-9 w-9 shrink-0 items-center justify-center rounded-lg transition-colors',
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
              <Tooltip
                allowOverflow
                content={
                  provider.apiKey
                    ? `密钥已存于 PHL 本地配置，启动实例时注入为环境变量 ${provider.apiKeyEnv}；系统/实例环境已有同名值时以其为准`
                    : `DSH 从环境变量 ${provider.apiKeyEnv} 读取密钥（或由 DSH 凭据提供）；可在编辑中直接填入密钥`
                }
              >
                <span className="font-mono">{provider.apiKeyEnv}</span>
              </Tooltip>
              {provider.apiKey && <Badge tone="ok">本机密钥</Badge>}
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
                  onClick={openEdit}
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
              <div className="mt-3 border-t border-line pt-3">
                <ProviderFields
                  providerId={provider.id}
                  form={form}
                  patch={(p) => setForm((f) => ({ ...f, ...p }))}
                  withEnabled
                />
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </motion.li>
  )
}

/* ------------------------------------------------------------------ *
 * panel
 * ------------------------------------------------------------------ */
