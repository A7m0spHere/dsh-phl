import type { InstanceRuntimeState, ProcessSample } from '@/types'

export function processReading(
  state: InstanceRuntimeState,
  sample: ProcessSample | undefined,
  samplingError: string | null,
  now: number,
): { label: string; tone: 'ok' | 'warn' | 'danger' | 'neutral'; detail: string; sample?: ProcessSample } {
  if (state.status === 'starting') return { label: '启动中', tone: 'warn', detail: '' }
  if (state.status === 'stopping') return { label: '停止中', tone: 'warn', detail: '' }
  if (state.status === 'error') return { label: '启动失败', tone: 'danger', detail: [state.error?.detail, state.error?.hint].filter(Boolean).join('\n') }
  if (state.status === 'stopped') return state.lastExit
    ? { label: '异常退出', tone: 'danger', detail: `退出码 ${state.lastExit.code ?? '未知'}，运行 ${state.lastExit.ranFor} 秒。请查看日志。` }
    : { label: '已停止', tone: 'neutral', detail: '' }
  if (samplingError) return { label: '采样失败', tone: 'warn', detail: samplingError }
  if (!sample || sample.pid !== state.pid || now - sample.sampledAtMs > 6000) {
    return { label: '等待采样', tone: 'warn', detail: '等待当前进程的真实资源数据。' }
  }
  if (sample.status !== 'running') return {
    label: sample.status === 'exited' ? '进程已退出' : '身份待确认',
    tone: 'warn', detail: sample.reason ?? '进程资源数据不可用。',
  }
  return {
    label: state.error?.title ?? (state.lastWebuiError ? 'WebUI 异常' : '运行中'),
    tone: state.error || state.lastWebuiError ? 'warn' : 'ok',
    detail: state.error?.detail ?? state.lastWebuiError?.headline ?? sample.reason ?? '', sample,
  }
}
