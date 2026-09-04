import { expect, it } from 'vitest'
import { normalizeRoot } from './paths'

it.each([
  [' C:\\ ', 'C:/'], ['D:////', 'D:/'], ['/', '/'],
  ['C:\\PHL\\', 'C:\\PHL'], ['/home/phl/', '/home/phl'],
  ['\\\\server\\share\\', '\\\\server\\share'], ['', ''],
])('normalizes %s without losing its absolute root', (input, expected) => {
  expect(normalizeRoot(input)).toBe(expected)
})
