import { describe, expect, it } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import path from 'node:path'
import { DIRECTORY_LABEL, KIND_LABEL, PHASE_LABEL, phaseText } from './taskLabels'

/**
 * The P3-8 gate: every `guarded(<id>, "<kind>", ...)` in the Rust sources
 * must have a user-facing label. A kind with no label surfaces its raw
 * snake-case token on the done/cancelled task rows — the whole point of
 * this file is that the user never reads an internal name.
 */
function rustSources(): string[] {
  const root = path.resolve(process.cwd(), 'src-tauri', 'src')
  const out: string[] = []
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const full = path.join(dir, name)
      if (statSync(full).isDirectory()) walk(full)
      else if (name.endsWith('.rs')) out.push(full)
    }
  }
  walk(root)
  return out
}

/** The string literal in `guarded`'s second argument (kind), or 'dynamic'. */
function guardedKinds(): string[] {
  const kinds: string[] = []
  for (const file of rustSources()) {
    const src = readFileSync(file, 'utf8')
    let idx = 0
    while ((idx = src.indexOf('guarded(', idx)) >= 0) {
      // Skip the macro definition itself.
      const isDef = /pub\s+(?:async\s+)?fn\s+guarded\s*\(/.test(src.slice(Math.max(0, idx - 40), idx))
      idx += 'guarded('.length
      if (isDef) continue
      // Read the two top-level arguments, ignoring commas nested in
      // parens / string literals.
      const args: string[] = []
      let depth = 1
      let current = ''
      let inString = false
      let escaped = false
      let i = idx
      for (; i < src.length && args.length < 2; i++) {
        const c = src[i]
        if (inString) {
          current += c
          if (escaped) escaped = false
          else if (c === '\\') escaped = true
          else if (c === '"') inString = false
          continue
        }
        if (c === '"') inString = true
        else if (c === '(') depth++
        else if (c === ')') {
          depth--
          // Closed the call (or a `guarded()` mention in a comment) before
          // two arguments were seen: not a call site we can type-check.
          if (depth === 0) break
        } else if (c === ',' && depth === 1) {
          args.push(current.trim())
          current = ''
          continue
        }
        current += c
      }
      if (args.length === 2) {
        const literal = /^"([a-z0-9-]+)"$/.exec(args[1])
        kinds.push(literal ? literal[1] : 'dynamic')
      }
    }
  }
  return kinds.filter((k) => k !== 'dynamic')
}

describe('task centre labels (P3-8/P3-9)', () => {
  it('every Rust guarded kind has a Chinese label', () => {
    const kinds = [...new Set(guardedKinds())]
    expect(kinds.length).toBeGreaterThanOrEqual(20)
    const unlabeled = kinds.filter((kind) => !(kind in KIND_LABEL))
    expect(unlabeled).toEqual([])
  })

  it('migration phases compose instead of enumerating', () => {
    // Forward: `set_phase(kind)` with the bare directory token.
    expect(phaseText({ phase: 'instances', cancelRequested: false, state: 'running' })).toBe('搬运实例目录')
    // Undo: `set_phase("撤销 {kind}")` — the dynamic compound the old flat
    // map could not cover.
    expect(phaseText({ phase: '撤销 versions', cancelRequested: false, state: 'running' })).toBe('撤销版本目录')
    // A cancel request outranks the phase word.
    expect(phaseText({ phase: 'copying', cancelRequested: true, state: 'running' })).toBe('正在取消…')
    // Phases the backend already emits in Chinese pass through.
    expect(phaseText({ phase: '解包整合包', cancelRequested: false, state: 'running' })).toBe('解包整合包')
  })

  it('the fixed phase table and the directory table describe disjoint tokens', () => {
    // A directory token in PHASE_LABEL would silently shadow the composed
    // 搬运 wording — keep the tables orthogonal.
    for (const dir of Object.keys(DIRECTORY_LABEL)) {
      expect(PHASE_LABEL).not.toHaveProperty(dir)
    }
  })
})
