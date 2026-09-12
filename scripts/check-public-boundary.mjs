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
 * carries the disabled-…invalid placeholder from GITHUB_MIRROR_GUIDE.md and is
 * SUPPOSED to track these files, so it only reports them; anywhere else
 * (publishing copy, CI, a fresh clone) a tracked boundary file fails the run.
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

/** Mirrors the publication-boundary block in the publishing copy's .gitignore. */
const BOUNDARY = [
  'CLAUDE.md',
  'PHL_OPTIMIZATION_ROADMAP.md',
  'PROJECT_REVIEW*.md',
  'PROJECT_STABILIZATION_*.md',
  'PROJECT_UX_REVIEW_*.md',
  'dsh-phl-*.md',
  'docs/alpha-release-acceptance-*.md',
  'docs/alpha5-installer-smoke-manual-*.json',
  'docs/preview-release-*.md',
  'docs/structure-review-*.md',
  'docs/windows-e2e-release-gate.md',
]

const args = process.argv.slice(2)

if (args.includes('--list')) {
  console.log('publication boundary patterns:')
  for (const pattern of BOUNDARY) console.log('  ' + pattern)
  process.exit(0)
}

/** Every pattern carries exactly one `*`, which never crosses a path separator. */
function matches(pattern, file) {
  const star = pattern.indexOf('*')
  if (star === -1) return file === pattern
  const head = pattern.slice(0, star)
  const tail = pattern.slice(star + 1)
  if (!file.startsWith(head) || !file.endsWith(tail)) return false
  return !file.slice(head.length, file.length - tail.length).includes('/')
}

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
console.error('Drop them (git rm --cached) and keep the boundary block in .gitignore;')
console.error('the procedure lives in GITHUB_MIRROR_GUIDE.md in the local workspace.')
process.exit(1)
