#!/usr/bin/env node
/**
 * PHL desktop alpha pass — GUI-layer acceptance against an isolated root:
 *
 *   npm run test:desktop [scenario-file.json]
 *
 * Sets PHL_ROOT to a fresh scratch directory under %TEMP% before launching
 * `tauri dev`, so the acceptance session (root.json pointer, processes.json,
 * migration journals, every instance/version/plugin byte) can never touch the
 * developer's real PHL data or DSH_HOME. A supplied PHL_ALPHA_ROOT is used
 * verbatim and retained. Only a root allocated here is removed on normal exit.
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
import { execFile, spawn } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

const scenario = process.argv[2]
  ? path.resolve(process.argv[2])
  : path.join(repoRoot, 'scripts', 'alpha-desktop-scenarios.json')
const scenarios = JSON.parse(fs.readFileSync(scenario, 'utf8'))

// Only a fresh directory allocated by this process belongs to the runner.
// A caller-supplied root may contain valuable data and is never auto-deleted.
const suppliedRoot = process.env.PHL_ALPHA_ROOT?.trim()
const scratch = suppliedRoot
  ? path.resolve(suppliedRoot)
  : fs.mkdtempSync(path.join(os.tmpdir(), 'phl-alpha-desktop-'))
fs.mkdirSync(scratch, { recursive: true })

// Evidence must survive scratch cleanup; each run also keeps its own report.
const reportPath = path.join(path.dirname(scratch), `${path.basename(scratch)}-${Date.now()}-report.json`)
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
let interrupted = false
const cleanup = () => {
  if (cleaning) return
  cleaning = true
  if (suppliedRoot || process.env.PHL_ALPHA_KEEP || interrupted) {
    process.stdout.write(`\nKeeping supplied, requested, or interrupted root: ${scratch}\n`)
    return
  }
  try {
    fs.rmSync(scratch, { recursive: true, force: true })
    process.stdout.write(`\nRemoved scratch root ${scratch}\n`)
  } catch (e) {
    process.stdout.write(`\nScratch root left behind (${e.message}); delete ${scratch}\n`)
  }
}
const interrupt = () => {
  if (interrupted) return
  interrupted = true
  if (process.platform === 'win32' && child.pid) {
    execFile('taskkill', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true }, (error) => {
      if (error) process.stderr.write(`Unable to stop desktop process tree: ${error.message}\n`)
    })
  } else {
    child.kill('SIGTERM')
  }
}
process.on('SIGINT', interrupt)
process.on('SIGTERM', interrupt)
child.on('error', (error) => {
  process.stderr.write(`Unable to launch desktop: ${error.message}\n`)
  process.exitCode = 1
})
child.on('close', (code) => {
  cleanup()
  process.exit(process.exitCode || code || (interrupted || code === null ? 1 : 0))
})
