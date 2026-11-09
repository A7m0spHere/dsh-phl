/** A verified OS reading of the registered DSH root process. */
export interface ProcessSample {
  instanceId: string
  pid: number
  port: number
  status: 'running' | 'exited' | 'unknown'
  startedAtMs: number | null
  sampledAtMs: number
  /** All available logical CPUs together are 100%. */
  cpuPercent: number | null
  /** Resident bytes, including shared resident pages. */
  memoryBytes: number | null
  reason: string | null
}
