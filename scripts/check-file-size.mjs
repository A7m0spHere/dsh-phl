#!/usr/bin/env node
/**
 * Ratchet size budget — keeps the big files from silently growing back.
 * Companion to docs/structure-review-2026-09.md; the qualitative rule there:
 * size is not a quality metric, but UNBOUNDED growth is how last year's
 * review found `instances/mod.rs` at 108 KB again.
 *
 *   node scripts/check-file-size.mjs            # verify
 *   node scripts/check-file-size.mjs --update   # re-ratchet to current sizes
 *
 * `verify` fails when:
 *   A. a registered file exceeds its budget, or
 *   B. a source file outside the table reaches UNBUDGETED_MAX — registration
 *      must be a deliberate line in the JSON (an explanation belongs with it),
 *      or
 *   C. a registered path no longer exists (a split removed it: lower the
 *      table with --update so the freed room cannot be quietly re-filled).
 *
 * It prints SHRINK candidates when a file sits >10% under budget — the
 * ratchet half: run --update to ratchet those ceilings down. --update only
 * ever touches budgets for files that already exist; it never registers new
 * paths (rule B's job) and never lowers a manual budget below what you set
 * for anything already inside 10% (so an intentional tight budget survives).
 */
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const budgetPath = path.join(repoRoot, 'scripts', 'file-size-budget.json')
const UPDATE = process.argv.includes('--update')
const UNBUDGETED_MAX = 40 * 1024
const SHRINK_HINT = 0.9 // budget * 0.9 > size ⇒ room is going unused
const SOURCE_EXTS = new Set(['.rs', '.ts', '.tsx'])
const WALK_ROOTS = ['src-tauri/src', 'src']
const SKIP_DIRS = new Set(['target', 'node_modules', 'dist', 'gen'])

function walk(dir, out) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!SKIP_DIRS.has(entry.name)) walk(path.join(dir, entry.name), out)
    } else if (SOURCE_EXTS.has(path.extname(entry.name))) {
      out.push(path.join(dir, entry.name))
    }
  }
}

const budgetDoc = JSON.parse(fs.readFileSync(budgetPath, 'utf8'))
const budgets = budgetDoc.budgets

if (UPDATE) {
  let changed = 0
  // 1. ratchet existing budgets toward current sizes (only if the file is
  //    >10% under its ceiling — small drift is allowed to stay buffered).
  for (const [rel, cap] of Object.entries(budgets)) {
    const abs = path.join(repoRoot, rel)
    if (!fs.existsSync(abs)) continue
    const size = fs.statSync(abs).size
    if (size < cap * SHRINK_HINT) {
      budgets[rel] = Math.ceil(size * 1.05)
      changed += 1
      console.log(`ratchet  ${rel}: ${cap} → ${budgets[rel]} (now ${size})`)
    }
  }
  // 2. drop deleted paths (a split removed them for good).
  for (const rel of Object.keys(budgets)) {
    if (!fs.existsSync(path.join(repoRoot, rel))) {
      delete budgets[rel]
      changed += 1
      console.log(`retire ${rel} (no longer present)`)
    }
  }
  if (!changed) console.log('budget already tight — nothing to ratchet')
  else {
    budgetDoc.budgets = Object.fromEntries(
      Object.entries(budgets).sort((a, b) => b[1] - a[1]),
    )
    fs.writeFileSync(budgetPath, JSON.stringify(budgetDoc, null, 2) + '\n')
    console.log(`wrote ${budgetPath}`)
  }
  process.exit(0)
}

const violations = []
const missing = []
const shrinks = []
const now = new Map() // registered path → size

for (const rel of Object.keys(budgets)) {
  const abs = path.join(repoRoot, rel)
  if (!fs.existsSync(abs)) {
    missing.push(rel)
    continue
  }
  const size = fs.statSync(abs).size
  now.set(rel, size)
  const cap = budgets[rel]
  if (size > cap) violations.push({ rel, size, cap })
  else if (size < cap * SHRINK_HINT) shrinks.push({ rel, size, cap })
}

const unregistered = []
for (const root of WALK_ROOTS) {
  const abs = path.join(repoRoot, root)
  if (!fs.existsSync(abs)) continue
  const files = []
  walk(abs, files)
  for (const f of files) {
    const rel = path.relative(repoRoot, f).split(path.sep).join('/')
    if (budgets[rel] !== undefined) continue
    const size = fs.statSync(f).size
    if (size >= UNBUDGETED_MAX) unregistered.push({ rel, size })
  }
}

console.log('================ PHL size budget ================')
if (violations.length === 0 && missing.length === 0 && unregistered.length === 0) {
  const total = Object.keys(budgets).length
  console.log(`PASS  ${total} registered files within budget (no file outside the table ≥ ${UNBUDGETED_MAX / 1024} KB)`)
} else {
  for (const v of violations) {
    console.log(
      `FAIL  ${v.rel}: ${v.size} bytes > budget ${v.cap} (+${((v.size / v.cap - 1) * 100).toFixed(1)}%). Split per docs/structure-review-2026-09.md, or raise the budget consciously in review.`,
    )
  }
  for (const m of missing) {
    console.log(`FAIL  ${m}: registered but deleted — run \`node scripts/check-file-size.mjs --update\` to retire it`)
  }
  for (const u of unregistered) {
    console.log(`FAIL  ${u.rel}: ${u.size} bytes, ≥ ${UNBUDGETED_MAX / 1024} KB, not registered — add a justified entry to scripts/file-size-budget.json`)
  }
}
for (const s of shrinks) {
  console.log(`note  ${s.rel}: ${s.size} bytes vs budget ${s.cap} — >10% headroom; consider --update to ratchet down`)
}
process.exit(violations.length + missing.length + unregistered.length === 0 ? 0 : 1)
