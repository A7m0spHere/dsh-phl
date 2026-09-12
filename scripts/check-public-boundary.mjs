#!/usr/bin/env node
/**
 * Publication boundary check — the published tree carries code, build config
 * and tests only. Planning, review and acceptance notes live in the
 * maintainer's local workspace (D:\AI项目\dsh-phl) and must never be tracked
 * in the publishing copy (D:\AI项目\dsh-phl-github), in CI, or in a clone.
 *
 *   node scripts/check-public-boundary.mjs            # auto
 *   node scripts/check-public-boundary.mjs --enforce  # fail on any hit
 *   node scripts/check-public-boundary.mjs --list     # print the patterns
 *
 * Auto mode tells the two copies apart by their push URL: the local workspace
 * carries a disabled-…invalid placeholder push URL and is SUPPOSED to track
 * these files, so it only reports them; anywhere else (publishing copy, CI, a
 * fresh clone) a tracked boundary file fails the run.
 *
 * Why this exists: the 2026-09-12 reconciliation fast-forwarded the publishing
 * copy to the workspace line and silently published 20 internal documents; the
 * .gitignore block that should have stopped it was overwritten by that same
 * fast-forward. This script is the guard that survives a copy.
 */
import { execFileSync } from 'node:child_process'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

/**
 * Every internal document lives under this one directory (2026-09-12 tidy-up),
 * so the boundary is a single rule instead of a name list that drifts. The
 * publishing copy's .gitignore carries the same line.
 */
const BOUNDARY = ['internal/**']

const args = process.argv.slice(2)

if (args.includes('--list')) {
  console.log('publication boundary patterns:')
  for (const pattern of BOUNDARY) console.log('  ' + pattern)
  process.exit(0)
}

/**
 * Two glob forms only:
 *   `dir/**`      — everything under `dir`, at ANY depth (crosses separators);
 *   `dir/pre-*.md` — a single `*`, which never crosses a path separator.
 * The 2026-09-12 tidy-up made the whole boundary one `internal/**` rule; the
 * single-`*` matcher below used to swallow it silently, so the self-test at
 * the bottom pins both forms.
 */
function matches(pattern, file) {
  if (pattern.endsWith('/**')) return file.startsWith(pattern.slice(0, -2))
  const star = pattern.indexOf('*')
  if (star === -1) return file === pattern
  const head = pattern.slice(0, star)
  const tail = pattern.slice(star + 1)
  if (!file.startsWith(head) || !file.endsWith(tail)) return false
  return !file.slice(head.length, file.length - tail.length).includes('/')
}

/** Guards the matcher itself: `**` crosses separators, `*` does not. */
function selfTest() {
  const cases = [
    ['internal/**', 'internal/README.md', true],
    ['internal/**', 'internal/reviews/deep/nested.md', true],
    ['internal/**', 'internal', false],
    ['internal/**', 'src/internal/README.md', false],
    ['docs/pre-*.md', 'docs/pre-1.md', true],
    ['docs/pre-*.md', 'docs/pre/1.md', false],
  ]
  const bad = cases.filter(([pattern, file, want]) => matches(pattern, file) !== want)
  if (bad.length === 0) return
  console.error('BOUNDARY MATCHER BROKEN:')
  for (const [pattern, file, want] of bad) {
    console.error(`  ${pattern} vs ${file}: want ${want}, got ${!want}`)
  }
  process.exit(1)
}

selfTest()

function git(gitArgs) {
  try {
    return execFileSync('git', gitArgs, {
      cwd: repoRoot,
      windowsHide: true,
      stdio: ['ignore', 'pipe', 'ignore'],
    }).toString()
  } catch {
    return null
  }
}

const tracked = git(['ls-files', '-z'])
if (tracked === null) {
  console.error('PUBLICATION BOUNDARY: not a git checkout - cannot check')
  process.exit(1)
}
const pushUrl = (git(['remote', 'get-url', '--push', 'origin']) || '').trim()
const isLocalWorkspace = /invalid|disabled/i.test(pushUrl)
const files = tracked.split('\0').filter(Boolean)
const hits = files.filter((file) => BOUNDARY.some((pattern) => matches(pattern, file)))

if (hits.length === 0) {
  console.log('PASS  no internal document tracked (' + BOUNDARY.length + ' patterns checked)')
  process.exit(0)
}

if (isLocalWorkspace && !args.includes('--enforce')) {
  console.log('note  local workspace (push URL disabled): ' + hits.length + ' internal document(s) tracked here by design')
  for (const file of hits) console.log('        ' + file)
  console.log('      the publishing copy and CI must not carry them: ' + pushUrl)
  process.exit(0)
}

console.error('PUBLICATION BOUNDARY FAILED - ' + hits.length + ' internal document(s) tracked in a publishing tree:')
for (const file of hits) console.error('  ' + file)
console.error('Drop them (git rm --cached) and keep the boundary rule in .gitignore;')
console.error('the procedure lives in the maintainer-local ops notes (internal/ops).')
process.exit(1)
