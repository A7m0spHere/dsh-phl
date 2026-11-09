import { invoke } from '@tauri-apps/api/core'
import { isDesktop } from './desktopCore'
import type { ProcessSample } from '@/types'

export async function sampleInstanceProcesses(): Promise<ProcessSample[]> {
  if (!isDesktop) return []
  return invoke('plugin:process-monitor|sample_instance_processes')
}
