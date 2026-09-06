import { describe, expect, it } from 'vitest'
import { parsePhlError, parseThrownError } from './errorCodes'

describe('parsePhlError', () => {
  it('splits the stable code from the human message', () => {
    const r = parsePhlError('[busy] 实例 main 正被另一个操作占用，请等待其完成后重试')
    expect(r.code).toBe('busy')
    expect(r.message).toBe('实例 main 正被另一个操作占用，请等待其完成后重试')
    expect(r.retryable).toBe(true)
    expect(r.hint).toContain('任务中心')
  })

  it('marks only transient kinds retryable', () => {
    expect(parsePhlError('[net-retryable] 下载失败').retryable).toBe(true)
    expect(parsePhlError('[port-conflict] 端口 3080 已被占用').retryable).toBe(true)
    expect(parsePhlError('[net-fatal] 下载源返回错误: 404').retryable).toBe(false)
    expect(parsePhlError('[permission] 无法删除').retryable).toBe(false)
  })

  it('passes uncoded and unknown-prefix messages through untouched', () => {
    expect(parsePhlError('实例不存在')).toEqual({
      code: null,
      message: '实例不存在',
      retryable: false,
      hint: null,
    })
    const weird = parsePhlError('[future-code] 未知')
    expect(weird.code).toBe(null)
    expect(weird.message).toBe('[future-code] 未知')
  })

  it('normalises string and Error rejections alike', () => {
    expect(parseThrownError('a string').message).toBe('a string')
    expect(parseThrownError(new Error('[disk-full] 空间不足')).code).toBe('disk-full')
    expect(parseThrownError(42).message).toBe('42')
  })
})
