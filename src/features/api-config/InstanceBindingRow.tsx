/** One instance row in the binding scope list. */
import { RefreshCw, Unplug } from 'lucide-react'
import { cn } from '@/lib/cn'
import { classifyInstance, planAdoption } from '@/lib/apiDiff'
import { useApiConfigStore, useInstanceStore, useUIStore } from '@/stores'
import { ApiBinding } from '@/types'
import { Badge, Button, Tooltip } from '@/components/ui'
import {
  formatSynced,
} from './fields'


export function InstanceBindingRow({
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
    <div className="flex items-center gap-3 border-t border-line px-4 py-2.5 transition-colors first:border-t-0 hover:bg-surface-hover/50">
      <span
        className={cn(
          'h-[7px] w-[7px] shrink-0 rounded-full',
          binding.inheritance === 'none'
            ? 'bg-ink-faint/40'
            : localChanges
              ? 'bg-warn'
              : 'bg-ok',
        )}
        aria-hidden
      />
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
              {localChanges && <span className="text-warn">· 有本地改动</span>}
            </>
          )}
        </span>
      </span>
      {snapshot && snapshot.missingKeys.length > 0 && (
        <Tooltip allowOverflow content={`以下密钥变量既无系统环境值、供应商也未在 PHL 内存密钥：${snapshot.missingKeys.join('、')}。请在对应供应商里填入密钥，或配置同名环境变量。`}>
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
