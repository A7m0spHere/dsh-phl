#!/usr/bin/env node
// Issue #7 — repeatable entry point for the real-DSH compatibility lane.
//
// The lane downloads REAL packages from registry.npmjs.org, runs real
// `npm install`s and starts real DSH servers on this machine. It is opt-in
// by design and never part of a CI gate: flaky upstream networks must not
// block every push (issue #7's own caveat). Without this wrapper the
// equivalent command is:
//
//   PHL_REAL_DSH=1 cargo test --lib real_dsh -- --ignored --nocapture --test-threads=1
//
// Usage (from the repo root):
//   node scripts/real-dsh-acceptance.mjs                 # 0.1.x baseline + 0.2.x matrix
//   node scripts/real-dsh-acceptance.mjs --with-runtime  # also the real Node runtime lane
//   node scripts/real-dsh-acceptance.mjs --list          # show what would run, no execution
//
// On Windows, npm scripts run through cmd.exe which cannot set env vars
// inline — that is why this wrapper exists rather than a bare package.json
// script with `PHL_REAL_DSH=1`.

import { spawnSync } from 'node:child_process'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const args = new Set(process.argv.slice(2))

const plan = args.has('--with-runtime')
  ? [
      'acceptance::real_dsh::real_dsh_upstream_install_launch_and_coexist (0.1.x baseline: install → launch → handshake → coexist → stop)',
      'acceptance::real_dsh::real_dsh_02x_compatibility_matrix (issue #7: one npm-confirmed 0.2.x through the 9-leg matrix)',
      'acceptance::real_dsh::real_dsh_webui_auth_probe (diagnostic companion, harmless)',
      'acceptance::real_runtime::real_runtimes_host_real_dsh_side_by_side (real Node runtimes from nodejs.org)',
    ]
  : [
      'acceptance::real_dsh::real_dsh_upstream_install_launch_and_coexist (0.1.x baseline: install → launch → handshake → coexist → stop)',
      'acceptance::real_dsh::real_dsh_02x_compatibility_matrix (issue #7: one npm-confirmed 0.2.x through the 9-leg matrix)',
      'acceptance::real_dsh::real_dsh_webui_auth_probe (diagnostic companion, harmless)',
    ]

if (args.has('--list') || args.has('-h') || args.has('--help')) {
  console.log('real-DSH acceptance lane — will run:')
  for (const item of plan) console.log(`  - ${item}`)
  console.log('Network required; nothing runs without PHL_REAL_DSH=1 (set by this wrapper).')
  process.exit(0)
}

console.log('[real-dsh] opt-in lane: downloading real packages from registry.npmjs.org and')
console.log('[real-dsh] starting real DSH servers on this machine. Not a CI gate (issue #7).')
for (const item of plan) console.log(`[real-dsh] will run: ${item}`)

// The test-name filter `real_` covers both `real_dsh_*` and `real_runtimes_*`.
const cargoArgs = ['test', '--lib', '--locked', args.has('--with-runtime') ? 'real_' : 'real_dsh', '--', '--ignored', '--nocapture', '--test-threads=1']

const result = spawnSync('cargo', cargoArgs, {
  cwd: path.join(root, 'src-tauri'),
  stdio: 'inherit',
  shell: process.platform === 'win32',
  env: { ...process.env, PHL_REAL_DSH: '1' },
})

if (result.error) {
  console.error(`[real-dsh] failed to launch cargo: ${result.error.message}`)
  process.exit(1)
}
console.log(
  result.status === 0
    ? '[real-dsh] lane PASSED — copy the [matrix leg …] lines into internal/acceptance/2026-10-31-issue-7-dsh-matrix/MATRIX.md'
    : `[real-dsh] lane FAILED (exit ${result.status}) — each failure names its classification: 网络/镜像故障 · 上游包不可用 · PHL 兼容性失败 · 环境不满足`,
)
process.exit(result.status ?? 1)
