import { describe, expect, it } from 'vitest'
import { interpretBootFailure } from './webuiBootFailure'

/** The exact card the shell probe forwarded on 2026-09-27. */
const BONK_PET_CARD = [
  'HARNESS',
  'Failed to load plugins',
  'dsh-bonk-pet',
  'web boot: 1 entry did not activate',
  'dsh-bonk-pet: pending (waiting for service: settingsScope)',
].join('\n')

describe('interpretBootFailure', () => {
  it('names the plugin, the missing service and the incompatibility', () => {
    const r = interpretBootFailure(BONK_PET_CARD)
    expect(r.headline).toBe('WebUI 启动失败：1 个插件未能加载')
    expect(r.causes).toHaveLength(1)
    expect(r.causes[0]).toContain('dsh-bonk-pet')
    expect(r.causes[0]).toContain('settingsScope')
    expect(r.causes[0]).toContain('不兼容')
    expect(r.hint).toContain('停用')
  })

  it('reads the package id when the card spells id (name)', () => {
    const r = interpretBootFailure(
      'web boot: 2 entries did not activate\nbetter-x (@scope/better-x): pending (waiting for service: settingsScope)',
    )
    expect(r.causes[0]).toContain('better-x')
    expect(r.headline).toContain('1 个插件')
  })

  it('lists several missing services from one pending entry', () => {
    const r = interpretBootFailure('x (pkg): pending (waiting for services: alpha, beta)')
    expect(r.causes[0]).toContain('alpha, beta')
  })

  it('keeps the raw error text of a throwing plugin', () => {
    const r = interpretBootFailure('y: failed to import')
    expect(r.causes[0]).toBe('插件「y」加载时抛错：failed to import')
  })

  it('degrades to the count line when no entry line matches', () => {
    const r = interpretBootFailure('web boot: 3 entries did not activate')
    expect(r.causes).toEqual([])
    expect(r.headline).toBe('WebUI 启动失败：3 个插件条目未能加载')
    expect(r.hint).toContain('原文没有被识别')
  })

  it('survives text that matches nothing at all', () => {
    const r = interpretBootFailure('totally new card format from a future DSH')
    expect(r.headline).toBe('WebUI 页面报告启动失败')
    expect(r.causes).toEqual([])
    expect(r.hint).toBeTruthy()
  })

  it('tolerates an empty report', () => {
    expect(() => interpretBootFailure('')).not.toThrow()
    expect(interpretBootFailure('').causes).toEqual([])
  })
})
