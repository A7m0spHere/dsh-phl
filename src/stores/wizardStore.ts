import { create } from 'zustand'
import type { InstanceDraft } from '@/types'

export type WizardStep = 'basics' | 'version' | 'runtime' | 'template' | 'review'

export const WIZARD_STEPS: { id: WizardStep; label: string; hint: string }[] = [
  { id: 'basics', label: '基本信息', hint: '名称与用途' },
  { id: 'version', label: 'DSH 版本', hint: '实例固定使用的版本' },
  { id: 'runtime', label: 'Runtime', hint: 'Node 版本' },
  { id: 'template', label: '模板与端口', hint: '预置插件与端口' },
  { id: 'review', label: '确认', hint: '检查隔离范围' },
]

const emptyDraft = (): InstanceDraft => ({
  name: '',
  note: '',
  kind: 'development',
  hue: Math.floor(Math.random() * 6),
  versionId: null,
  runtimeId: null,
  templateId: 'blank',
  autoPort: true,
  port: 3080,
  copyFromId: null,
})

interface WizardState {
  step: WizardStep
  draft: InstanceDraft
  /** Steps the user has already left, so we only show errors where relevant. */
  visited: Set<WizardStep>
  reset: (patch?: Partial<InstanceDraft>) => void
  setStep: (step: WizardStep) => void
  patch: (patch: Partial<InstanceDraft>) => void
  next: () => void
  prev: () => void
}

export const useWizardStore = create<WizardState>()((set, get) => ({
  step: 'basics',
  draft: emptyDraft(),
  visited: new Set<WizardStep>(),

  reset: (patch) =>
    set({ step: 'basics', draft: { ...emptyDraft(), ...patch }, visited: new Set() }),

  setStep: (step) => set({ visited: new Set(get().visited).add(get().step), step }),

  patch: (patch) => set({ draft: { ...get().draft, ...patch } }),

  next: () => {
    const index = WIZARD_STEPS.findIndex((s) => s.id === get().step)
    if (index < WIZARD_STEPS.length - 1) get().setStep(WIZARD_STEPS[index + 1].id)
  },

  prev: () => {
    const index = WIZARD_STEPS.findIndex((s) => s.id === get().step)
    if (index > 0) get().setStep(WIZARD_STEPS[index - 1].id)
  },
}))

/** Which steps are complete enough to move on from. */
export function stepStatus(
  step: WizardStep,
  draft: InstanceDraft,
  takenNames: string[],
): { ok: boolean; error?: string } {
  switch (step) {
    case 'basics':
      if (!draft.name.trim()) return { ok: false, error: '请填写实例名称' }
      if (takenNames.includes(draft.name.trim())) return { ok: false, error: '已存在同名实例' }
      return { ok: true }
    case 'version':
      return draft.versionId ? { ok: true } : { ok: false, error: '请选择一个 DSH 版本' }
    case 'runtime':
      return draft.runtimeId ? { ok: true } : { ok: false, error: '请选择一个 Node Runtime' }
    case 'template':
      if (!draft.autoPort && (draft.port < 1024 || draft.port > 65535))
        return { ok: false, error: '端口需要在 1024 – 65535 之间' }
      return { ok: true }
    default:
      return { ok: true }
  }
}
