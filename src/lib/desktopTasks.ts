/**
 * Task-registry bridge (domain module of the desktop split, roadmap O-13).
 * Reads the backend task registry maintained by `resources::guarded`.
 */
import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'

export interface TaskInfo {
  id: string
  kind: string
  label: string
  resources: string[]
  phase: string
  state: 'running' | 'done' | 'failed' | 'cancelled'
  cancelRequested: boolean
  error: string | null
  startedAt: number
  finishedAt: number | null
}

export interface TaskList {
  tasks: TaskInfo[]
  /** Resource keys currently locked — a conflict can be explained with these. */
  held: string[]
}

/** Live and recent long tasks from the backend registry (single source). */
export async function listTasks(): Promise<TaskList> {
  if (!isDesktop) return { tasks: [], held: [] }
  return invoke<TaskList>('list_tasks')
}
