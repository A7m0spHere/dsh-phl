#!/usr/bin/env node
/**
 * CDP feasibility spike for the Windows installer smoke lane.
 *
 * Question this answers (W2 step 0): can a CI-run PHL be driven through the
 * WebView2 remote-debugging port? If yes, `install-smoke.mjs` builds on raw
 * CDP (Node's global fetch + WebSocket — no Playwright, no browser download,
 * no WinAppDriver). If no, the installer lane degrades to "process alive +
 * FindWindow" and the CDP steps move to the self-hosted VM lane.
 *
 * Usage:
 *   node e2e/cdp-spike.mjs [path-to-phl-exe]   # default: the debug build exe
 *
 * What it checks, in order:
 *   1. the exe starts and survives 5 s (no early crash),
 *   2. a WebView2 DevTools endpoint appears on 127.0.0.1:9333,
 *   3. a `page` target exists (the window was created and renders a document),
 *   4. `Runtime.evaluate` works and `window.__TAURI_INTERNALS__` exists —
 *      the exact bridge the smoke uses to invoke commands,
 *   5. `__TAURI_INTERNALS__.invoke("list_instances")` round-trips against
 *      the real Rust backend (PHL_ROOT points at a throwaway dir, so this
 *      never reads the developer's data).
 *
 * Exit 0 = every step passed; the smoke lane is viable on this machine.
 */
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const exe =
  process.argv[2] ??
  path.join(repoRoot, 'src-tauri', 'target', 'debug', 'dsh-phl.exe')
const PORT = 9333

if (!fs.existsSync(exe)) {
  console.error(`exe not found: ${exe}\nbuild it first: cargo build -p dsh-phl --manifest-path src-tauri/Cargo.toml`)
  process.exit(2)
}

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'phl-cdp-spike-'))
const child = spawn(exe, [], {
  windowsHide: true,
  env: {
    ...process.env,
    // Isolated data root: the spike must not read or write real user state.
    PHL_ROOT: path.join(scratch, 'root'),
    // The one setting that opens the DevTools port on the WebView2 runtime.
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
  },
})

const fail = (msg) => {
  console.error(`FAIL: ${msg}`)
  tryProcessExit()
  process.exit(1)
}
const ok = (msg) => console.log(`ok   ${msg}`)

function tryProcessExit() {
  try {
    child.kill()
  } catch {
    /* best effort */
  }
  fs.rmSync(scratch, { recursive: true, force: true })
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

// 1. survive start
child.on('exit', (code) => {
  console.error(`FAIL: exe exited early with code ${code}`)
  process.exit(1)
})
await sleep(5000)
ok('process alive after 5 s')

// 2. DevTools endpoint answers
let targets = null
for (let i = 0; i < 20; i++) {
  try {
    const res = await fetch(`http://127.0.0.1:${PORT}/json/list`)
    if (res.ok) {
      targets = await res.json()
      break
    }
  } catch {
    /* not up yet */
  }
  await sleep(500)
}
if (!targets) fail(`no DevTools endpoint on ${PORT} within 15 s`)
ok(`DevTools endpoint answers (${targets.length} targets)`)

// 3. a page target exists
const page = targets.find((t) => t.type === 'page' && t.webSocketDebuggerUrl)
if (!page) fail(`no debuggable page target: ${JSON.stringify(targets.map((t) => t.type))}`)
ok(`page target: ${page.url}`)

// 4+5. evaluate over raw CDP
const ws = new WebSocket(page.webSocketDebuggerUrl)
await new Promise((resolve, reject) => {
  ws.onopen = resolve
  ws.onerror = () => reject(new Error('websocket connect failed'))
})
let cmdId = 0
const pending = new Map()
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data)
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg)
    pending.delete(msg.id)
  }
}
const send = (method, params = {}) =>
  new Promise((resolve) => {
    const id = ++cmdId
    pending.set(id, resolve)
    ws.send(JSON.stringify({ id, method, params }))
  })
await send('Runtime.enable')
const evalJs = async (expression, awaitPromise = false) => {
  const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise })
  if (r.result?.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails))
  // CDP shape: {id, result: {result: {type, value}}} — the inner `result`.
  return r.result?.result?.value
}

const bridge = await evalJs('!!(window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke)')
if (!bridge) fail('window.__TAURI_INTERNALS__.invoke missing — bridge not exposed')
ok('__TAURI_INTERNALS__ bridge present')

const instances = await evalJs('window.__TAURI_INTERNALS__.invoke("list_instances")', true)
if (!Array.isArray(instances)) fail(`invoke("list_instances") returned ${JSON.stringify(instances)}`)
ok(`invoke round-trip works: list_instances → [${instances.length} entries]`)

// document actually rendered something (first paint happened before app_ready
// shows the window; a rendered body means the frontend booted)
const rendered = await evalJs('document.body && document.body.children.length > 0')
if (!rendered) fail('document body is empty — the webview never rendered')
ok('document rendered')

console.log('\nCDP SPIKE: PASS — the installer smoke can drive PHL over raw CDP.')
ws.close()
tryProcessExit()
process.exit(0)
