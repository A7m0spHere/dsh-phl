import { describe, expect, it } from 'vitest'
import { processReading } from './processStatus'
import type { ProcessSample } from '@/types'

const sample: ProcessSample = {
  instanceId: 'one', pid: 123, port: 3080, status: 'running', startedAtMs: 500,
  sampledAtMs: 10000, cpuPercent: 1.2, memoryBytes: 1024, reason: null,
}

describe('process readings', () => {
  it('uses the OS start time and keeps an initial CPU sample unknown', () => {
    const result = processReading({ status: 'running', pid: 123, startedAt: 9000 }, { ...sample, cpuPercent: null }, null, 11000)
    expect(result.label).toBe('运行中')
    expect(result.sample?.startedAtMs).toBe(500)
    expect(result.sample?.cpuPercent).toBeNull()
  })

  it('never shows a previous PID or stale measurements as current', () => {
    expect(processReading({ status: 'running', pid: 124 }, sample, null, 11000).sample).toBeUndefined()
    expect(processReading({ status: 'running', pid: 123 }, sample, null, 17000).sample).toBeUndefined()
    expect(processReading({ status: 'running', pid: 123 }, sample, 'access denied', 11000).label).toBe('采样失败')
  })

  it('shows process exit, unknown identity, boot failures and abnormal exits explicitly', () => {
    const running = { status: 'running' as const, pid: 123 }
    expect(processReading(running, { ...sample, status: 'exited' }, null, 11000).label).toBe('进程已退出')
    expect(processReading(running, { ...sample, status: 'unknown', reason: 'PID reused' }, null, 11000).detail).toBe('PID reused')
    expect(processReading({ status: 'error', error: { title: '启动失败', detail: '端口冲突' } }, sample, null, 11000).detail).toContain('端口冲突')
    expect(processReading({ status: 'stopped', lastExit: { at: 1, code: 7, ranFor: 2 } }, sample, null, 11000).label).toBe('异常退出')
  })

  it('prioritizes an in-flight stop over a previous running sample', () => {
    expect(processReading({ status: 'stopping', pid: 123 }, sample, null, 11000)).toMatchObject({ label: '停止中' })
    expect(processReading({ status: 'stopping', pid: 123 }, sample, null, 11000).sample).toBeUndefined()
  })
})
