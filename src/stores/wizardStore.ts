import { create } from 'zustand'
import type { InstanceDraft } from '@/types'

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
  // New instances inherit the global API library by default; the wizard's
  // API step lets the user opt out.
  apiInheritance: 'default',
})

interface WizardState {
  draft: InstanceDraft
  reset: (patch?: Partial<InstanceDraft>) => void
  patch: (patch: Partial<InstanceDraft>) => void
}

export const useWizardStore = create<WizardState>()((set, get) => ({
  draft: emptyDraft(),

  reset: (patch) => set({ draft: { ...emptyDraft(), ...patch } }),

  patch: (patch) => set({ draft: { ...get().draft, ...patch } }),
}))

/**
 * Everything that blocks submission, keyed by the page section it belongs to.
 * The create page surfaces each entry inline next to its section *and* as a
 * footer summary, so a missing required choice is called out in both places.
 */
export interface DraftIssues {
  name?: string
  version?: string
  runtime?: string
  port?: string
}

export function draftIssues(draft: InstanceDraft, takenNames: string[]): DraftIssues {
  const issues: DraftIssues = {}
  if (!draft.name.trim()) issues.name = '请填写实例名称'
  else if (takenNames.includes(draft.name.trim())) issues.name = '已存在同名实例'
  if (!draft.versionId) issues.version = '请选择一个 DSH 版本'
  if (!draft.runtimeId) issues.runtime = '请选择一个 Node Runtime'
  if (!draft.autoPort && (draft.port < 1024 || draft.port > 65535))
    issues.port = '端口需要在 1024 – 65535 之间'
  return issues
}
