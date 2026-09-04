import { afterEach, expect, it, vi } from 'vitest'
import { formatRelative } from './format'

afterEach(() => vi.useRealTimers())

it.each([[60, '1 分钟前'], [3599, '59 分钟前'], [3600, '1 小时前'], [86400, '1 天前']])(
  'formats %i seconds ago with the correct unit', (seconds, expected) => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-09-05T00:00:00Z'))
    expect(formatRelative(Date.now() - seconds * 1000)).toBe(expected)
  },
)
