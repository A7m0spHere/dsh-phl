import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { CATEGORY_LABEL, REPAIR_ACTION_LABEL } from './healthCategories'

const rust = (name: string) => readFileSync(path.resolve(process.cwd(), `src-tauri/src/${name}`), 'utf8')

/**
 * The C-14 gate: every category literal the verify report can emit must
 * have a display label. Parsed straight from the Rust source so a new
 * `VerifyCheck::` cannot ship to the UI as a raw token.
 */
describe('verify check categories', () => {
  it('every VerifyCheck category in verify.rs has a label', () => {
    const src = rust('verify.rs')
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

/**
 * The same gate for the repair actions the card narrates ("已修复：…"). Two
 * backend lists have to agree — the actions `verify.rs` can flag and the ones
 * `repair.rs` is willing to run — so both are read.
 */
describe('repair action labels', () => {
  it('every action verify can flag or repair can run has a label', () => {
    const actions = new Set<string>()
    for (const name of ['verify.rs', 'repair.rs']) {
      for (const m of rust(name).matchAll(/"([a-z]+-[a-z-]+)"/g)) {
        const token = m[1]
        // Action-shaped tokens only: check ids share the shape, and an
        // over-broad regex would make this gate report noise instead.
        if (/^(install|reinstall|recreate|cleanup|disable)-/.test(token)) actions.add(token)
      }
    }
    expect(actions.size).toBeGreaterThanOrEqual(6)
    const unlabeled = [...actions].filter((a) => !(a in REPAIR_ACTION_LABEL))
    expect(unlabeled).toEqual([])
  })
})
