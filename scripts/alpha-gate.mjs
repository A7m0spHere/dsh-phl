#!/usr/bin/env node
/**
 * PHL alpha gate — one deterministic, CI-capable command:
 *
 *   npm run test:alpha
 *
 * Runs, in order: version check · typecheck · bridge check · frontend tests ·
 * the desktop runner tests · the whole Rust workspace (fmt + clippy + all
 * crate/lib tests). Every step runs exactly once; failures do not stop the
 * sweep — the summary lists all of them, so a single pass over the gate gives
 * the complete picture.
 *
 * The GUI/desktop layer is deliberately NOT here (it needs a display and a
 * Tauri window): use `npm run test:desktop`, which launches PHL against an
 * isolated PHL_ROOT scratch data root.
 */
import { spawnSync } from 'node:child_process'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

const steps = [
  { name: 'version check', cmd: 'node', args: ['scripts/check-versions.mjs'] },
  { name: 'typecheck', cmd: 'npm', args: ['run', 'typecheck'] },
  { name: 'bridge check', cmd: 'npm', args: ['run', 'bridge:check'] },
  { name: 'frontend tests', cmd: 'npm', args: ['test'] },
  { name: 'desktop runner tests', cmd: 'node', args: ['--test', 'scripts/test-alpha-desktop.mjs'] },
  { name: 'rust fmt', cmd: 'cargo', args: ['fmt', '--check', '--all'], dir: 'src-tauri' },
  { name: 'rust clippy', cmd: 'cargo', args: ['clippy', '--workspace', '--all-targets', '--', '-D', 'warnings'], dir: 'src-tauri' },
  { name: 'rust tests (workspace)', cmd: 'cargo', args: ['test', '--workspace'], dir: 'src-tauri' },
]

const results = []
for (const step of steps) {
  const started = Date.now()
  process.stdout.write(`\n=== ${step.name} ===\n`)
  const r = spawnSync(step.cmd, step.args, {
    cwd: step.dir ? path.join(repoRoot, step.dir) : repoRoot,
    stdio: 'inherit',
    shell: process.platform === 'win32',
  })
  results.push({ name: step.name, ok: r.status === 0, seconds: ((Date.now() - started) / 1000).toFixed(1) })
}

process.stdout.write('\n================ PHL alpha gate ================\n')
for (const r of results) {
  process.stdout.write(`${r.ok ? 'PASS' : 'FAIL'}  ${r.name.padEnd(24)} ${r.seconds}s\n`)
}
const failed = results.filter((r) => !r.ok)
process.stdout.write(
  failed.length === 0
    ? 'ALPHA GATE: all steps green — safe to proceed to the desktop pass.\n'
    : `ALPHA GATE: ${failed.length} step(s) failed: ${failed.map((f) => f.name).join(', ')}\n`,
)
process.exit(failed.length === 0 ? 0 : 1)
