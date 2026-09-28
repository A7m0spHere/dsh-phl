import { parseThrownError } from '@/lib/errorCodes'
import { create } from 'zustand'
import {
  choosePackOpenPath,
  installPack,
  installCommunityPack,
  previewPack,
  previewCommunityPack,
  type CommunityPackPreview,
  type PackInstallRequest,
  type RemotePackInstallOutcome,
  type RemotePackPreview,
} from '@/lib/desktop'
import { instanceFromRecord, newInstanceId } from '@/services/tauriInstances'
import { isDesktop } from '@/lib/desktopCore'
import { toBoundVersionId } from '@/lib/instanceVersion'
import { useInstanceStore } from './instanceStore'
import { useUIStore } from './uiStore'

/**
 * State for "安装整合包" (P1-4). A transient flow like the adoption wizard:
 * pick a `.phlpack` → preview (validated + dependency-resolved) → install
 * (staged, atomic). The committed instance goes to `instanceStore.admitInstance`
 * through the same door as Bundle import.
 *
 * P1-4 keeps PHL as the config source: the pack's embedded environment is
 * applied by Rust, and remote plugins come back as `deferredPlugins` for the
 * instance's plugin page to install — never auto-fetched inside this store.
 */

export type PackStep = 'pick' | 'preview' | 'progress'
type PreviewState = 'idle' | 'loading' | 'ready' | 'error'

interface PackState {
  step: PackStep
  path: string | null
  /** Which family the picked file is — decided by content, not extension. */
  mode: 'phlpack' | 'community' | null
  preview: RemotePackPreview | null
  communityPreview: CommunityPackPreview | null
  /** Explicit DSH version choice for packs that do not pin one. */
  communityVersion: string
  previewState: PreviewState
  previewError: string | null
  /** Instance name the user may override before install (defaults to pack name). */
  name: string
  installing: boolean
  installError: string | null
  progress: number

  canRun: () => boolean
  pickPack: () => Promise<void>
  setName: (name: string) => void
  setCommunityVersion: (v: string) => void
  back: () => void
  install: () => Promise<void>
  installCommunity: () => Promise<void>
  cancelInstall: () => void
  reset: () => void
}

let installController: AbortController | null = null
let previewGeneration = 0

/** Identity + naming fields; Rust owns version/runtime/profile/env. */
function requestManifest(preview: RemotePackPreview, name: string): PackInstallRequest['manifest'] {
  return {
    id: newInstanceId(name || preview.name),
    name: (name || preview.name).trim(),
    note: '来自整合包',
    kind: 'sandbox',
    hue: 0,
    // Canonical id, not the pack's bare version string — the same phantom-
    // binding bug that adoption had (Rust normalizes this too, on re-install).
    versionId: toBoundVersionId(preview.dshVersion),
    runtimeId: preview.runtime,
    // A real suggested port, not `0` (same trap the adoption draft had,
    // 2026-09-11: the launch auto-scan advanced 0 → 1, a port Chromium
    // refuses to open — ERR_UNSAFE_PORT).
    port: useInstanceStore.getState().suggestPort(),
    autoPort: true,
    profile: 'web',
    createdAt: new Date().toISOString(),
    lastRunAt: null,
    totalRuntime: 0,
    favorite: false,
    env: {},
    args: [],
    api: { inheritance: 'default', providerIds: [] },
  }
}

export const usePackStore = create<PackState>((set, get) => ({
  step: 'pick',
  path: null,
  mode: null,
  preview: null,
  communityPreview: null,
  communityVersion: '',
  previewState: 'idle',
  previewError: null,
  name: '',
  installing: false,
  installError: null,
  progress: 0,

  canRun() {
    return isDesktop
  },

  async pickPack() {
    if (get().installing) return
    const generation = ++previewGeneration
    const path = await choosePackOpenPath()
    if (!path || generation !== previewGeneration) return
    set({ path, step: 'preview', previewState: 'loading', preview: null, communityPreview: null, mode: null, previewError: null })
    try {
      // Content decides, not the extension: a .phlpack names phlpack.json,
      // a .dspack names dspack.json + manifest.json. Try ours, then theirs.
      const preview = await previewPack(path)
      if (generation !== previewGeneration) return
      set({ preview, mode: 'phlpack', previewState: 'ready', name: preview.name })
    } catch (phlpackError) {
      if (generation !== previewGeneration) return
      try {
        const community = await previewCommunityPack(path)
        if (generation !== previewGeneration) return
        set({
          communityPreview: community,
          mode: 'community',
          previewState: 'ready',
          name: community.displayName || community.name,
        })
      } catch (err2) {
        if (generation !== previewGeneration) return
        // Neither family parsed the file: the user deserves both reasons,
        // with the first (their likely intent) leading.
        const first = parseThrownError(phlpackError).message
        const second = parseThrownError(err2).message
        set({
          previewState: 'error',
          previewError: second === first ? first : `${first}（按社区包解析：${second}）`,
        })
      }
    }
  },

  setName(name) {
    set({ name })
  },

  setCommunityVersion(v) {
    set({ communityVersion: v })
  },

  back() {
    const s = get()
    if (s.step === 'preview') {
      previewGeneration += 1
      set({ step: 'pick', path: null, preview: null, communityPreview: null, mode: null, previewState: 'idle', previewError: null })
      return
    }
    // A *failed* install leaves `step` on 'progress'. "返回修改" must walk back
    // to the already-validated preview — keeping the chosen pack and the edited
    // name so the user can fix and retry — clearing the stale error and progress
    // so the retried install starts clean. Guarded on `!installing`: an
    // in-flight install can never be walked back into a second submit (§R4).
    if (s.step === 'progress' && !s.installing && s.installError) {
      set({ step: 'preview', installError: null, progress: 0 })
    }
  },

  async install() {
    const { path, preview, name } = get()
    if (!path || !preview || get().installing) return
    const ui = useUIStore.getState()
    const req: PackInstallRequest = { manifest: requestManifest(preview, name), allowMissing: true }
    set({ step: 'progress', installing: true, installError: null, progress: 0 })
    const controller = new AbortController()
    installController = controller
    try {
      const outcome: RemotePackInstallOutcome = await installPack(
        path,
        req,
        (p) => { if (installController === controller) set({ progress: p.progress }) },
        controller.signal,
      )
      useInstanceStore.getState().admitInstance(instanceFromRecord(outcome.record))
      await useInstanceStore.getState().load()
      if (installController !== controller) return
      set({ installing: false })
      const notes: string[] = []
      if (outcome.sessionsImported > 0) notes.push(`导入 ${outcome.sessionsImported} 条历史对话`)
      if (outcome.deferredPlugins.length > 0)
        notes.push(`${outcome.deferredPlugins.length} 个远程插件需在插件页安装`)
      if (outcome.credentialNames.length > 0)
        notes.push(`需重新配置凭据：${outcome.credentialNames.join('、')}`)
      ui.toast({
        kind: 'success',
        title: `已安装「${outcome.record.name}」`,
        message: notes.length > 0 ? notes.join(' · ') : '整合包已安装为可启动的实例。',
      })
    } catch (err) {
      if (installController !== controller) return
      const message = parseThrownError(err).message
      set({ installing: false, installError: message === 'cancelled' ? '安装已取消' : message })
    } finally {
      if (installController === controller) installController = null
    }
  },

  cancelInstall() {
    installController?.abort()
  },

  async installCommunity() {
    const { path, communityPreview, communityVersion, name } = get()
    if (!path || !communityPreview || get().installing) return
    const ui = useUIStore.getState()
    const dshVersion = communityPreview.dshVersion ?? communityVersion
    if (!dshVersion) return
    const manifest = requestManifest(
      { name: communityPreview.name, dshVersion, runtime: 'node-system' } as RemotePackPreview,
      name,
    )
    set({ step: 'progress', installing: true, installError: null, progress: 0 })
    const controller = new AbortController()
    installController = controller
    try {
      const outcome = await installCommunityPack(
        {
          instance: manifest,
          path,
          packSha256: communityPreview.packSha256,
          dshVersion,
          installDependencies: true,
          allowMissing: false,
        },
        (p) => { if (installController === controller) set({ progress: p.progress }) },
        controller.signal,
      )
      useInstanceStore.getState().admitInstance(instanceFromRecord(outcome.record))
      await useInstanceStore.getState().load()
      if (installController !== controller) return
      set({ installing: false })
      const notes: string[] = []
      if (outcome.readiness === 'needsDependencies')
        notes.push('依赖未完成，实例保持待补依赖状态，可在实例详情重试')
      if (outcome.dependencyFailures.length > 0)
        notes.push(outcome.dependencyFailures.slice(0, 3).join('；'))
      ui.toast({
        kind: outcome.readiness === 'readyToLaunch' ? 'success' : 'warn',
        title: `已安装「${outcome.record.name}」`,
        message: notes.length > 0 ? notes.join(' · ') : '社区整合包已安装。',
      })
    } catch (err) {
      if (installController !== controller) return
      const message = parseThrownError(err).message
      set({ installing: false, installError: message === 'cancelled' ? '安装已取消' : message })
    } finally {
      if (installController === controller) installController = null
    }
  },

  reset() {
    previewGeneration += 1
    installController?.abort()
    installController = null
    set({
      step: 'pick',
      path: null,
      preview: null,
      communityPreview: null,
      mode: null,
      communityVersion: '',
      previewState: 'idle',
      previewError: null,
      name: '',
      installing: false,
      installError: null,
      progress: 0,
    })
  },
}))
