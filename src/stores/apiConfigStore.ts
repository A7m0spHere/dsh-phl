import { parseThrownError } from '@/lib/errorCodes'
import { create } from 'zustand'
import * as desktop from '@/lib/desktop'
import { useSettingsStore } from './settingsStore'
import { useInstanceStore } from './instanceStore'
import { useUIStore } from './uiStore'
import { type AdoptionPlan, planAdoption } from '@/lib/apiDiff'
import type { ApiBinding, ApiConfig, ApiProvider, InstanceLiveSnapshot } from '@/types'
import { repository } from '@/services'
import { hasMissingMetadata, metadataSummary } from '@/lib/modelMetadata'

/**
 * Frontend half of the global API provider library (`src-tauri/src/api_config.rs`).
 *
 * The store is deliberately thin: the library lives on disk under
 * `<root>/config/api.json` (Rust is the writer), and instances carry only a
 * *binding*. Every mutation goes through `save`, which round-trips through
 * Rust so validation (env-name grammar, provider-name grammar) happens in
 * exactly one place.
 *
 * Direction matters: 同步 pushes library → instance files; 采纳 pulls
 * instance files → library (never touching other instances). Launch-time
 * rewriting was removed — the files are the instances' truth, and `snapshots`
 * holds what each settings.yaml actually says.
 *
 * In the browser preview the Rust commands are unavailable, so config falls
 * back to localStorage and sync no-ops — enough for UI work.
 */

const BROWSER_KEY = 'phl.apiConfig'

function newProviderId(): string {
  return `p-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`
}

/** Slug DSH-style provider names come from: `DeepSeek 官方` → `deepseek`. */
export function suggestProviderName(name: string): string {
  const ascii = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return ascii || `provider-${Math.random().toString(36).slice(2, 6)}`
}

export function suggestEnvName(name: string): string {
  const slug = suggestProviderName(name).replace(/-/g, '_').toUpperCase()
  return `PHL_${slug}_API_KEY`
}

/** Serializes API-config writes; see `save`. */
let saveQueue: Promise<unknown> = Promise.resolve()

interface ApiConfigState {
  config: ApiConfig | null
  loaded: boolean
  saving: boolean
  /** Saves queued behind the one in flight (the storage page waits for these). */
  pendingSaves: number
  enriching: boolean
  enrichMissingModels: () => Promise<void>
  /** Instance id whose binding is currently being materialized. */
  syncing: string | null
  /** Last-known live snapshot per instance (the file truth; refreshed on demand). */
  snapshots: Record<string, InstanceLiveSnapshot>

  load: () => Promise<void>
  /** Validate + persist the whole library; returns null when the save failed. */
  save: (next: ApiConfig) => Promise<ApiConfig | null>

  addProvider: (
    draft: Omit<ApiProvider, 'id'> & { id?: string },
  ) => Promise<ApiProvider | null>
  updateProvider: (id: string, patch: Partial<ApiProvider>) => Promise<boolean>
  removeProvider: (id: string) => Promise<boolean>
  setDefaults: (defaultProviderId: string | undefined, defaultModel: ApiConfig['defaultModel']) => Promise<boolean>

  /** Materialize + persist the binding on the instance manifest. */
  syncInstance: (instanceId: string, binding: ApiBinding) => Promise<ApiBinding | null>
  /** Read a live settings.yaml back as a library seed (first-run import). */
  importFromInstance: (instanceId: string) => Promise<ApiConfig | null>
  /** Pull live snapshots for the given instance bindings. */
  refreshSnapshots: (bindings: Record<string, ApiBinding>) => Promise<void>
  /**
   * Fold one instance's live state back into the library. Pure additions
   * always apply; same-name field changes apply; `removeMissing` (entries
   * the instance deleted) only when the caller opts in. This never writes
   * to any instance's settings.yaml.
   */
  adoptFromInstance: (
    instanceId: string,
    opts: { removeMissing: boolean; updateDefault: boolean },
  ) => Promise<AdoptionPlan | null>
}

function loadBrowserConfig(): ApiConfig | null {
  try {
    const raw = localStorage.getItem(BROWSER_KEY)
    return raw ? (JSON.parse(raw) as ApiConfig) : null
  } catch {
    return null
  }
}

function saveBrowserConfig(config: ApiConfig): void {
  localStorage.setItem(BROWSER_KEY, JSON.stringify(config))
}

export const useApiConfigStore = create<ApiConfigState>()((set, get) => ({
  config: null,
  loaded: false,
  saving: false,
      pendingSaves: 0,
  enriching: false,
  syncing: null,
  snapshots: {},

  async enrichMissingModels() {
    const { config, enriching, saving } = get()
    if (!config || enriching || saving) return
    const root = useSettingsStore.getState().root
    set({ enriching: true })
    try {
      const batches: import('@/types').ModelMetadataBatch[] = []
      const providers: ApiProvider[] = []
      for (const provider of config.providers) {
        const candidates = provider.models.filter((m) => m.id.trim() && hasMissingMetadata(m))
        if (!candidates.length) { providers.push(provider); continue }
        const batch = await repository.enrichModelMetadata({ models: candidates, provider: provider.name })
        batches.push(batch)
        let cursor = 0
        providers.push({ ...provider, models: provider.models.map((m) => candidates.includes(m) ? batch.results[cursor++].model : m) })
      }
      // No stale network response may replace edits, deletions or a switched root.
      if (get().config !== config || root !== useSettingsStore.getState().root) {
        useUIStore.getState().toast({ kind: 'info', title: '配置已发生变化，本次补全未应用，请重试' })
        return
      }
      const combined: import('@/types').ModelMetadataBatch = {
        results: batches.flatMap((b) => b.results),
        catalogStatus: batches.find((b) => b.catalogStatus !== 'fresh')?.catalogStatus ?? 'fresh',
      }
      if (combined.results.some((r) => r.changed) && !await get().save({ ...config, providers })) return
      useUIStore.getState().toast({ kind: 'info', title: '模型信息补全完成', message: metadataSummary(combined) })
    } catch (err) {
      useUIStore.getState().toast({ kind: 'warn', title: '模型信息补全暂不可用', message: parseThrownError(err).message })
    } finally { set({ enriching: false }) }
  },

  async load() {
    const root = useSettingsStore.getState().root
    set({ config: null, snapshots: {}, loaded: false })
    if (desktop.isDesktop) {
      const config = await desktop.loadApiConfig()
      if (root === useSettingsStore.getState().root) set({ config, loaded: true })
      return
    }
    set({ config: loadBrowserConfig(), loaded: true })
  },

  async save(next) {
    // Serialized in call order. Rejecting a second save while one was in
    // flight threw the user's edit away: the caller is a form that closes
    // without awaiting the result, so "请等待…" was the last thing they saw.
    // Each call now waits its turn and resolves with what it actually wrote.
    set((state) => ({ pendingSaves: state.pendingSaves + 1 }))
    const run = saveQueue.then(async () => {
      set({ saving: true })
      try {
        if (desktop.isDesktop) {
          const saved = await desktop.saveApiConfig(next)
          set({ config: saved })
          return saved
        }
        saveBrowserConfig(next)
        set({ config: next })
        return next
      } catch (err) {
        useUIStore.getState().toast({
          kind: 'error',
          title: '保存 API 配置库失败',
          message: parseThrownError(err).message,
        })
        return null
      } finally {
        set({ saving: false })
      }
    })
    saveQueue = run.then(
      () => undefined,
      () => undefined,
    )
    try {
      return await run
    } finally {
      set((state) => ({ pendingSaves: Math.max(0, state.pendingSaves - 1) }))
    }
  },

  async addProvider(draft) {
    const current = get().config
    const provider: ApiProvider = { ...draft, id: draft.id ?? newProviderId() }
    const base: ApiConfig = current ?? {
      version: 1,
      updatedAt: '',
      providers: [],
    }
    const providers = base.providers.some((p) => p.id === provider.id)
      ? base.providers.map((p) => (p.id === provider.id ? provider : p))
      : [...base.providers, provider]
    const saved = await get().save({
      ...base,
      providers,
      defaultProviderId: base.defaultProviderId ?? provider.id,
    })
    return saved ? provider : null
  },

  async updateProvider(id, patch) {
    const current = get().config
    if (!current) return false
    if (!current.providers.some((p) => p.id === id)) return false
    const saved = await get().save({
      ...current,
      providers: current.providers.map((p) => (p.id === id ? { ...p, ...patch } : p)),
    })
    return !!saved
  },

  async removeProvider(id) {
    const current = get().config
    if (!current) return false
    const removed = current.providers.find((p) => p.id === id)
    const saved = await get().save({
      ...current,
      providers: current.providers.filter((p) => p.id !== id),
      defaultProviderId:
        current.defaultProviderId === id ? undefined : current.defaultProviderId,
      // The default model references providers *by name*, and that reference
      // is independent of which provider is the default — a provider with a
      // different id can own the default model and must be cleared too.
      defaultModel:
        current.defaultModel && removed && current.defaultModel.providerName === removed.name
          ? undefined
          : current.defaultModel,
    })
    // Custom bindings may still list the deleted id. The checkbox list can
    // only render providers that exist, so without this prune the instance
    // stays permanently un-syncable (resolve errors on the missing id) and
    // the user cannot un-pick what they cannot see.
    if (saved && removed) {
      for (const instance of useInstanceStore.getState().instances) {
        const binding = instance.api
        if (binding?.inheritance === 'custom' && binding.providerIds.includes(id)) {
          useInstanceStore
            .getState()
            .updateInstance(instance.id, { api: { ...binding, providerIds: binding.providerIds.filter((p) => p !== id) } })
        }
      }
    }
    return !!saved
  },

  async setDefaults(defaultProviderId, defaultModel) {
    const current = get().config
    if (!current) return false
    const saved = await get().save({ ...current, defaultProviderId, defaultModel })
    return !!saved
  },

  async syncInstance(instanceId, binding) {
    const { config, syncing } = get()
    if (!config) {
      useUIStore.getState().toast({ kind: 'info', title: '还没有全局配置库', message: '先在「模型与 API」页创建供应商配置。' })
      return null
    }
    if (syncing) return null
    set({ syncing: instanceId })
    try {
      let applied: ApiBinding
      if (desktop.isDesktop) {
        applied = await desktop.syncInstanceApi(instanceId, binding, config)
      } else {
        applied = { ...binding, syncedAt: new Date().toISOString(), syncedHash: 'browser' }
      }
      // Persisting the binding rides on the instance manifest update — the
      // same path every other instance field uses, so save conflicts stay
      // in one place.
      useInstanceStore.getState().updateInstance(instanceId, { api: applied })
      return applied
    } catch (err) {
      useUIStore.getState().toast({
        kind: 'error',
        title: '同步 API 配置失败',
        message: parseThrownError(err).message,
      })
      return null
    } finally {
      set({ syncing: null })
    }
  },

  async importFromInstance(instanceId) {
    if (!desktop.isDesktop) return null
    try {
      const imported = await desktop.importInstanceApi(instanceId)
      if (!imported) return null
      // Persist right away: an imported-but-unsaved seed would vanish on the
      // next launch while the UI already behaved as if the library existed.
      const saved = await get().save(imported)
      return saved
    } catch (err) {
      useUIStore.getState().toast({
        kind: 'error',
        title: '从实例导入失败',
        message: parseThrownError(err).message,
      })
      return null
    }
  },

  async refreshSnapshots(bindings) {
    if (!desktop.isDesktop) return
    const { config } = get()
    if (!config) return
    const entries = await Promise.all(
      Object.entries(bindings).map(async ([id, binding]) => {
        try {
          return [id, await desktop.instanceLiveSnapshot(id, binding, config)] as const
        } catch {
          return [id, undefined] as const
        }
      }),
    )
    set({
      snapshots: {
        ...get().snapshots,
        ...Object.fromEntries(
          entries.filter((e): e is readonly [string, InstanceLiveSnapshot] => !!e[1]),
        ),
      },
    })
  },

  async adoptFromInstance(instanceId, opts) {
    const { config, snapshots } = get()
    const live = snapshots[instanceId]?.live
    if (!config || !live) return null
    const instance = useInstanceStore.getState().byId(instanceId)
    const binding: ApiBinding = instance?.api ?? { inheritance: 'none', providerIds: [] }
    const plan = planAdoption(binding, config, live)

    let next: ApiConfig = { ...config, providers: [...config.providers] }
    const replace = (id: string, patch: Partial<ApiProvider>) => {
      next.providers = next.providers.map((p) => (p.id === id ? { ...p, ...patch } : p))
    }
    for (const p of plan.add) {
      // New identity: `imported-<name>` ids collide across instances.
      next.providers.push({ ...p, id: newProviderId() })
    }
    for (const u of plan.update) replace(u.id, u.patch)
    if (opts.removeMissing) {
      const doomed = new Set(plan.removeMissing.map((r) => r.id))
      next.providers = next.providers.filter((p) => !doomed.has(p.id))
      if (next.defaultProviderId && doomed.has(next.defaultProviderId)) {
        next.defaultProviderId = undefined
      }
    }
    if (opts.updateDefault && live.defaultModel) {
      const libOwner = next.providers.find((p) => p.name === live.defaultModel!.providerName)
      if (libOwner) {
        next.defaultModel = live.defaultModel
        next.defaultProviderId = libOwner.id
      }
    }
    const saved = await get().save(next)
    return saved ? plan : null
  },
}))
