import { parseThrownError } from '@/lib/errorCodes'
import { create } from 'zustand'
import {
  adoptInstance,
  chooseDshExecutable,
  chooseDshHome,
  discoverDsh,
  inspectDshExecutable,
  inspectDshHome,
  listHomeSessions,
  previewAdoption,
  type AdoptionMode,
  type AdoptionSessionStrategy,
  type RemoteAdoptionPreview,
  type RemoteDshCandidate,
  type RemoteSessionInfo,
} from '@/lib/desktop'
import { instanceFromRecord, newInstanceId } from '@/services/tauriInstances'
import { useInstanceStore } from './instanceStore'
import { useUIStore } from './uiStore'

/**
 * State for "接入本机 DSH": the discovery scan plus the adoption wizard that
 * follows it. Split from `instanceStore` because it is a transient flow (a
 * scan and a draft), not part of the persisted instance model — but its
 * commit hands the new instance straight to `instanceStore.admitInstance`,
 * the same door the bundle importer uses.
 *
 * Discovery and adoption are desktop-only (they touch the real filesystem).
 * In a browser the scan returns empty and the wizard shows a clear notice
 * instead of faking results — per the project rule that browser mode may
 * degrade features but must never pretend they ran.
 */

export type AdoptionStep = 'discover' | 'configure' | 'preview' | 'progress'
type ScanState = 'idle' | 'scanning' | 'error'
type PreviewState = 'idle' | 'loading' | 'ready' | 'error'

interface AdoptionState {
  step: AdoptionStep
  candidates: RemoteDshCandidate[]
  scanState: ScanState
  scanError: string | null
  selectedId: string | null

  // Draft, filled on the configure step.
  name: string
  mode: AdoptionMode
  sessionStrategy: AdoptionSessionStrategy
  // The "选择对话" picker: the source home's conversations (lazily listed)
  // and the session dir names the user checked. Only read when the strategy
  // is `selected`.
  sourceSessions: RemoteSessionInfo[] | null
  sourceSessionsError: string | null
  selectedSessionDirs: string[]

  preview: RemoteAdoptionPreview | null
  previewState: PreviewState
  previewError: string | null

  submitting: boolean
  submitError: string | null
  progress: number

  selected: () => RemoteDshCandidate | undefined
  reset: () => void
  rescan: () => Promise<void>
  addHomeManually: () => Promise<void>
  addExecutableManually: () => Promise<void>
  select: (id: string) => void
  back: () => void
  setName: (name: string) => void
  setMode: (mode: AdoptionMode) => void
  setSessionStrategy: (s: AdoptionSessionStrategy) => void
  loadSourceSessions: () => Promise<void>
  toggleSessionDir: (dir: string) => void
  toConfigure: () => void
  toPreview: () => Promise<void>
  commit: () => Promise<void>
}

/** Merge a fresh/inspected candidate into the list, replacing by id. */
function upsert(list: RemoteDshCandidate[], next: RemoteDshCandidate): RemoteDshCandidate[] {
  const without = list.filter((c) => c.id !== next.id)
  return [...without, next]
}

export const useAdoptionStore = create<AdoptionState>((set, get) => ({
  step: 'discover',
  candidates: [],
  scanState: 'idle',
  scanError: null,
  selectedId: null,
  name: '',
  mode: 'managed-copy',
  sessionStrategy: 'all',
  sourceSessions: null,
  sourceSessionsError: null,
  selectedSessionDirs: [],
  preview: null,
  previewState: 'idle',
  previewError: null,
  submitting: false,
  submitError: null,
  progress: 0,

  selected() {
    return get().candidates.find((c) => c.id === get().selectedId)
  },

  reset() {
    set({
      step: 'discover',
      selectedId: null,
      name: '',
      mode: 'managed-copy',
      sessionStrategy: 'all',
      sourceSessions: null,
      sourceSessionsError: null,
      selectedSessionDirs: [],
      preview: null,
      previewState: 'idle',
      previewError: null,
      submitting: false,
      submitError: null,
      progress: 0,
    })
  },

  async rescan() {
    set({ scanState: 'scanning', scanError: null })
    try {
      const candidates = await discoverDsh()
      set({ candidates, scanState: 'idle' })
    } catch (err) {
      set({
        scanState: 'error',
        scanError: parseThrownError(err).message,
      })
    }
  },

  async addHomeManually() {
    const path = await chooseDshHome()
    if (!path) return
    const ui = useUIStore.getState()
    try {
      const candidate = await inspectDshHome(path)
      set({ candidates: upsert(get().candidates, candidate), selectedId: candidate.id })
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '该目录不是可接入的 DSH',
        message: parseThrownError(err).message,
      })
    }
  },

  async addExecutableManually() {
    const path = await chooseDshExecutable()
    if (!path) return
    const ui = useUIStore.getState()
    try {
      const candidate = await inspectDshExecutable(path)
      set({ candidates: upsert(get().candidates, candidate), selectedId: candidate.id })
    } catch (err) {
      ui.toast({
        kind: 'error',
        title: '无法识别该 DSH 可执行文件',
        message: parseThrownError(err).message,
      })
    }
  },

  select(id) {
    // A different source home invalidates the lazily-listed conversations.
    if (get().selectedId !== id) {
      set({
        selectedId: id,
        sourceSessions: null,
        sourceSessionsError: null,
        selectedSessionDirs: [],
      })
    } else {
      set({ selectedId: id })
    }
  },

  back() {
    const step = get().step
    if (step === 'preview') set({ step: 'configure' })
    else if (step === 'configure') set({ step: 'discover' })
  },

  setName(name) {
    set({ name })
  },

  setMode(mode) {
    // External in-place adoption always keeps the home's own history; force the
    // strategy so the preview never computes a meaningless exclusion.
    set(mode === 'external' ? { mode, sessionStrategy: 'all' } : { mode })
  },

  setSessionStrategy(s) {
    set({ sessionStrategy: s })
    // Lazily list the source's conversations the first time the picker opens.
    if (s === 'selected' && get().sourceSessions === null && !get().sourceSessionsError) {
      void get().loadSourceSessions()
    }
  },

  async loadSourceSessions() {
    const candidate = get().selected()
    if (!candidate) return
    set({ sourceSessions: null, sourceSessionsError: null })
    try {
      const sessions = await listHomeSessions(candidate.dshHome)
      set({ sourceSessions: sessions })
    } catch (err) {
      set({
        sourceSessionsError: parseThrownError(err).message,
      })
    }
  },

  toggleSessionDir(dir) {
    set((s) => ({
      selectedSessionDirs: s.selectedSessionDirs.includes(dir)
        ? s.selectedSessionDirs.filter((d) => d !== dir)
        : [...s.selectedSessionDirs, dir],
    }))
  },

  toConfigure() {
    const candidate = get().selected()
    if (!candidate) return
    const ui = useUIStore.getState()
    if (candidate.alreadyManaged) {
      ui.toast({ kind: 'warn', title: '该环境已被实例管理', message: '无需再次接入。' })
      return
    }
    // Seed the name from the display label; strip a leading `~/.` noise.
    if (!get().name) set({ name: candidate.displayName.replace(/^~[\\/]/, '') || '接入的 DSH' })
    set({ step: 'configure' })
  },

  async toPreview() {
    const candidate = get().selected()
    if (!candidate) return
    const { name, mode, sessionStrategy, selectedSessionDirs } = get()
    // The picker's own contract: "选择对话" without any check is a user error,
    // not a backend round-trip.
    if (sessionStrategy === 'selected' && selectedSessionDirs.length === 0) return
    set({ step: 'preview', previewState: 'loading', previewError: null, preview: null })
    try {
      const preview = await previewAdoption({
        sourceHome: candidate.dshHome,
        mode,
        sessionStrategy,
        sessionDirs: sessionStrategy === 'selected' ? selectedSessionDirs : [],
        manifest: adoptionManifest(candidate, name),
      })
      set({ preview, previewState: 'ready' })
    } catch (err) {
      set({ previewState: 'error', previewError: parseThrownError(err).message })
    }
  },

  async commit() {
    const candidate = get().selected()
    const preview = get().preview
    if (!candidate || !preview) return
    const ui = useUIStore.getState()
    const { selectedSessionDirs } = get()
    set({ step: 'progress', submitting: true, submitError: null, progress: 0 })
    try {
      const outcome = await adoptInstance(
        {
          sourceHome: candidate.dshHome,
          mode: preview.mode,
          sessionStrategy: preview.sessionStrategy,
          sessionDirs: preview.sessionStrategy === 'selected' ? selectedSessionDirs : [],
          manifest: adoptionManifest(candidate, get().name),
        },
        (p) => set({ progress: p.progress }),
      )
      useInstanceStore.getState().admitInstance(instanceFromRecord(outcome.record))
      await useInstanceStore.getState().load()
      set({ submitting: false })
      ui.toast({
        kind: 'success',
        title: `已接入「${outcome.record.name}」`,
        message:
          preview.mode === 'external'
            ? '原地接入完成：历史对话与配置保持不变。'
            : preview.sessionStrategy === 'none'
              ? '复制完成：历史对话未迁移，环境已隔离到 PHL。'
              : preview.sessionStrategy === 'selected'
                ? `复制完成：已迁移 ${preview.selectedSessionCount} 条选定的历史对话。`
                : '复制完成：历史对话随环境一起接入。',
      })
    } catch (err) {
      set({ submitting: false, submitError: parseThrownError(err).message })
    }
  },
}))

/**
 * The identity + environment manifest adoption persists. Only the fields the
 * user names here are sent; Rust owns the management fields (mode, external
 * home, provenance). `versionId` borrows the detected version so the version
 * flow can offer to install it; `runtimeId` maps the detected Node major to
 * PHL's `node-<major>` runtime, falling back to `node-system` (the runtime
 * entry that runs `node` off PATH) when the version is unknown — `node` alone
 * is not a runtime id and would never resolve.
 */
function adoptionManifest(
  candidate: RemoteDshCandidate,
  name: string,
): import('@/lib/desktop').RemoteInstanceManifest {
  return {
    id: newInstanceId(name || candidate.displayName),
    name: (name || candidate.displayName).trim(),
    note: '接入的本机 DSH',
    kind: 'sandbox',
    hue: 0,
    versionId: candidate.detectedVersion ?? '',
    runtimeId: candidate.nodeVersion
      ? `node-${candidate.nodeVersion.split('.')[0]}`
      : 'node-system',
    port: 0,
    autoPort: true,
    profile: candidate.profile || 'web',
    createdAt: new Date().toISOString(),
    lastRunAt: null,
    totalRuntime: 0,
    favorite: false,
    env: {},
    args: [],
    // Adoption never auto-binds the global library (spec §12, §3.2): the
    // adopted environment already carries its own configuration.
    api: null,
  }
}
