import { describe, expect, it } from 'vitest'
import type { DshVersion } from '@/types'
import { resolveBoundVersion, toBoundVersionId } from './instanceVersion'

function v(id: string): DshVersion {
  return { id, name: id.replace(/^dsh-/, ''), state: { kind: 'installed' } } as DshVersion
}

describe('toBoundVersionId', () => {
  it('maps a bare detected version onto the canonical id', () => {
    expect(toBoundVersionId('0.1.2-rc.1')).toBe('dsh-0.1.2-rc.1')
  })
  it('passes canonical ids through and normalizes whitespace', () => {
    expect(toBoundVersionId('dsh-0.1.2')).toBe('dsh-0.1.2')
    expect(toBoundVersionId('  dsh-0.1.2  ')).toBe('dsh-0.1.2')
  })
  it('keeps empty as empty', () => {
    expect(toBoundVersionId('')).toBe('')
    expect(toBoundVersionId(null)).toBe('')
    expect(toBoundVersionId(undefined)).toBe('')
  })
})

describe('resolveBoundVersion', () => {
  const catalog = [v('dsh-0.1.2-rc.1'), v('dsh-0.2.0')]
  it('resolves a canonical binding exactly', () => {
    expect(resolveBoundVersion(catalog, 'dsh-0.2.0')?.id).toBe('dsh-0.2.0')
  })
  it('migrates a legacy bare-version binding on read', () => {
    expect(resolveBoundVersion(catalog, '0.1.2-rc.1')?.id).toBe('dsh-0.1.2-rc.1')
  })
  it('answers null for unbound or unknown values', () => {
    expect(resolveBoundVersion(catalog, '')).toBeNull()
    expect(resolveBoundVersion(catalog, null)).toBeNull()
    expect(resolveBoundVersion(catalog, 'dsh-9.9.9')).toBeNull()
    expect(resolveBoundVersion(catalog, '7.7.7-local')).toBeNull()
  })
})
