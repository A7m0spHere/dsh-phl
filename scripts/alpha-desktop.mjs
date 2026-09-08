#!/usr/bin/env node
/**
 * PHL desktop alpha pass — GUI-layer acceptance against an isolated root:
 *
 *   npm run test:desktop [scenario-file.json]
 *
 * Sets PHL_ROOT to a fresh scratch directory under %TEMP% before launching
 * `tauri dev`, so the acceptance session (root.json pointer, processes.json,
 * migration journals, every instance/version/plugin byte) can never touch the
 * developer's real PHL data or DSH_HOME. On exit the scratch root is removed.
 *
 * The scenario JSON is a machine-readable checklist of the GUI scenarios
 * (create / clone+snapshot / pack / exploratory). Drive it with the
 * `scripts/alpha-desktop-record.mjs` reporter (or a Computer-Use agent): each
 * step you mark pass/fail is appended to a report file next to the scratch
 * root, so the pass leaves evidence instead of a vibe.
 *
 * Optional scenario notes:
 *   PHL_ALPHA_KEEP=1  keep the scratch root after exit (for post-mortems)
 *   PHL_ALPHA_ROOT=C:\\path  use an explicit root instead of a new temp dir
 */
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

const scratch =
  process.env.PHL_ALPHA_ROOT || path.join(os.tmpdir(), `phl-alpha-desktop-${new Date().toISOString().replace(/[:.]/g, '-')}`)
fs.mkdirSync(scratch, { recursive: true })

const scenario = process.argv[2]
  ? path.resolve(process.argv[2])
  : path.join(repoRoot, 'scripts', 'alpha-desktop-scenarios.json')
const scenarios = JSON.parse(fs.readFileSync(scenario, 'utf8'))

const reportPath = path.join(scratch, 'alpha-desktop-report.json')
fs.writeFileSync(
  reportPath,
  JSON.stringify({ startedAt: new Date().toISOString(), root: scratch, scenarios }, null, 2),
)

process.stdout.write(
  `\nPHL desktop alpha\n  data root : ${scratch}  (PHL_ROOT — real user data is untouched)\n` +
    `  scenarios : ${scenario}\n  report    : ${reportPath}\n\n` +
    `Scenarios:\n${scenarios.map((s) => `  - ${s.id}: ${s.title}`).join('\n')}\n\n` +
    `Launching tauri dev… mark results with scripts/alpha-desktop-record.mjs\n\n`,
)

const child = spawn('npm', ['run', 'app:dev'], {
  cwd: repoRoot,
  stdio: 'inherit',
  shell: process.platform === 'win32',
  env: { ...process.env, PHL_ROOT: scratch },
})

let cleaning = false
const cleanup = () => {
  if (cleaning) return
  cleaning = true
  if (process.env.PHL_ALPHA_KEEP) {
    process.stdout.write(`\nKeeping scratch root per PHL_ALPHA_KEEP: ${scratch}\n`)
    return
  }
  try {
    fs.rmSync(scratch, { recursive: true, force: true })
    process.stdout.write(`\nRemoved scratch root ${scratch}\n`)
  } catch (e) {
    process.stdout.write(`\nScratch root left behind (${e.message}); delete ${scratch}\n`)
  }
}
process.on('exit', cleanup)
process.on('SIGINT', () => { child.kill(); })
process.on('SIGTERM', () => { child.kill(); })
child.on('exit', (code) => {
  cleanup()
  process.exit(code ?? 0)
})
