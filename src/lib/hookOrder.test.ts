/**
 * No hook may be called after a component's first top-level `return`.
 *
 * The project has no ESLint, so `react-hooks/rules-of-hooks` never runs. That
 * let a `useMemo` sit below an early return in `PluginsPage`: with no instance
 * the first render bailed out before it, switching to 插件库 then called one
 * hook more than the previous render, and React threw "Rendered more hooks than
 * during the previous render" — a blank window with no error boundary to catch
 * it. This test encodes the invariant across the tree until a real linter is
 * wired into the gate.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const SRC = join(process.cwd(), 'src')

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) return files(path)
    return path.endsWith('.tsx') && !path.endsWith('.test.tsx') ? [path] : []
  })
}

/** Blanks out comments and string literals so a `useX(` inside prose or text
 * cannot be mistaken for a call. */
function codeOnly(source: string): string {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
    .replace(/\/\/[^\n]*/g, '')
    .replace(/'(?:[^'\\\n]|\\.)*'/g, "''")
    .replace(/"(?:[^"\\\n]|\\.)*"/g, '""')
    .replace(/`(?:[^`\\]|\\.)*`/g, '``')
}

interface Offence {
  file: string
  component: string
  hook: string
}

function offences(source: string, file: string): Offence[] {
  const code = codeOnly(source)
  const lines = code.split('\n')
  const found: Offence[] = []
  lines.forEach((line, index) => {
    const open = line.match(/^(?:export\s+)?function\s+([A-Z]\w*)\s*\(/)
    if (!open) return
    const component = open[1]
    let depth = 0
    let sawTopLevelReturn = false
    for (let i = index; i < lines.length; i++) {
      const text = lines[i]
      const isReturn = depth === 1 && /^\s{2,}return\b/.test(text)
      const opensIf = depth === 1 && /^\s{2}if\s*\(.*\)\s*\{?\s*$/.test(text)
      if (isReturn || (opensIf && /\breturn\b/.test(lines[i + 1] ?? ''))) sawTopLevelReturn = true
      if (sawTopLevelReturn) {
        const hook = text.match(/\b(use[A-Z]\w*)\s*\(/)
        if (hook) found.push({ file, component, hook: hook[1] })
      }
      for (const ch of text) {
        if (ch === '{') depth++
        else if (ch === '}') depth--
      }
      // A column-0 closing brace ends the component. Counting braces alone
      // is not enough: JSX and object literals balance out, but one stray
      // brace in markup let the scan run into the NEXT component and blame
      // its hooks (that is how VersionRow was reported by mistake).
      if (i > index && /^\}/.test(text)) break
      if (depth <= 0 && i > index) break
    }
  })
  return found
}

describe('hook order', () => {
  it('calls every hook before a component can return early', () => {
    const found = files(SRC).flatMap((path) =>
      offences(readFileSync(path, 'utf8'), path.replace(process.cwd() + '\\', '')),
    )
    expect(
      found.map((o) => `${o.file}: ${o.component} calls ${o.hook} after a top-level return`),
    ).toEqual([])
  })
})
