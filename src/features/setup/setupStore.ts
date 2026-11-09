import { create } from 'zustand'
import { repository, Cancelled } from '@/services'
import { isDesktop } from '@/lib/desktop'
import { parseThrownError } from '@/lib/errorCodes'
import { isRuntimeCompatible, pickDefaultRuntime, pickSetupVersion } from '@/lib/wizardDefaults'
import { isRuntimeBusy, isVersionBusy, type InstanceDraft } from '@/types'
import { useCatalogStore } from '@/stores/catalogStore'
import { useInstanceStore } from '@/stores/instanceStore'
import { useSettingsStore } from '@/stores/settingsStore'
import { draftIssues } from '@/stores/wizardStore'

export type SetupStep = 'runtime' | 'version' | 'create' | 'launch' | 'open'
type SetupStatus = 'idle' | 'running' | 'cancelling' | 'failed' | 'cancelled' | 'complete'

interface SetupState {
  open: boolean
  status: SetupStatus
  step: SetupStep
  name: string
  versionId: string
  runtimeId: string
  root: string
  instanceId: string | null
  error: string | null
  progress: number | null
  detail: string
  show: () => void
  close: () => void
  configure: (patch: { name?: string; versionId?: string; runtimeId?: string }) => void
  run: () => Promise<void>
  cancel: () => void
}

let controller: AbortController | null = null
let transfer: { kind: 'runtime' | 'version'; id: string } | null = null

export const setupIsBusy = (status: SetupStatus) => status === 'running' || status === 'cancelling'

export const useSetupStore = create<SetupState>()((set, get) => ({
  open: false,
  status: 'idle',
  step: 'runtime',
  name: '',
  versionId: '',
  runtimeId: '',
  root: '',
  instanceId: null,
  error: null,
  progress: null,
  detail: '',

  show() {
    if (setupIsBusy(get().status)) return
    const root = useSettingsStore.getState().root
    const previous = get().instanceId
    if (root === get().root && previous && useInstanceStore.getState().byId(previous)) {
      set({ open: true })
      return
    }
    const catalog = useCatalogStore.getState()
    const version = pickSetupVersion(catalog.versions, catalog.runtimes)
    const runtime = version && pickDefaultRuntime(catalog.runtimes, version)
    const names = new Set(useInstanceStore.getState().instances.map((instance) => instance.name))
    let name = '我的 DSH'
    let suffix = 2
    while (names.has(name)) name = `我的 DSH ${suffix++}`
    set({
      open: true, status: 'idle', step: 'runtime', name, root,
      versionId: version?.id ?? '', runtimeId: runtime?.id ?? '',
      instanceId: null, error: null, progress: null, detail: '',
    })
  },

  close() {
    if (setupIsBusy(get().status)) return
    useSettingsStore.getState().markGuideSeen()
    set({ open: false })
  },

  configure(patch) {
    if (setupIsBusy(get().status) || get().instanceId) return
    if (patch.versionId !== undefined) {
      const catalog = useCatalogStore.getState()
      const version = catalog.versions.find((candidate) => candidate.id === patch.versionId)
      const runtime = catalog.runtimes.find((candidate) => candidate.id === get().runtimeId)
      patch = {
        ...patch,
        runtimeId: version && runtime && isRuntimeCompatible(runtime, version)
          ? runtime.id
          : version ? pickDefaultRuntime(catalog.runtimes, version)?.id ?? '' : '',
      }
    }
    set({ ...patch, status: 'idle', error: null })
  },

  async run() {
    if (setupIsBusy(get().status)) return
    const config = get()
    const attempt = new AbortController()
    controller = attempt
    set({ status: 'running', error: null, progress: null, detail: '' })
    let creating = false
    const check = () => {
      if (attempt.signal.aborted) throw new Cancelled()
      if (useSettingsStore.getState().root !== config.root) {
        throw new Error('数据目录已变更，请重新打开一键装好向导。')
      }
    }

    try {
      check()
      const catalog = useCatalogStore.getState()
      const version = catalog.versions.find((candidate) => candidate.id === config.versionId)
      const runtime = catalog.runtimes.find((candidate) => candidate.id === config.runtimeId)
      if (!version || !runtime) throw new Error('请选择可用的 DSH 版本和 Node，目录为空时可重新加载。')
      if (version.pendingPublish) throw new Error('所选 DSH 版本尚未上架 npm，请改选可下载的版本。')
      if (!isRuntimeCompatible(runtime, version)) throw new Error('所选 Node 与 DSH 的版本要求不匹配，请重新选择。')
      if (isVersionBusy(version) || isRuntimeBusy(runtime)) {
        throw new Error('所选组件有其他任务正在执行，请等待任务结束后重试。')
      }

      const draft: InstanceDraft = {
        name: config.name.trim(), note: '', kind: 'production', hue: 0,
        versionId: version.id, runtimeId: runtime.id, templateId: 'blank',
        autoPort: true, port: useInstanceStore.getState().suggestPort(),
        copyFromId: null, apiInheritance: 'default',
      }
      if (!config.instanceId) {
        const issues = draftIssues(draft, useInstanceStore.getState().instances.map((instance) => instance.name), version)
        const issue = Object.values(issues)[0]
        if (issue) throw new Error(issue)
      }

      set({ step: 'runtime' })
      if (runtime.state.kind !== 'installed') {
        transfer = { kind: 'runtime', id: runtime.id }
        await catalog.installRuntime(runtime.id)
        transfer = null
        check()
        const state = useCatalogStore.getState().runtimeById(runtime.id)?.state
        if (state?.kind !== 'installed') throw new Error(state?.kind === 'failed' ? state.reason : 'Node 安装尚未完成，请重试。')
      }

      check()
      set({ step: 'version' })
      if (useCatalogStore.getState().versionById(version.id)?.state.kind !== 'installed') {
        transfer = { kind: 'version', id: version.id }
        await catalog.installVersion(version.id)
        transfer = null
        check()
        const state = useCatalogStore.getState().versionById(version.id)?.state
        if (state?.kind !== 'installed') throw new Error(state?.kind === 'failed' ? state.reason : 'DSH 安装尚未完成，请重试。')
      }

      check()
      set({ step: 'create' })
      let instanceId = config.instanceId
      if (!instanceId) {
        if (useInstanceStore.getState().createProgress) throw new Error('已有实例创建任务进行中，请稍后重试。')
        creating = true
        useInstanceStore.setState({ createProgress: { step: 'create', progress: 0 } })
        const instance = await repository.createInstance(draft, undefined, (progress) => {
          useInstanceStore.setState({ createProgress: progress })
          set({ progress: progress.progress, detail: progress.detail ?? '' })
        }, attempt.signal)
        useInstanceStore.getState().admitInstance(instance)
        instanceId = instance.id
        set({ instanceId })
        useInstanceStore.setState({ createProgress: null })
        creating = false
      }
      if (!useInstanceStore.getState().byId(instanceId)) throw new Error('实例已被移除，请重新打开向导。')

      check()
      set({ step: 'launch', progress: null, detail: '' })
      if (useInstanceStore.getState().stateOf(instanceId).status !== 'running') {
        await useInstanceStore.getState().launch(instanceId, { autoOpenWebUi: false })
      }
      check()
      const state = useInstanceStore.getState().stateOf(instanceId)
      if (state.status !== 'running') {
        throw new Error(state.error
          ? [state.error.title, state.error.detail, state.error.hint].filter(Boolean).join('\n')
          : 'DSH 尚未就绪，可重试启动或前往实例查看日志。')
      }
      if (isDesktop && !state.webUrl) throw new Error('DSH 进程仍在运行，尚未获得 WebUI 认证地址。请前往实例查看日志或停止后重试。')

      set({ step: 'open' })
      if (isDesktop) {
        const result = await useInstanceStore.getState().openWebUi(instanceId)
        if (!result.ok) throw new Error(`${result.reason}\n实例仍在运行，可以重试打开或前往实例查看详情。`)
      }
      check()
      useSettingsStore.getState().markGuideSeen()
      set({ status: 'complete', progress: 1 })
    } catch (error) {
      if (error instanceof Cancelled) {
        const id = get().instanceId
        const stopped = !id || await useInstanceStore.getState().stop(id)
        set(stopped
          ? { status: 'cancelled', error: null }
          : { status: 'failed', error: '取消后未能确认 DSH 进程退出，请前往实例重试停止。' })
      } else {
        set({ status: 'failed', error: parseThrownError(error).message })
      }
    } finally {
      if (creating) useInstanceStore.setState({ createProgress: null })
      transfer = null
      controller = null
    }
  },

  cancel() {
    if (get().status !== 'running') return
    set({ status: 'cancelling' })
    controller?.abort()
    const catalog = useCatalogStore.getState()
    if (transfer?.kind === 'runtime') catalog.cancelRuntime(transfer.id)
    if (transfer?.kind === 'version') catalog.cancelVersion(transfer.id)
    const id = get().instanceId
    if (id) useInstanceStore.getState().cancelLaunch(id)
  },
}))
