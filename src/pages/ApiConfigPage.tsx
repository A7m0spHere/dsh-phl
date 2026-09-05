import { useEffect, useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  CloudDownload,
  KeyRound,
  Plus,
  RefreshCw,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { useMotion } from '@/lib/motion'
import {
  useApiConfigStore,
  useInstanceStore,
  useUIStore,
} from '@/stores'
import type { ApiProvider } from '@/types'
import {
  Button,
  EmptyState,
  SectionCard,
  Select,
  SettingRow,
  Skeleton,
} from '@/components/ui'
import { PageShell } from '@/components/layout/Page'
import { PanelDivider, PanelGroup, PanelItem, PanelShell, PanelStat } from '@/components/layout/Panel'

import { NewProviderCard } from '@/features/api-config/fields'
import { DefaultModelPicker } from '@/features/api-config/DefaultModelPicker'
import { ProviderCard } from '@/features/api-config/ProviderCard'
import { InstanceBindingRow } from '@/features/api-config/InstanceBindingRow'

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
        <PanelStat label="有本地改动" value={dirty ? `${dirty} 个` : '无'} />
      </PanelGroup>

      {(config?.providers.length ?? 0) > 0 && (
        <>
          <PanelDivider />
          <PanelGroup title="供应商">
            {config!.providers.map((p) => (
              <PanelItem
                key={p.id}
                groupId="api-providers"
                icon={<KeyRound size={14} />}
                label={p.name}
                count={p.models.length || undefined}
                active={false}
                onClick={() => navigate({ name: 'apiConfig' })}
              />
            ))}
          </PanelGroup>
        </>
      )}

      <div className="mt-auto p-3">
        <div className="flex items-start gap-2 rounded-lg bg-surface-sunken p-3 text-sm leading-relaxed text-ink-faint ring-1 ring-inset ring-line">
          <KeyRound size={13} className="mt-[2px] shrink-0" />
          <span>密钥可填入 PHL（保存在本机配置目录，启动实例时注入，不写入实例目录），也可只填环境变量名、由系统环境或 DSH 凭据提供。</span>
        </div>
      </div>
    </PanelShell>
  )
}

/* ------------------------------------------------------------------ *
 * instance bindings row
 * ------------------------------------------------------------------ */

export function ApiConfigPage() {
  const config = useApiConfigStore((s) => s.config)
  const loaded = useApiConfigStore((s) => s.loaded)
  const saving = useApiConfigStore((s) => s.saving)
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
  const instances = useInstanceStore((s) => s.instances)
  const instanceStates = useInstanceStore((s) => s.states)
  const confirm = useUIStore((s) => s.confirm)
  const toast = useUIStore((s) => s.toast)
  const { stagger, riseItem } = useMotion()

  const [adding, setAdding] = useState(false)
  const [importTarget, setImportTarget] = useState('')

  // Live badges are library-relative too: a provider rename is invisible to
  // `bindingsKey`, so the effect also rides the library identity — every
  // save (adoption and provider edits included) re-snapshots the list.
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
    if (old && patch.apiKeyEnv && patch.apiKeyEnv !== old.apiKeyEnv && usedByCount(id) > 0) {
      const ok = await confirm({
        title: '更换密钥环境变量',
        message: `${usedByCount(id)} 个已绑定实例此前按 ${old.apiKeyEnv} 解析密钥。改名后需要在 DSH 中为新变量重新配置一次密钥，除非系统环境里已有同名变量。`,
        detail: `${old.apiKeyEnv} → ${patch.apiKeyEnv}`,
        confirmLabel: '仍然更换',
      })
      if (!ok) return
    }
    // A rename leaves the old-named entry inside instance files (they carry
    // no old-name marker) — where it resurfaces as "本地新增" and invites a
    // mistaken 采纳. Same discipline as the env-var rename: warn first.
    if (
      old &&
      patch.name &&
      patch.name !== old.name &&
      instances.some((i) => i.api && i.api.inheritance !== 'none' && i.api.syncedHash)
    ) {
      const ok = await confirm({
        title: '重命名供应商标识',
        message: '已下发到实例的条目仍用旧名保留——下次同步会把新名写入，而旧名条目会被识别为"本地新增"。如只是想改显示信息，请改备注而不是标识。',
        detail: `${old.name} → ${patch.name}`,
        confirmLabel: '仍然重命名',
      })
      if (!ok) return
    }
    if (patch.name && config?.providers.some((p) => p.id !== id && p.name === patch.name)) {
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

  const onImport = async () => {
    if (!importTarget) return
    const seeded = await importFromInstance(importTarget)
    if (seeded) {
      setImportTarget('')
      toast({
        kind: 'success',
        title: '已从实例导入',
        message: `${seeded.providers.length} 个供应商已进入全局库，可继续编辑名称与密钥变量。`,
      })
    }
  }

  if (!loaded) {
    return (
      <PageShell title="模型与 API" subtitle="维护一份全局供应商配置，按实例下发到 DSH。">
        <div className="space-y-2">
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-[92px]" />
          ))}
        </div>
      </PageShell>
    )
  }

  const providerCount = config?.providers.length ?? 0
  const isEmpty = providerCount === 0 && !adding

  return (
    <PageShell
      title="模型与 API"
      subtitle="在这里配置一次，新建实例开箱即用；存量实例可随时同步或接管。密钥可直接填入（存本机、启动时注入），也可引用已有环境变量。"
      actions={
        providerCount > 0 && !adding ? (
          <Button size="sm" variant="primary" disabled={saving} onClick={() => setAdding(true)}>
            <Plus size={12} />
            新增供应商
          </Button>
        ) : undefined
      }
    >
      <motion.div variants={stagger()} initial="hidden" animate="show" className="space-y-4">
        {isEmpty ? (
          <motion.div variants={riseItem}>
            <EmptyState
              icon={<KeyRound size={20} />}
              title="还没有全局配置库"
              description="新建一个供应商，或从已经配置好的实例导入——导入会读取该实例当前的 settings.yaml。"
              action={
                <div className="flex flex-wrap items-center justify-center gap-2">
                  <Button variant="primary" onClick={() => setAdding(true)}>
                    <Plus size={13} />
                    新建供应商
                  </Button>
                  {instances.length > 0 && (
                    <>
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
                      <Button variant="secondary" disabled={!importTarget} onClick={() => void onImport()}>
                        <CloudDownload size={13} />
                        导入
                      </Button>
                    </>
                  )}
                </div>
              }
            />
          </motion.div>
        ) : (
          <div className="space-y-2">
            <AnimatePresence initial={false}>
              {adding && (
                <NewProviderCard key="new" onCancel={() => setAdding(false)} />
              )}
            </AnimatePresence>

            {providerCount > 0 && (
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
            )}
          </div>
        )}

        {providerCount > 0 && (
          <motion.div variants={riseItem}>
            <SectionCard title="默认模型" description="下发给所有「继承全局」实例的默认配置">
          <SettingRow
            title="默认供应商"
            control={
              <Select
                value={config?.defaultProviderId ?? ''}
                onChange={(e) => void setDefaults(e.target.value || undefined, config?.defaultModel)}
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
          </motion.div>
        )}

        {providerCount > 0 && (
          <motion.div variants={riseItem}>
            <SectionCard
              title="实例绑定"
              description="展示每个实例当前真实生效的配置。实例内改动会被保留；同步按全局库覆盖（覆盖前有确认），采纳把实例配置合入全局库。"
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
          </motion.div>
        )}
      </motion.div>
    </PageShell>
  )
}

/* ------------------------------------------------------------------ *
 * default-model picker (provider + model)
 * ------------------------------------------------------------------ */
