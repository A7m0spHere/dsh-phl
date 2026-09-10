import { Cancelled, repository } from '@/services'
import type { DshVersion } from '@/types'
import { parseThrownError } from '@/lib/errorCodes'
import { acquireTransferSlot, releaseTransferSlot } from './transferCoordinator'
import { maybeHintMirror, observePendingReleases, officialRegistryLooksSlow } from './catalogNotifications'
import { useUIStore } from './uiStore'
import type { CatalogGet, CatalogSet, TransferControllers, VersionActions } from './catalogTypes'

export function createVersionActions(
  set: CatalogSet,
  get: CatalogGet,
  controllers: TransferControllers,
): VersionActions {
  return {
    async refreshVersions(opts = {}) {
      const silent = !!opts.silent
      const { versionsSyncing, loading } = get()
      if (versionsSyncing || loading) return
      set({ versionsSyncing: true, versionsSyncAttemptedAt: Date.now() })
      try {
        const next = await repository.listVersions()
        const keep = new Set(['queued', 'downloading', 'extracting', 'verifying', 'failed'])
        const prev = new Map(get().versions.map((version) => [version.id, version]))
        const merged = next.map((version) => {
          const old = prev.get(version.id)
          return old && keep.has(old.state.kind) ? { ...version, state: old.state } : version
        })
        const added = merged.filter((version) => !prev.has(version.id))
        set({ versions: merged, versionsLoaded: true, versionsSyncedAt: Date.now() })
        observePendingReleases(merged, get().installVersion)
        if (!silent) {
          useUIStore.getState().toast(
            added.length
              ? {
                  kind: 'success',
                  title: added.length === 1 ? `发现新版本 ${added[0].name}` : `发现 ${added.length} 个新版本`,
                  message: added.map((version) => version.name).slice(0, 5).join('、'),
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
            message: parseThrownError(err).message || '无法连接发布源，请检查网络后重试。',
            action: { label: '重试', run: () => void get().refreshVersions() },
          })
        }
      } finally {
        set({ versionsSyncing: false })
      }
    },

    async installVersion(id) {
      const version = get().versions.find((candidate) => candidate.id === id)
      if (!version) return
      const controller = new AbortController()
      const key = `v:${id}`
      controllers.set(key, controller)
      const startedAt = Date.now()
      let depsStartedAt = 0
      const patch = (state: DshVersion['state']) =>
        set((current) => ({
          versions: current.versions.map((candidate) => (candidate.id === id ? { ...candidate, state } : candidate)),
        }))

      patch({ kind: 'queued' })
      try {
        if (!(await acquireTransferSlot(key, controller.signal))) throw new Cancelled()
        await repository.installVersion(
          version,
          (progress) => {
            if (progress.stage === 'downloading') {
              patch({
                kind: 'downloading',
                progress: progress.progress ?? 0,
                bytesDone: progress.bytesDone ?? 0,
                bytesPerSec: progress.bytesPerSec ?? 0,
              })
              maybeHintMirror(() => officialRegistryLooksSlow(startedAt, progress.bytesDone ?? 0))
            } else if (progress.stage === 'extracting') {
              patch({ kind: 'extracting', progress: progress.progress ?? 0 })
            } else if (progress.stage === 'installingDeps' || progress.stage === 'installing-deps') {
              // The wire word is camelCase (the Rust enum's serde rename); the
              // kebab form is the browser mock's. Before this matched, the
              // longest stage of a cold install fell through to "verifying"
              // and the page pinned the bar at a fake 97%.
              if (depsStartedAt === 0) depsStartedAt = Date.now()
              patch({ kind: 'installing-deps', progress: progress.progress ?? 0 })
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
          const reason = parsed.code ? `${parsed.message}（${parsed.hint}）` : parsed.message || '下载中断，未能校验完整性'
          patch({ kind: 'failed', reason })
          useUIStore.getState().toast({
            kind: 'error',
            title: `DSH ${version.name} 安装失败`,
            message: reason,
            action: { label: '重试', run: () => get().installVersion(id) },
          })
        }
      } finally {
        releaseTransferSlot(key)
        controllers.delete(key)
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
        const parsed = parseThrownError(err)
        const message = parsed.code
          ? `${parsed.message}（${parsed.hint}）`
          : parsed.message || '版本目录无法移除，可能被其他程序占用。'
        useUIStore.getState().toast({ kind: 'error', title: '删除版本失败', message, duration: 6000 })
        return
      }
      set((current) => ({
        versions: current.versions.map((version) =>
          version.id === id ? { ...version, state: { kind: 'available' } } : version,
        ),
      }))
    },
  }
}
