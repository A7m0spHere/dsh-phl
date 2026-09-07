import { parseThrownError } from '@/lib/errorCodes'
import { create } from 'zustand'
import {
  choosePackOpenPath,
  installPack,
  previewPack,
  type PackInstallRequest,
  type RemotePackInstallOutcome,
  type RemotePackPreview,
} from '@/lib/desktop'
import { instanceFromRecord, newInstanceId } from '@/services/tauriInstances'
import { isDesktop } from '@/lib/desktopCore'
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
  preview: RemotePackPreview | null
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
  back: () => void
  install: () => Promise<void>
  cancelInstall: () => void
  reset: () => void
}

let installController: AbortController | null = null

/** Identity + naming fields; Rust owns version/runtime/profile/env. */
function requestManifest(preview: RemotePackPreview, name: string): PackInstallRequest['manifest'] {
  return {
    id: newInstanceId(name || preview.name),
    name: (name || preview.name).trim(),
    note: '来自整合包',
    kind: 'sandbox',
    hue: 0,
    versionId: preview.dshVersion,
    runtimeId: preview.runtime,
    port: 0,
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
  preview: null,
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
    const path = await choosePackOpenPath()
    if (!path) return
    set({ path, step: 'preview', previewState: 'loading', preview: null, previewError: null })
    try {
      const preview = await previewPack(path)
      set({ preview, previewState: 'ready', name: preview.name })
    } catch (err) {
      set({ previewState: 'error', previewError: parseThrownError(err).message })
    }
  },

  setName(name) {
    set({ name })
  },

  back() {
    const s = get()
    if (s.step === 'preview') {
      set({ step: 'pick', path: null, preview: null, previewState: 'idle', previewError: null })
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
    if (!path || !preview) return
    const ui = useUIStore.getState()
    const req: PackInstallRequest = { manifest: requestManifest(preview, name), allowMissing: true }
    set({ step: 'progress', installing: true, installError: null, progress: 0 })
    installController = new AbortController()
    try {
      const outcome: RemotePackInstallOutcome = await installPack(
        path,
        req,
        (p) => set({ progress: p.progress }),
        installController.signal,
      )
      useInstanceStore.getState().admitInstance(instanceFromRecord(outcome.record))
      await useInstanceStore.getState().load()
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
      const message = parseThrownError(err).message
      set({ installing: false, installError: message === 'cancelled' ? '安装已取消' : message })
    } finally {
      installController = null
    }
  },

  cancelInstall() {
    installController?.abort()
  },

  reset() {
    installController?.abort()
    installController = null
    set({
      step: 'pick',
      path: null,
      preview: null,
      previewState: 'idle',
      previewError: null,
      name: '',
      installing: false,
      installError: null,
      progress: 0,
    })
  },
}))
