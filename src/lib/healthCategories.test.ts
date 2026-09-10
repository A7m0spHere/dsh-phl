import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { CATEGORY_LABEL } from './healthCategories'

/**
 * The C-14 gate: every category literal the verify report can emit must
 * have a display label. Parsed straight from the Rust source so a new
 * `VerifyCheck::` cannot ship to the UI as a raw token.
 */
describe('verify check categories', () => {
  it('every VerifyCheck category in verify.rs has a label', () => {
    const src = readFileSync(path.resolve(process.cwd(), 'src-tauri/src/verify.rs'), 'utf8')
    // Construction sites are `(id, category, message)` — sometimes on one
    // line, sometimes wrapped; strip newlines first so both shapes match.
    const flat = src.replace(/\s+/g, ' ')
    const categories = new Set<string>()
    for (const m of flat.matchAll(/VerifyCheck::(?:pass|fail|warn)\(\s*"([a-z0-9-]+)",\s*"([a-z0-9-]+)"/g)) {
      categories.add(m[2])
    }
    expect(categories.size).toBeGreaterThanOrEqual(5)
    const unlabeled = [...categories].filter((c) => !(c in CATEGORY_LABEL))
    expect(unlabeled).toEqual([])
  })
})
