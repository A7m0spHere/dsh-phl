import { create } from 'zustand'
import { listTasks, type TaskInfo } from '@/lib/desktop'
import { isDesktop } from '@/lib/desktop'

/**
 * The frontend view of the backend task registry (roadmap O-10). The Rust
 * `guarded()` entry is the only writer — this store never invents task state,
 * it polls the same map the conflict guard maintains, so what the task centre
 * shows and what a `[busy]` rejection referred to are guaranteed to agree.
 *
 * Polling is adaptive and desktop-only: fast while anything runs or the panel
 * is open (the user is watching), 8 s otherwise, and stopped entirely when
 * the window is hidden — a minimised PHL does no background work for this.
 */

const FAST_MS = 1500
const SLOW_MS = 8000

interface TaskState {
  tasks: TaskInfo[]
  held: string[]
  open: boolean
  setOpen: (open: boolean) => void
  refresh: () => Promise<void>
  runningCount: () => number
  /** Latest failed task per kind — the retry entry point. */
  recentFailures: () => TaskInfo[]
}

export const useTaskStore = create<TaskState>()((set, get) => ({
  tasks: [],
  held: [],
  open: false,
  setOpen: (open) => {
    set({ open })
    if (open) void get().refresh()
  },
  async refresh() {
    try {
      const list = await listTasks()
      set({ tasks: list.tasks, held: list.held })
    } catch {
      // A transient IPC failure keeps the previous snapshot; the next tick
      // catches up. A task centre must never take the app down.
    }
  },
  runningCount: () => get().tasks.filter((t) => t.state === 'running').length,
  recentFailures: () => {
    const seen = new Set<string>()
    const out: TaskInfo[] = []
    for (const t of get().tasks) {
      if (t.state !== 'failed') continue
      if (seen.has(t.kind)) continue
      seen.add(t.kind)
      out.push(t)
    }
    return out.slice(0, 5)
  },
}))

let timer: number | null = null

/** Start the adaptive poll loop once, from app boot. */
export function startTaskPolling(): void {
  if (!isDesktop || timer !== null) return
  const tick = () => {
    const store = useTaskStore.getState()
    void store.refresh()
    const fast = store.open || store.runningCount() > 0
    timer = window.setTimeout(tick, fast ? FAST_MS : SLOW_MS)
  }
  // Hidden window: pause entirely; resume on visibility.
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      if (timer !== null) window.clearTimeout(timer)
      timer = null
    } else if (timer === null) {
      tick()
    }
  })
  timer = window.setTimeout(tick, 1000)
}
