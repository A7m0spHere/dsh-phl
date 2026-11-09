import { useEffect } from 'react'
import { create } from 'zustand'
import { repository } from '@/services'
import { isDesktop } from '@/lib/desktop'
import { parseThrownError } from '@/lib/errorCodes'
import { useSettingsStore } from '@/stores/settingsStore'
import type { ProcessSample } from '@/types'

interface ProcessState {
  samples: Record<string, ProcessSample>
  loading: boolean
  error: string | null
  refreshedAt: number | null
  refresh: () => Promise<void>
}

let request: Promise<void> | null = null

export const useProcessStore = create<ProcessState>()((set) => ({
  samples: {}, loading: false, error: null, refreshedAt: null,
  refresh() {
    if (request) return request
    const root = useSettingsStore.getState().root
    set({ loading: true })
    request = Promise.resolve().then(async () => {
      try {
        const samples = await repository.sampleProcesses()
        if (useSettingsStore.getState().root !== root) return
        set({
          samples: Object.fromEntries(samples.map((sample) => [sample.instanceId, sample])),
          error: null, refreshedAt: Date.now(),
        })
      } catch (error) {
        if (useSettingsStore.getState().root === root) set({ error: parseThrownError(error).message })
      } finally {
        request = null
        set({ loading: false })
      }
    })
    return request
  },
}))

/** Only the visible process view samples. A slow request never stacks up. */
export function useProcessSampling(): void {
  useEffect(() => {
    if (!isDesktop) return
    let disposed = false
    let polling = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const tick = async () => {
      if (disposed || polling || document.hidden) return
      polling = true
      await useProcessStore.getState().refresh()
      polling = false
      if (!disposed && !document.hidden) timer = setTimeout(() => void tick(), 2000)
    }
    const visibility = () => {
      clearTimeout(timer)
      if (!document.hidden) void tick()
    }
    document.addEventListener('visibilitychange', visibility)
    void tick()
    return () => {
      disposed = true
      clearTimeout(timer)
      document.removeEventListener('visibilitychange', visibility)
    }
  }, [])
}
