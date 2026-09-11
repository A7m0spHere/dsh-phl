#!/usr/bin/env node
/**
 * PHL Windows installer smoke — the gate's OS-level lane.
 *
 * Answers the question the unit suites cannot: does the thing a user actually
 * downloads install, start, upgrade, and uninstall correctly? Drives the
 * real app over the WebView2 DevTools port (raw CDP — no Playwright, no
 * WinAppDriver; feasibility proven by `cdp-spike.mjs`), never touching
 * PHL_ROOT: an installed app run is its own evidence.
 *
 * Usage:
 *   node e2e/install-smoke.mjs --installer=<setup.exe> [--expect-version=X.Y.Z]
 *                             [--out=<report-dir>] [--keep]
 *   node e2e/install-smoke.mjs --exe=<phl.exe>        # run-only smoke
 *
 * Steps for the installer mode:
 *   I-1  silent install (`/S`)                     → files on disk
 *   I-2  exe file version == --expect-version      → no half-applied bump
 *   I-3  start installed app + CDP                 → first-launch works
 *   I-4  app bridge + real IPC round-trip          → backend answers
 *   I-5  3 s of console/exception capture          → shipped UI is clean
 *   I-6  updater manifest contract (host-side)     → remote never AHEAD of the
 *        release being published (older is the pre-publish norm; the
 *        `updates` branch is only pushed after this gate passes)
 *   I-7  silent upgrade (`/S` again)               → app still starts
 *   I-8  silent uninstall (Uninstall.exe /S)       → files gone
 *   I-9  root pointer survives uninstall           → user data promise
 *
 * Exit code 0 = every step passed; the JSON report names each verdict.
 * A step that already FAILed is never overwritten by a later PASS for the
 * same id — the evidence trail must not contain both verdicts.
 */
import { execFile, spawn } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'

const PORT = Number(process.env.PHL_SMOKE_CDP_PORT ?? 9444)
const args = process.argv.slice(2)
const argOf = (name) => {
  const hit = args.find((a) => a.startsWith(`--${name}=`))
  return hit ? hit.slice(name.length + 3) : undefined
}
const installer = argOf('installer')
const directExe = argOf('exe')
const expectVersion = argOf('expect-version')
const outDir = argOf('out') ?? path.join(os.tmpdir(), 'phl-install-smoke-report')
const keep = args.includes('--keep')

if (!installer && !directExe) {
  console.error('usage: install-smoke.mjs --installer=<setup.exe> | --exe=<phl.exe> [--expect-version=V]')
  process.exit(2)
}
fs.mkdirSync(outDir, { recursive: true })

const report = { startedAt: new Date().toISOString(), installer: installer ?? null, exe: directExe ?? null, steps: [] }
const failedIds = new Set()
const step = (id, name, verdict, detail = '') => {
  if (verdict === 'PASS' && failedIds.has(id)) {
    // A FAIL already owns this step id; a later PASS would be false evidence.
    return
  }
  if (verdict === 'FAIL') failedIds.add(id)
  report.steps.push({ id, name, verdict, detail, at: new Date().toISOString() })
  console.log(`${verdict === 'PASS' ? 'ok  ' : verdict === 'SKIP' ? 'skip' : 'FAIL'}  [${id}] ${name}${detail ? ` — ${detail}` : ''}`)
}

let app = null
const killApp = () => {
  if (keep) return
  const pid = app?.pid
  if (!pid) return
  // Detached (group-leader) child: kill the whole tree. Spawned fire-and-
  // forget — awaiting it races libuv teardown on Windows (uv async.c assert).
  try {
    const killer = spawn('taskkill', ['/PID', String(pid), '/T', '/F'], { windowsHide: true })
    killer.on('error', () => {})
  } catch {
    try {
      app.kill('SIGKILL')
    } catch {
      /* already gone */
    }
  }
}
let finished = false
const finish = (code) => {
  if (finished) return
  finished = true
  report.finishedAt = new Date().toISOString()
  report.exitCode = code
  killApp()
  const file = path.join(outDir, `install-smoke-${Date.now()}.json`)
  fs.writeFileSync(file, JSON.stringify(report, null, 2))
  console.log(`\nreport: ${file}`)
  // `process.exit` would run libuv teardown with the taskkill child and CDP
  // socket in flight — that asserts on Windows. The report is flushed
  // synchronously above; exit pure.
  process.reallyExit(code)
}
// Watchdog: a wedged step must still produce a report within 25 minutes.
const watchdog = setTimeout(() => {
  report.steps.push({ id: 'X-0', name: 'watchdog', verdict: 'FAIL', detail: '25 minute budget exceeded', at: new Date().toISOString() })
  finish(1)
}, 25 * 60 * 1000)
watchdog.unref()
process.on('uncaughtException', (e) => {
  report.steps.push({ id: 'X-1', name: 'uncaught', verdict: 'FAIL', detail: String(e?.stack ?? e).slice(0, 800), at: new Date().toISOString() })
  finish(1)
})

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const run = (cmd, cmdArgs, opts = {}) =>
  new Promise((resolve, reject) => {
    execFile(cmd, cmdArgs, { windowsHide: true, ...opts }, (err, stdout, stderr) => {
      if (err) reject(Object.assign(err, { stdout, stderr }))
      else resolve(stdout)
    })
  })

// ------------------------- tiny raw-CDP client -------------------------
async function connectCdp(timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs
  let targets = null
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://127.0.0.1:${PORT}/json/list`)
      if (res.ok) {
        targets = await res.json()
        const page = targets.find((t) => t.type === 'page' && t.url.startsWith('http'))
        if (page?.webSocketDebuggerUrl) return page
      }
    } catch {
      /* endpoint not up yet */
    }
    await sleep(500)
  }
  throw new Error(`no page target on DevTools port ${PORT} within ${timeoutMs} ms`)
}

function cdpClient(wsUrl) {
  const ws = new WebSocket(wsUrl)
  let id = 0
  const pending = new Map()
  const listeners = []
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data)
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg)
      pending.delete(msg.id)
    } else if (msg.method) {
      for (const fn of listeners) fn(msg)
    }
  }
  const open = new Promise((resolve, reject) => {
    ws.onopen = resolve
    ws.onerror = () => reject(new Error('cdp websocket failed'))
  })
  const send = (method, params = {}) =>
    new Promise((resolve) => {
      const n = ++id
      pending.set(n, resolve)
      ws.send(JSON.stringify({ id: n, method, params }))
    })
  return {
    ready: open,
    send,
    onEvent: (fn) => listeners.push(fn),
    close: () => ws.close(),
    async evaluate(expression, { awaitPromise = false } = {}) {
      const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise })
      const ex = r.result?.exceptionDetails
      if (ex) throw new Error(`page eval threw: ${JSON.stringify(ex).slice(0, 300)}`)
      return r.result?.result?.value
    },
  }
}

// ------------------------------ app start ------------------------------
async function startInstalled(exe) {
  app = spawn(exe, [], {
    windowsHide: true,
    detached: true,
    env: {
      ...process.env,
      // The one setting that opens the WebView2 DevTools port.
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
    },
  })
  app.on('error', () => {
    /* the early-exit check below reports it */
  })
  app.unref()
  const started = app
  await sleep(4000)
  if (started.exitCode !== null) {
    throw new Error(`app exited early with code ${started.exitCode}`)
  }
  return started
}

/* --------------------- locate the installed exe ---------------------- */
// Scan the same registry view "Apps & features" uses, by DisplayName — the
// NSIS key spelling (`…_is1`) and the per-user vs per-machine hive both
// depend on template internals and elevation, so neither is guessed here.
async function locateInstalledExe() {
  const ps =
    `Get-ChildItem HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall, ` +
    `HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall, ` +
    `HKLM:\\Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall ` +
    `-ErrorAction SilentlyContinue | ForEach-Object { Get-ItemProperty $_.PSPath } | ` +
    `Where-Object { $_.DisplayName -eq 'PHL' -and $_.InstallLocation } | ` +
    `Select-Object -First 1 -ExpandProperty InstallLocation`
  try {
    const out = (await run('powershell', ['-NoProfile', '-Command', ps], { timeout: 30000 })).trim()
    for (const line of out.split(/\r?\n/)) {
      const loc = line.trim()
      if (!loc) continue
      const candidate = path.join(loc, 'PHL.exe')
      if (fs.existsSync(candidate)) return candidate
    }
  } catch {
    /* registry scan failed — fall through to known per-user paths */
  }
  // Per-user NSIS defaults to `$LOCALAPPDATA\{productName}`; per-machine to
  // Program Files. The identifier fallbacks cover template variants.
  const guesses = [
    path.join(process.env.LOCALAPPDATA ?? '', 'PHL', 'PHL.exe'),
    path.join(process.env.LOCALAPPDATA ?? '', 'dev.dshphl.desktop', 'PHL.exe'),
    path.join(process.env['ProgramFiles'] ?? 'C:\\Program Files', 'PHL', 'PHL.exe'),
  ]
  return guesses.find((g) => g && fs.existsSync(g)) ?? null
}

// ------------------------------ the lane ------------------------------
let installedExe = directExe ?? null

if (installer) {
  if (!fs.existsSync(installer)) {
    step('I-0', 'installer exists', 'FAIL', `not found: ${installer}`)
  } else {
    step('I-0', 'installer exists', 'PASS', installer)
  }

  // I-1 silent install. `/S` is Tauri's NSIS silent flag. The bundle uses
  // `installMode: "both"`: when the process token is elevated (CI runners
  // are) NSIS defaults silent installs to per-machine — registry HKLM +
  // Program Files, and `/D=` is ignored. The locate step reads both hives
  // accordingly; if elevation semantics ever change, the 3-minute timeout
  // plus the FAIL below surface it rather than hanging the gate.
  try {
    await run(installer, ['/S'], { timeout: 180000 })
  } catch (e) {
    step('I-1', 'silent install completes', 'FAIL', `installer exited with error: ${e.message}`)
  }
  if (!failedIds.has('I-1')) {
    installedExe = await locateInstalledExe()
    if (!installedExe) {
      step('I-1', 'silent install completes', 'FAIL', 'installed PHL.exe not found (registry scan or known paths)')
    } else {
      step('I-1', 'silent install completes', 'PASS', installedExe)
      report.installDir = path.dirname(installedExe)
    }
  }

  // I-2 file version metadata matches the release (PowerShell owns the API).
  if (failedIds.has('I-1')) {
    step('I-2', 'exe version matches release', 'SKIP', 'install not located')
  } else if (expectVersion) {
    try {
      const v = (
        await run('powershell', [
          '-NoProfile',
          '-Command',
          `(Get-Item '${installedExe.replaceAll("'", "''")}').VersionInfo.FileVersion`,
        ])
      ).trim()
      if (!v.includes(expectVersion)) {
        step('I-2', 'exe version matches release', 'FAIL', `file version "${v}" lacks ${expectVersion}`)
      } else {
        step('I-2', 'exe version matches release', 'PASS', v)
      }
    } catch (e) {
      step('I-2', 'exe version matches release', 'FAIL', e.message)
    }
  } else {
    step('I-2', 'exe version matches release', 'SKIP', 'no --expect-version given')
  }
}

// I-3 first launch of the *installed* binary
let cdp = null
if (!installedExe) {
  step('I-3', 'installed app starts and holds a window', 'FAIL', 'no exe to start (install not located)')
} else {
  try {
    await startInstalled(installedExe)
    step('I-3', 'installed app starts and holds a window', 'PASS')
    const page = await connectCdp()
    cdp = cdpClient(page.webSocketDebuggerUrl)
    await cdp.ready
    await cdp.send('Runtime.enable')
    step('I-3b', 'CDP page target reachable', 'PASS', page.url)
  } catch (e) {
    step('I-3', 'installed app starts and holds a window', 'FAIL', e.message)
  }
}

// I-4 bridge + real IPC round-trip. The visible window is itself proof the
// `app_ready` handshake fired (the shell starts hidden).
if (cdp) {
  try {
    const hasInvoke = await cdp.evaluate('typeof (window.__TAURI_INTERNALS__||{}).invoke')
    if (hasInvoke !== 'function') throw new Error(`invoke is ${hasInvoke}`)
    const count = await cdp.evaluate('window.__TAURI_INTERNALS__.invoke("list_instances").then(r => r.length)', {
      awaitPromise: true,
    })
    if (typeof count !== 'number') throw new Error(`unexpected reply: ${JSON.stringify(count)}`)
    step('I-4', 'bridge + list_instances round-trip', 'PASS', `${count} instances visible`)
  } catch (e) {
    step('I-4', 'bridge + list_instances round-trip', 'FAIL', e.message)
  }

  // I-5 quiet console: any page exception in a settled 3 s window fails.
  try {
    const problems = []
    await cdp.send('Log.enable').catch(() => {})
    cdp.onEvent((msg) => {
      if (msg.method === 'Runtime.exceptionThrown') {
        problems.push(msg.params?.details?.exception?.description ?? 'exception')
      }
      if (msg.method === 'Log.entryAdded' && msg.params?.entry?.level === 'error') {
        const text = msg.params.entry.text ?? ''
        // WebView2 emits benign network noise for devtools/favicon; keep the
        // filter explicit and visible in the report detail.
        if (!/favicon|DevTools|websocket/i.test(text)) problems.push(text)
      }
    })
    await sleep(3000)
    if (problems.length) {
      step('I-5', 'no console errors on first window', 'FAIL', problems.slice(0, 3).join(' | ').slice(0, 500))
    } else {
      step('I-5', 'no console errors on first window', 'PASS')
    }
  } catch (e) {
    step('I-5', 'no console errors on first window', 'FAIL', e.message)
  }
} else {
  step('I-4', 'bridge + real IPC round-trip', 'SKIP', 'no CDP session')
  step('I-5', 'no console errors on first window', 'SKIP', 'no CDP session')
}

// I-6 updater manifest contract (host side). This gate runs BEFORE the new
// version is published, so the endpoint legitimately still serves the
// PREVIOUS version — older-than-build is normal and passes. What must never
// happen at this point is the endpoint serving something NEWER than the
// version being released (the manifest ran ahead of a tag, or the tag was
// reused): installed apps would then consider the build stale mid-publish.
// Transport failure SKIPs (a flaky network must not red the gate; the very
// next workflow steps need the network anyway). On a repo whose first ever
// release hasn't seeded the updates branch, 404 is a contract FAIL — the
// seeding step lives in release.yml and the first alpha proved it.
try {
  const latest = await new Promise((resolve, reject) => {
    import('node:https').then((https) => {
      const req = https.get(
        'https://raw.githubusercontent.com/A7m0spHere/dsh-phl/updates/latest.json',
        { headers: { Connection: 'close' } },
        (res) => {
          if (res.statusCode !== 200) {
            return reject(Object.assign(new Error(`endpoint returned ${res.statusCode}`), { contract: true }))
          }
          let raw = ''
          res.setEncoding('utf8')
          res.on('data', (c) => (raw += c))
          res.on('end', () => {
            try {
              const version = JSON.parse(raw).version
              if (typeof version !== 'string' || !version) {
                reject(Object.assign(new Error('manifest has no version'), { contract: true }))
              } else {
                resolve(version)
              }
            } catch (e) {
              reject(Object.assign(e, { contract: true }))
            }
          })
        },
      )
      req.setTimeout(15000, () => req.destroy(new Error('manifest fetch timed out')))
      req.on('error', reject)
    })
  }).catch((e) => {
    if (e.contract) throw e
    step('I-6', 'updater manifest contract', 'SKIP', `network unreachable: ${e.message}`)
    return null
  })
  if (latest !== null) {
    if (expectVersion) {
      const cmp = latest.localeCompare(expectVersion, undefined, { numeric: true })
      if (cmp > 0) {
        step('I-6', 'updater manifest contract', 'FAIL', `updates branch already serves ${latest}, newer than the ${expectVersion} being released — manifest ran ahead of the tag`)
      } else {
        step('I-6', 'updater manifest contract', 'PASS', `updates=${latest} ≤ build=${expectVersion}`)
      }
    } else {
      step('I-6', 'updater manifest contract', 'PASS', `updates=${latest} (no --expect-version to compare)`)
    }
  }
} catch (e) {
  step('I-6', 'updater manifest contract', 'FAIL', e.message)
}

// I-7 / I-8 / I-9 — the upgrade + uninstall pair, only in installer mode.
if (installer && installedExe && !failedIds.has('I-1')) {
  killApp()
  await sleep(2000)
  try {
    await run(installer, ['/S'], { timeout: 180000 })
    await startInstalled(installedExe)
    step('I-7', 'silent upgrade then relaunch', 'PASS')
    killApp()
    await sleep(2000)
  } catch (e) {
    step('I-7', 'silent upgrade then relaunch', 'FAIL', e.message)
    killApp()
  }

  if (!failedIds.has('I-7')) {
    try {
      const guess = path.join(path.dirname(installedExe), 'Uninstall PHL.exe')
      let uninstaller = guess
      if (!fs.existsSync(guess)) {
        // Fall back to exactly what "Apps & features" invokes.
        const ps =
          `Get-ChildItem HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall, ` +
          `HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall, ` +
          `HKLM:\\Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall ` +
          `-ErrorAction SilentlyContinue | ForEach-Object { Get-ItemProperty $_.PSPath } | ` +
          `Where-Object { $_.DisplayName -eq 'PHL' } | ` +
          `Select-Object -First 1 -ExpandProperty UninstallString`
        const raw = (await run('powershell', ['-NoProfile', '-Command', ps], { timeout: 30000 })).trim()
        uninstaller = raw.replace(/^"|"$/g, '')
      }
      if (!uninstaller || !fs.existsSync(uninstaller)) throw new Error(`uninstaller not found (tried ${guess})`)
      // The app must not be running: NSIS refuses otherwise.
      killApp()
      await sleep(1500)
      const pointer = path.join(process.env.APPDATA ?? '', 'PHL', 'root.json')
      // (paths.rs:204 — the pointer lives in %APPDATA%\PHL.)
      const existed = fs.existsSync(pointer)
      await run(uninstaller, ['/S'], { timeout: 180000 })
      await sleep(3000)
      if (fs.existsSync(installedExe)) throw new Error('installed exe survived uninstall')
      step('I-8', 'silent uninstall removes program files', 'PASS')

      // I-9 user-data promise: the root pointer (and everything it names)
      // outlives the uninstall. On a truly clean machine the pointer may
      // never have existed (the first-launch chooser flow) — then its
      // absence is also a PASS; what must never happen is an uninstall that
      // deletes it.
      const still = fs.existsSync(pointer)
      if (existed && !still) {
        step('I-9', 'user data survives uninstall', 'FAIL', `uninstall deleted ${pointer}`)
      } else {
        step('I-9', 'user data survives uninstall', 'PASS', existed ? 'pointer preserved' : 'no pointer existed (clean machine)')
      }
    } catch (e) {
      step('I-8', 'uninstall lane', 'FAIL', e.message)
      step('I-9', 'user data survives uninstall', 'SKIP', 'uninstall did not complete')
    }
  } else {
    step('I-8', 'uninstall lane', 'SKIP', 'upgrade step failed (do not uninstall over a broken state)')
    step('I-9', 'user data survives uninstall', 'SKIP', 'upgrade step failed')
  }
} else {
  step('I-7', 'upgrade lane', 'SKIP', installer ? 'install not located' : 'run-only mode')
  step('I-8', 'uninstall lane', 'SKIP', installer ? 'install not located' : 'run-only mode')
  step('I-9', 'user data promise', 'SKIP', installer ? 'install not located' : 'run-only mode')
}

try {
  cdp?.close()
} catch {
  /* already closed */
}
finish(failedIds.size === 0 ? 0 : 1)
