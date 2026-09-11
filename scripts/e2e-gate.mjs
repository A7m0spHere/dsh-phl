#!/usr/bin/env node
/**
 * PHL Windows E2E gate — one command for both lanes:
 *
 *   npm run test:e2e:core                       # headless journeys + faults
 *   npm run test:e2e:smoke -- --installer=X [--expect-version=Y]
 *   node scripts/e2e-gate.mjs all --installer=X --expect-version=Y
 *
 * `core` runs the `#[ignore]`-marked release journeys in src-tauri
 * (`release_e2e`): cold install, restart adoption, snapshot/pack, migration,
 * the updater manifest contract, and the system-level fault injections. It
 * needs a real `node` on PATH (the fake runtime is the host's own node.exe)
 * and keeps all its downloads on a local mock server.
 *
 * `smoke` runs the OS-level installer lane (e2e/install-smoke.mjs): silent
 * install → first launch over WebView2 CDP → upgrade → uninstall → user-data
 * promise. Pass `--exe=<path>` instead of `--installer` for a run-only check
 * of an unpacked build.
 *
 * Windows only: both lanes assert real OS state (taskkill, NTFS locks, the
 * registry, the DevTools port).
 */
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const mode = (process.argv[2] ?? 'core').toLowerCase()
const extra = process.argv.slice(3)

if (process.platform !== 'win32') {
  console.error('e2e gate: Windows only (real taskkill / NTFS / registry / WebView2).')
  process.exit(2)
}

const steps = []
if (mode === 'core' || mode === 'all') {
  steps.push({
    name: 'e2e core journeys (release_e2e)',
    // shell:true on Windows: cargo lives behind cargo.exe and Node's
    // shell-less spawn does not resolve it via PATH the way bash does.
    cmd: 'cargo',
    args: ['test', '--workspace', '--', '--ignored', 'release_e2e', '--test-threads=1'],
    dir: 'src-tauri',
    shell: true,
  })
}
if (mode === 'smoke' || mode === 'all') {
  const installer = extra.find((a) => a.startsWith('--installer='))
  const exe = extra.find((a) => a.startsWith('--exe='))
  if (!installer && !exe) {
    console.error('e2e smoke needs --installer=<setup.exe> or --exe=<phl.exe>')
    process.exit(2)
  }
  for (const arg of [...(installer ? [installer] : []), ...(exe ? [exe] : []), ...extra.filter((a) => !a.startsWith('--installer=') && !a.startsWith('--exe='))]) {
    const value = arg.split('=')[1]
    if (value && !path.isAbsolute(value) && !fs.existsSync(path.join(repoRoot, value))) {
      console.error(`e2e smoke: path does not exist: ${value}`)
      process.exit(2)
    }
  }
  steps.push({
    name: 'e2e installer smoke',
    cmd: process.execPath,
    args: ['e2e/install-smoke.mjs', ...extra],
    dir: '.',
  })
}

const results = []
for (const step of steps) {
  const started = Date.now()
  process.stdout.write(`\n=== ${step.name} ===\n`)
  const r = spawnSync(step.cmd, step.args, {
    cwd: path.join(repoRoot, step.dir),
    stdio: 'inherit',
    shell: step.shell ?? false,
    windowsHide: true,
  })
  results.push({ name: step.name, ok: r.status === 0, seconds: ((Date.now() - started) / 1000).toFixed(1) })
}

process.stdout.write('\n================ PHL e2e gate ================\n')
for (const r of results) {
  process.stdout.write(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name.padEnd(32)} ${r.seconds}s\n`)
}
const failed = results.filter((r) => !r.ok)
process.stdout.write(
  failed.length === 0
    ? 'E2E GATE: all lanes green — release candidates may proceed.\n'
    : `E2E GATE: ${failed.length} lane(s) failed: ${failed.map((f) => f.name).join(', ')}\n`,
)
process.exit(failed.length === 0 ? 0 : 1)
