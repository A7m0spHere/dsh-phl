import { create } from 'zustand'
import { repository, Cancelled } from '@/services'
import type { PluginCatalogOrigin } from '@/services'
import type { DshVersion, InstalledPlugin, InstanceTemplate, Plugin, Runtime } from '@/types'
import { useUIStore } from './uiStore'
import { useInstanceStore } from './instanceStore'
import { useSettingsStore } from './settingsStore'
import { detectPendingChanges, loadPendingSet, savePendingSet } from '@/lib/pendingReleases'
import { parseThrownError } from '@/lib/errorCodes'

/**
 * Every settled version-catalog fetch runs through here: GitHub-only rows
 * are remembered, remembered rows that reappear installable raise a toast —
 * the "notify me when the pending version is published" behaviour, riding
 * the existing manual/auto sync with no extra polling.
 */
function observePendingReleases(versions: DshVersion[]) {
  try {
    const { published, next } = detectPendingChanges(loadPendingSet(), versions)
    savePendingSet(next)
    if (published.length && useSettingsStore.getState().pendingReleaseAlerts) {
      const first = published[0]
      useUIStore.getState().toast({
        kind: 'success',
        title:
          published.length === 1
            ? `新版本已可安装：${first}`
            : `${published.length} 个版本已可安装`,
        message: '此前仅在 GitHub 发布，现已上架安装包源。',
        action: { label: '安装', run: () => void useCatalogStore.getState().installVersion(`dsh-${first}`) },
      })
    }
  } catch (err) {
    console.warn('[phl] pending-release check failed:', err)
  }
}

/** Live transfer state for one (instance, plugin) install. */
export interface PluginTransferState {
  stage: 'queued' | 'preparing' | 'downloading' | 'verifying' | 'installing'
  progress: number
  bytesDone: number
  bytesPerSec: number
}

export const pluginKey = (instanceId: string, pluginId: string) => `p:${instanceId}:${pluginId}`

/* ----------------------- 慢速官方源 → 镜像提醒 ----------------------- */

const OFFICIAL_HINT_MIN_AVERAGE = 256 * 1024 // bytes/s
let mirrorHintShown = false

/**
 * 官方 npm 源在国内经常慢到让人以为程序死了。下载/装依赖明显偏慢且当前
 * 用的正是官方源时，给一次性的可操作提醒（一键切到 npmmirror）；镜像源
 * 或已提示过则绝不再弹。切换设置只影响之后的安装——进行中的那一次要用
 * 户自己取消重试，绝不擅自替用户取消。
 */
function maybeHintMirror(slow: () => boolean): void {
  if (mirrorHintShown) return
  if (useSettingsStore.getState().source !== 'official') return
  if (!slow()) return
  mirrorHintShown = true
  useUIStore.getState().toast({
    kind: 'info',
    title: '官方源速度较慢，试试镜像？',
    message:
      '检测到从 npm 官方源下载较慢。切换到 npmmirror 镜像通常能显著提速；切换后取消当前安装、重新安装即可生效。',
    duration: 12000,
    action: {
      label: '切换到镜像',
      run: () => {
        useSettingsStore.getState().set('source', 'mirror-cn')
        useUIStore
          .getState()
          .toast({ kind: 'success', title: '已切换到 npmmirror 镜像', message: '取消当前安装后重新安装即可生效。' })
      },
    },
  })
}

interface CatalogState {
  versions: DshVersion[]
  runtimes: Runtime[]
  plugins: Plugin[]
  templates: InstanceTemplate[]
  loaded: boolean
  loading: boolean
  /**
   * True when *no* source and not even the on-disk cache could produce a
   * catalog — the registry tab shows a notice with the real cause and a
   * retry instead of failing silently. Partial degradations (fallback
   * mirror, cache replay) keep a populated list and are described by
   * `pluginsOrigin` instead.
   */
  pluginsOffline: boolean
  pluginsError?: string
  /**
   * Provenance of the loaded catalog — which base served it, whether the
   * user's configured source was bypassed, and the registry's `updated`
   * stamp. `null` before the first load or in the browser mock (no remote
   * registry to attribute). The registry tab surfaces this so a stale
   * mirror can never masquerade as the live catalog.
   */
  pluginsOrigin: PluginCatalogOrigin | null
  /**
   * Versions have settled (成功或失败都算)。版本目录是最慢的远程源，
   * 页面骨架屏只等自己关心的模块，不陪别的源干等。
   */
  versionsLoaded: boolean
  /** A version-catalog re-sync (manual or scheduled) is in flight. */
  versionsSyncing: boolean
  /** Epoch ms of the last *successful* catalog sync — drives the "上次同步" label. */
  versionsSyncedAt: number | null
  /**
   * Epoch ms of the last sync *attempt*, success or not. The scheduler ticks
   * on this so a failing network can't turn auto-refresh into a retry loop.
   */
  versionsSyncAttemptedAt: number

  load: () => Promise<void>
  /**
   * Re-pull the remote catalog and merge it into `versions`. `silent` is the
   * scheduled path: no toasts either way, just fresh data if it arrives.
   */
  refreshVersions: (opts?: { silent?: boolean }) => Promise<void>

  installVersion: (id: string) => Promise<void>
  cancelVersion: (id: string) => void
  removeVersion: (id: string) => Promise<void>

  installRuntime: (id: string) => Promise<void>
  cancelRuntime: (id: string) => void
  removeRuntime: (id: string) => Promise<void>

  /** Live plugin installs keyed by `p:<instanceId>:<pluginId>`. */
  pluginTransfers: Record<string, PluginTransferState>
  /**
   * Resolved newest version per plugin id — `undefined` = not checked yet,
   * `null` = checked but undeterminable (GitHub-source plugins). Lets the
   * updates tab query npm only for installed plugins instead of the whole
   * catalog.
   */
  latestVersions: Record<string, string | null>
  refreshLatestVersions: (instanceId: string) => Promise<void>
  installPlugin: (instanceId: string, pluginId: string) => Promise<void>
  cancelPlugin: (instanceId: string, pluginId: string) => void
  setPluginEnabled: (instanceId: string, pluginId: string, enabled: boolean) => Promise<void>
  uninstallPlugin: (instanceId: string, pluginId: string) => Promise<void>

  versionById: (id: string) => DshVersion | undefined
  runtimeById: (id: string) => Runtime | undefined
  pluginById: (id: string) => Plugin | undefined
  /** Number of transfers currently in flight — drives the title-bar indicator. */
  activeTransfers: () => number
}

const controllers = new Map<string, AbortController>()

/* ------------------------- transfer concurrency gate ------------------------- */

/**
 * The 「同时下载数」 setting enforced for real (roadmap O-10): a new transfer
 * must own one of at most `concurrency` slots before it touches the network;
 * the rest sit in `queued` state and start as predecessors finish or are
 * cancelled. All transfers share the one budget — versions, runtimes and
 * plugin installs compete honestly instead of each page self-limiting.
 */
const activeSlots = new Set<string>()
const slotWaiters: Array<{ key: string; start: () => void }> = []

function slotLimit(): number {
  const raw = useSettingsStore.getState().concurrency
  return Math.max(1, Math.floor(Number.isFinite(raw) ? raw : 2) || 1)
}

function pumpSlots(): void {
  while (activeSlots.size < slotLimit() && slotWaiters.length > 0) {
    slotWaiters.shift()!.start()
  }
}

/**
 * Wait for a slot. `false` means the caller must treat the operation as
 * cancelled — the user aborted while queued (the transfer itself never
 * started, which is exactly why cancel-before-start works).
 */
function acquireTransferSlot(key: string, signal: AbortSignal): Promise<boolean> {
  if (signal.aborted) return Promise.resolve(false)
  if (activeSlots.size < slotLimit() && !activeSlots.has(key)) {
    activeSlots.add(key)
    return Promise.resolve(true)
  }
  return new Promise<boolean>((resolve) => {
    const dequeue = () => {
      const i = slotWaiters.findIndex((w) => w.key === key && w.start === waiter.start)
      if (i >= 0) slotWaiters.splice(i, 1)
      resolve(false)
    }
    const waiter = {
      key,
      start: () => {
        signal.removeEventListener('abort', dequeue)
        if (signal.aborted) {
          resolve(false)
          return
        }
        activeSlots.add(key)
        resolve(true)
      },
    }
    signal.addEventListener('abort', dequeue, { once: true })
    slotWaiters.push(waiter)
  })
}

function releaseTransferSlot(key: string): void {
  activeSlots.delete(key)
  pumpSlots()
}

/**
 * Applies a change to an instance's plugin list against the **current** store
 * state rather than a snapshot.
 *
 * Every plugin mutation awaits a Rust pipeline that can run for minutes, and
 * the transfer guard is per (instance, plugin) — so two plugins can install
 * into the same instance concurrently. Rebuilding the list from a snapshot
 * captured before the await let whichever finished second drop the other's
 * entry, while its files stayed on disk and in cordis.patch.yml.
 */
function patchInstancePlugins(
  instanceId: string,
  update: (plugins: InstalledPlugin[]) => InstalledPlugin[],
) {
  const store = useInstanceStore.getState()
  const current = store.byId(instanceId)
  if (!current) return
  store.updateInstance(instanceId, { plugins: update(current.plugins) })
}

export const useCatalogStore = create<CatalogState>()((set, get) => ({
  versions: [],
  runtimes: [],
  plugins: [],
  templates: [],
  loaded: false,
  loading: false,
  versionsLoaded: false,
  versionsSyncing: false,
  versionsSyncedAt: null,
  versionsSyncAttemptedAt: 0,
  pluginTransfers: {},
  pluginsOffline: false,
  pluginsOrigin: null,
  latestVersions: {},

  async load() {
    if (get().loading) return
    set({ loading: true, versionsSyncAttemptedAt: Date.now() })
    const failures: Array<[string, unknown]> = []
    const swallow = (label: string) => (err: unknown) => {
      console.warn(`[phl] ${label} load failed:`, err)
      failures.push([label, err])
    }
    try {
      // Modules land independently: each one is written into state as soon as
      // it settles, so a slow source never holds the others' data hostage.
      await Promise.all([
        // .then(ok).catch(err) — NOT .then(ok, err): the two-argument form
        // leaves an exception thrown by the success callback (e.g. inside
        // observePendingReleases) unhandled, which rejects the whole
        // Promise.all before `versionsLoaded` is ever set — the versions page
        // then sits on its skeleton forever with no error shown.
        repository
          .listVersions()
          .then((versions) => {
            set({ versions, versionsLoaded: true, versionsSyncedAt: Date.now() })
            observePendingReleases(versions)
          })
          .catch((err) => {
            swallow('版本目录')(err)
            set({ versionsLoaded: true })
          }),
        // Same .then(ok).catch(err) discipline as listVersions above: with
        // the two-argument form, an exception thrown by the success callback
        // (e.g. reading catalog.plugins off a malformed result) escapes the
        // handler, rejects Promise.all, and skips the per-source error toasts.
        repository
          .listRuntimes()
          .then((runtimes) => set({ runtimes }))
          .catch(swallow('Runtime 列表')),
        repository
          .listPlugins()
          .then((catalog) =>
            set({
              plugins: catalog.plugins,
              pluginsOffline: !!catalog.offline,
              pluginsError: catalog.error,
              pluginsOrigin: catalog.origin ?? null,
            }),
          )
          .catch(swallow('插件市场')),
        repository
          .listTemplates()
          .then((templates) => set({ templates }))
          .catch(swallow('实例模板')),
      ])
      set({ loaded: true })
      for (const [label, err] of failures) {
        const detail = err instanceof Error && err.message ? err.message : '无法连接发布源，请检查网络。'
        useUIStore.getState().toast({
          kind: 'error',
          title: `${label}加载失败`,
          message: detail,
          action: { label: '重试', run: () => void get().load() },
        })
      }
    } finally {
      set({ loading: false })
    }
  },

  /* ---------------- versions ---------------- */

  async refreshVersions(opts = {}) {
    const silent = !!opts.silent
    const { versionsSyncing, loading } = get()
    // The startup load already re-pulls the catalog — two concurrent
    // fetches would just race each other's `set`.
    if (versionsSyncing || loading) return
    set({ versionsSyncing: true, versionsSyncAttemptedAt: Date.now() })
    try {
      const next = await repository.listVersions()
      // The new list re-derives every state from a fresh disk scan, which
      // knows nothing about in-flight transfers: a downloading row would
      // flicker back to "可安装" until the next progress tick (or forever,
      // during the pre-first-chunk quiet). Carry those over by id.
      const keep = new Set(['queued', 'downloading', 'extracting', 'verifying', 'failed'])
      const prev = new Map(get().versions.map((v) => [v.id, v]))
      const merged = next.map((v) => {
        const old = prev.get(v.id)
        return old && keep.has(old.state.kind) ? { ...v, state: old.state } : v
      })
      const added = merged.filter((v) => !prev.has(v.id))
      set({ versions: merged, versionsLoaded: true, versionsSyncedAt: Date.now() })
      observePendingReleases(merged)
      if (!silent) {
        useUIStore.getState().toast(
          added.length
            ? {
                kind: 'success',
                title: added.length === 1 ? `发现新版本 ${added[0].name}` : `发现 ${added.length} 个新版本`,
                message: added.map((v) => v.name).slice(0, 5).join('、'),
              }
            : { kind: 'info', title: '已是最新版本', message: '版本目录已同步，没有发现新版本。' },
        )
      }
    } catch (err) {
      console.warn('[phl] version catalog refresh failed:', err)
      if (!silent) {
        useUIStore.getState().toast({
          kind: 'error',
          title: '同步版本目录失败',
          message:
            err instanceof Error && err.message
              ? err.message
              : '无法连接发布源，请检查网络后重试。',
          action: { label: '重试', run: () => void get().refreshVersions() },
        })
      }
    } finally {
      set({ versionsSyncing: false })
    }
  },

  async installVersion(id) {
    const version = get().versions.find((v) => v.id === id)
    if (!version) return
    const controller = new AbortController()
    controllers.set(`v:${id}`, controller)
    const startedAt = Date.now()
    let depsStartedAt = 0

    const patch = (state: DshVersion['state']) =>
      set({ versions: get().versions.map((v) => (v.id === id ? { ...v, state } : v)) })

    patch({ kind: 'queued' })
    try {
      if (!(await acquireTransferSlot(`v:${id}`, controller.signal))) throw new Cancelled()
      await repository.installVersion(
        version,
        (p) => {
          if (p.stage === 'downloading') {
            patch({
              kind: 'downloading',
              progress: p.progress ?? 0,
              bytesDone: p.bytesDone ?? 0,
              bytesPerSec: p.bytesPerSec ?? 0,
            })
            maybeHintMirror(
              () =>
                Date.now() - startedAt > 15_000 &&
                (p.bytesDone ?? 0) / Math.max(Date.now() - startedAt, 1) < OFFICIAL_HINT_MIN_AVERAGE,
            )
          } else if (p.stage === 'extracting') {
            patch({ kind: 'extracting', progress: p.progress ?? 0 })
          } else if (p.stage === 'installing-deps') {
            if (depsStartedAt === 0) depsStartedAt = Date.now()
            patch({ kind: 'installing-deps', progress: p.progress ?? 0 })
            maybeHintMirror(() => Date.now() - depsStartedAt > 60_000)
          } else {
            patch({ kind: 'verifying' })
          }
        },
        controller.signal,
      )
      patch({ kind: 'installed', installedAt: new Date().toISOString() })
      useUIStore.getState().toast({
        kind: 'success',
        title: `DSH ${version.name} 安装完成`,
        message: '现在可以在实例中固定使用这个版本。',
      })
    } catch (err) {
      if (err instanceof Cancelled) {
        patch({ kind: 'available' })
        useUIStore.getState().toast({ kind: 'info', title: `已取消下载 ${version.name}` })
      } else {
        const parsed = parseThrownError(err)
        const reason =
          (parsed.code ? `${parsed.message}（${parsed.hint}）` : parsed.message) ||
          '下载中断，未能校验完整性'
        patch({ kind: 'failed', reason })
        useUIStore.getState().toast({
          kind: 'error',
          title: `DSH ${version.name} 安装失败`,
          message: reason,
          action: { label: '重试', run: () => get().installVersion(id) },
        })
      }
    } finally {
      releaseTransferSlot(`v:${id}`)
      controllers.delete(`v:${id}`)
    }
  },

  cancelVersion(id) {
    controllers.get(`v:${id}`)?.abort()
  },

  async removeVersion(id) {
    try {
      await repository.removeVersion(id)
    } catch (err) {
      console.warn('[phl] removeVersion failed:', err)
      // Tauri rejections arrive as plain strings — `instanceof Error` would
      // drop the backend's actual reason and show the generic fallback. The
      // backend codes common failures; surface the hint next to the message.
      const parsed = parseThrownError(err)
      const message = parsed.code
        ? `${parsed.message}（${parsed.hint}）`
        : parsed.message || '版本目录无法移除，可能被其他程序占用。'
      useUIStore.getState().toast({
        kind: 'error',
        title: '删除版本失败',
        message,
        duration: 6000,
      })
      return
    }
    set({
      versions: get().versions.map((v) => (v.id === id ? { ...v, state: { kind: 'available' } } : v)),
    })
  },

  /* ---------------- runtimes ---------------- */

  async installRuntime(id) {
    const runtime = get().runtimes.find((r) => r.id === id)
    if (!runtime) return
    const controller = new AbortController()
    controllers.set(`r:${id}`, controller)

    const patch = (state: Runtime['state']) =>
      set({ runtimes: get().runtimes.map((r) => (r.id === id ? { ...r, state } : r)) })
    const startedAt = Date.now()

    patch({ kind: 'queued' })
    try {
      if (!(await acquireTransferSlot(`r:${id}`, controller.signal))) throw new Cancelled()
      await repository.installRuntime(
        runtime,
        (p) => {
          if (p.stage === 'downloading') {
            patch({
              kind: 'downloading',
              progress: p.progress ?? 0,
              bytesDone: p.bytesDone ?? 0,
              bytesPerSec: p.bytesPerSec ?? 0,
            })
            maybeHintMirror(
              () =>
                Date.now() - startedAt > 15_000 &&
                (p.bytesDone ?? 0) / Math.max(Date.now() - startedAt, 1) < OFFICIAL_HINT_MIN_AVERAGE,
            )
          } else {
            // `verifying` has no progress field; defaulting keeps the bar from
            // going NaN between download and extract.
            patch({ kind: 'extracting', progress: p.progress ?? 0 })
          }
        },
        controller.signal,
      )
      patch({ kind: 'installed', installedAt: new Date().toISOString() })
      useUIStore.getState().toast({ kind: 'success', title: `${runtime.name} 安装完成` })
    } catch (err) {
      if (err instanceof Cancelled) {
        patch({ kind: 'available' })
      } else {
        patch({ kind: 'failed', reason: '下载失败' })
        useUIStore.getState().toast({ kind: 'error', title: `${runtime.name} 安装失败` })
      }
    } finally {
      releaseTransferSlot(`r:${id}`)
      controllers.delete(`r:${id}`)
    }
  },

  cancelRuntime(id) {
    controllers.get(`r:${id}`)?.abort()
  },

  async removeRuntime(id) {
    await repository.removeRuntime(id)
    set({
      runtimes: get().runtimes.map((r) => (r.id === id ? { ...r, state: { kind: 'available' } } : r)),
    })
  },

  /* ---------------- plugins ---------------- */

  /**
   * Installs a plugin into one instance. Progress lives in `pluginTransfers`
   * keyed by `p:<instanceId>:<pluginId>` — never on the catalog `Plugin`,
   * which is shared across instances and may be installing into several
   * places at once.
   */
  async installPlugin(instanceId, pluginId) {
    const instance = useInstanceStore.getState().byId(instanceId)
    const plugin = get().pluginById(pluginId)
    if (!instance || !plugin) return
    // An existing install means this is an update: the pipeline replaces the
    // package directory, so only concurrent transfers guard.
    const key = pluginKey(instanceId, pluginId)
    if (get().pluginTransfers[key]) return

    const controller = new AbortController()
    controllers.set(key, controller)

    const patch = (t: PluginTransferState) =>
      set({ pluginTransfers: { ...get().pluginTransfers, [key]: t } })

    patch({ stage: 'queued', progress: 0, bytesDone: 0, bytesPerSec: 0 })
    try {
      if (!(await acquireTransferSlot(key, controller.signal))) throw new Cancelled()
      patch({ stage: 'preparing', progress: 0, bytesDone: 0, bytesPerSec: 0 })
      const { version, registryId } = await repository.installPlugin(
        plugin,
        instance,
        (p) =>
          // `preparing` and `verifying` carry no numbers — they are unit
          // variants on the Rust side — so the fields have to be defaulted
          // rather than copied. Reading them straight through produced
          // `undefined`, which rendered as a NaN-width bar and a literal
          // "NaN%" caption. The mock always supplies numbers, so this was
          // invisible in `npm run dev`.
          patch({
            stage:
              p.stage === 'extracting' || p.stage === 'installing-deps'
                ? 'installing'
                : p.stage,
            progress: p.progress ?? 0,
            bytesDone: p.bytesDone ?? 0,
            bytesPerSec: p.bytesPerSec ?? 0,
          }),
        controller.signal,
      )
      patchInstancePlugins(instanceId, (plugins) =>
        plugins.some((ip) => ip.pluginId === pluginId)
          ? plugins.map((ip) =>
              ip.pluginId === pluginId ? { ...ip, version, registryId, enabled: true } : ip,
            )
          : [...plugins, { pluginId, version, registryId, enabled: true }],
      )
      useUIStore.getState().toast({
        kind: 'success',
        title: `已安装到「${instance.name}」`,
        message: `${plugin.name} ${version}`,
      })
    } catch (err) {
      if (err instanceof Cancelled) {
        useUIStore.getState().toast({ kind: 'info', title: `已取消安装 ${plugin.name}` })
      } else {
        const reason = err instanceof Error && err.message ? err.message : String(err)
        useUIStore.getState().toast({
          kind: 'error',
          title: `${plugin.name} 安装失败`,
          message: reason,
          action: { label: '重试', run: () => void get().installPlugin(instanceId, pluginId) },
        })
      }
    } finally {
      releaseTransferSlot(key)
      const rest = { ...get().pluginTransfers }
      delete rest[key]
      set({ pluginTransfers: rest })
      controllers.delete(key)
    }
  },

  cancelPlugin(instanceId, pluginId) {
    controllers.get(pluginKey(instanceId, pluginId))?.abort()
  },

  /** Resolves the newest published version for every installed plugin once. */
  async refreshLatestVersions(instanceId) {
    const instance = useInstanceStore.getState().byId(instanceId)
    if (!instance) return
    const pending = instance.plugins.filter(
      (ip) =>
        !ip.linked &&
        get().latestVersions[ip.pluginId] === undefined &&
        get().pluginById(ip.pluginId),
    )
    if (pending.length === 0) return
    set({
      latestVersions: {
        ...get().latestVersions,
        ...Object.fromEntries(pending.map((ip) => [ip.pluginId, null])),
      },
    })
    const resolved = await Promise.all(
      pending.map(async (ip) => {
        const plugin = get().pluginById(ip.pluginId)!
        try {
          return [ip.pluginId, await repository.latestPluginVersion(plugin)] as const
        } catch {
          return [ip.pluginId, null] as const
        }
      }),
    )
    set({
      latestVersions: { ...get().latestVersions, ...Object.fromEntries(resolved) },
    })
  },

  /** Flips `disabled` in cordis.patch.yml and mirrors it on the instance. */
  async setPluginEnabled(instanceId, pluginId, enabled) {
    const instance = useInstanceStore.getState().byId(instanceId)
    if (!instance) return
    const installed = instance.plugins.find((ip) => ip.pluginId === pluginId)
    if (!installed) return
    const label = get().pluginById(pluginId)?.name ?? pluginId
    // Linked plugins are local dev folders — there is nothing to patch on
    // disk, so only the instance record changes. Everything else *must* reach
    // disk: gating this on catalog metadata (absent whenever the registry is
    // unreachable) turned the switch into a no-op that still reported success,
    // while `cordis.patch.yml` — the file DSH actually reads — went untouched.
    if (!installed.linked) {
      try {
        await repository.setPluginEnabled(instance, installed, enabled)
      } catch (err) {
        useUIStore.getState().toast({
          kind: 'error',
          title: `${label} ${enabled ? '启用' : '停用'}失败`,
          message: err instanceof Error && err.message ? err.message : String(err),
        })
        return
      }
    }
    patchInstancePlugins(instanceId, (plugins) =>
      plugins.map((p) => (p.pluginId === pluginId ? { ...p, enabled } : p)),
    )
  },

  /** Removes the package from the profile and the instance record. */
  async uninstallPlugin(instanceId, pluginId) {
    const instance = useInstanceStore.getState().byId(instanceId)
    if (!instance) return
    const removed = instance.plugins.find((ip) => ip.pluginId === pluginId)
    if (!removed) return
    const label = get().pluginById(pluginId)?.name ?? pluginId
    if (!removed.linked) {
      try {
        await repository.uninstallPlugin(instance, removed)
      } catch (err) {
        useUIStore.getState().toast({
          kind: 'error',
          title: `${label} 卸载失败`,
          message: err instanceof Error && err.message ? err.message : String(err),
        })
        return
      }
    }
    patchInstancePlugins(instanceId, (plugins) =>
      plugins.filter((p) => p.pluginId !== pluginId),
    )
    useUIStore.getState().toast({
      kind: 'info',
      title: `已从「${instance.name}」移除 ${label}`,
    })
  },

  /* ---------------- lookups ---------------- */

  versionById: (id) => get().versions.find((v) => v.id === id),
  runtimeById: (id) => get().runtimes.find((r) => r.id === id),
  pluginById: (id) => get().plugins.find((p) => p.id === id),
  activeTransfers: () =>
    get().versions.filter((v) => ['downloading', 'extracting', 'verifying', 'queued'].includes(v.state.kind))
      .length +
    get().runtimes.filter((r) => ['downloading', 'extracting'].includes(r.state.kind)).length +
    Object.keys(get().pluginTransfers).length,
}))
