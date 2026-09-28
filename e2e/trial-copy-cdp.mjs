#!/usr/bin/env node
/**
 * R1·M3 trial-copy GUI evidence — `预览 → 创建副本 → 查看副本` driven through
 * the real Tauri window.
 *
 *   node e2e/trial-copy-cdp.mjs [--exe=<phl.exe>] [--out=<dir>] [--keep]
 *
 * Why this lane exists: the round-3 review (2026-09-26) rejected "the copy
 * flow works" evidence that never touched the page. R3-01 was a front/back
 * DTO break that only the real dialog could expose, and the reviewer asked for
 * a delivery that actually walks the page once and saves the request the UI
 * really sent.
 *
 * What it does, on a throwaway `PHL_ROOT` seeded by this script (no real
 * download, no developer data, no network):
 *
 *   1. launches the built exe with the WebView2 DevTools port open,
 *   2. wraps `window.__TAURI_INTERNALS__.invoke` so every `preview_trial` /
 *      `create_trial` payload the page sends is captured verbatim,
 *   3. clicks the real UI path — instance menu → 复制并试用新版 → 创建副本 →
 *      打开副本 — waiting on rendered text, never on sleeps alone,
 *   4. asserts the captured create request carries the preview-issued plan
 *      (R3-01) and that `list_instances` shows the copy on the new binding,
 *   5. writes `<out>/round3-gui-evidence.json` + a screenshot, and prints both.
 *
 * It is deliberately a lane script, not a test-runner file: it needs a real
 * window, real WebView2 and a medium-integrity token (WebView2 refuses the
 * DevTools port for elevated processes — proven on the alpha.5 lane).
 */
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import {
  cdpClient,
  clickText,
  connectCdp,
  ipcRecorder,
  json,
  sleep,
  waitForEnabledButton,
  waitForHttp,
  waitForText,
} from './lib/tauri-cdp.mjs'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const argOf = (name, fallback) => {
  const hit = process.argv.slice(2).find((a) => a.startsWith(`--${name}=`))
  return hit ? hit.split('=').slice(1).join('=') : fallback
}
const exe = path.resolve(argOf('exe', path.join(repoRoot, 'src-tauri', 'target', 'debug', 'dsh-phl.exe')))
const outDir = path.resolve(argOf('out', path.join(repoRoot, 'internal', 'acceptance', 'next-release')))
const keep = process.argv.includes('--keep')
const label = argOf('label', 'gui')
// A `cargo build` debug exe is a DEV build: it loads the UI from `devUrl`
// (localhost:5180) instead of the embedded assets, exactly like `tauri dev`.
// The lane therefore brings the Vite server up itself unless told not to.
const withVite = !process.argv.includes('--no-vite')
const PORT = Number(process.env.PHL_TRIAL_CDP_PORT ?? 9446)

/** Two installed versions: the source binds the older one, so the dialog's
 * default target is the newer — exactly the "试用新版" setup a user has. */
const SOURCE_VERSION = '9.9.8-cdp'
const TARGET_VERSION = '9.9.9-cdp'
const RUNTIME = 'node-24.0.0'
const SOURCE_ID = 'cdp-src'

/**
 * The throwaway data root this lane runs against: two installed versions (the
 * source binds the older one so the dialog's default target is the newer), a
 * runtime marker, and one managed source instance with something to copy.
 * Nothing is downloaded and no real user data is touched.
 */
function seedRoot(root) {
  const write = (p, body) => {
    fs.mkdirSync(path.dirname(p), { recursive: true })
    fs.writeFileSync(p, body)
  }
  const now = new Date().toISOString()
  for (const version of [SOURCE_VERSION, TARGET_VERSION]) {
    write(
      path.join(root, 'versions', version, 'phl-install.json'),
      JSON.stringify({ installedAt: now, version }),
    )
    write(
      path.join(root, 'versions', version, 'package.json'),
      JSON.stringify({ name: '@deepseek-ai/dsh', version }),
    )
    write(path.join(root, 'versions', version, 'lib', 'bin.js'), '// fake dsh entry\n')
  }
  write(
    path.join(root, 'runtimes', RUNTIME, 'phl-runtime.json'),
    JSON.stringify({
      installedAt: now,
      version: '24.0.0',
      platform: 'win-x64',
      arch: 'x64',
      bytes: 0,
      sha256: '',
    }),
  )
  const home = path.join(root, 'instances', SOURCE_ID, 'dsh-home')
  write(path.join(home, 'settings.yaml'), 'model: cdp-fixture\n')
  write(
    path.join(home, 'profiles', 'web', 'package.json'),
    JSON.stringify({ name: 'dsh-profile-web', private: true, dependencies: {} }),
  )
  write(path.join(home, 'profiles', 'web', 'cordis.patch.yml'), '[]\n')
  fs.mkdirSync(path.join(root, 'instances', SOURCE_ID, 'workspace'), { recursive: true })
  write(
    path.join(root, 'instances', SOURCE_ID, 'instance.json'),
    JSON.stringify({
      schemaVersion: 2,
      id: SOURCE_ID,
      name: 'CDP 源实例',
      kind: 'sandbox',
      hue: 0,
      versionId: `dsh-${SOURCE_VERSION}`,
      runtimeId: RUNTIME,
      port: 8123,
      autoPort: false,
      profile: 'web',
      createdAt: now,
    }),
  )
  return root
}

/** Waits until the COPY dialog's own version select offers real options — the
 * catalogue load is asynchronous, and picking a target requires it. Targeting
 * the labelled select matters: the scope/workspace selects always carry
 * values, so "some select has options" would pass with no version at all. */
async function waitForVersionOptions(cdp, timeoutMs, label) {
  const expr = `(() => {
    const field = [...document.querySelectorAll('label')].find((l) =>
      (l.textContent || '').includes('目标 DSH 版本'),
    )
    const select = field?.querySelector('select')
    const options = select ? [...select.options].map((o) => o.value).filter(Boolean) : []
    return { found: !!select, options }
  })()`
  const deadline = Date.now() + timeoutMs
  let last = null
  while (Date.now() < deadline) {
    last = await cdp.evaluate(expr)
    if (last?.options?.length) return last
    await sleep(300)
  }
  throw new Error(`${label}: the version select never offered an option (last=${JSON.stringify(last)})`)
}

/* --------------------------------- run --------------------------------- */
const steps = []
let ipc = null
function step(id, title, status, detail = '') {
  steps.push({ id, title, status, detail })
  process.stdout.write(`  [${status}] ${id} ${title}${detail ? ` — ${detail}` : ''}\n`)
}

async function main() {
  if (!fs.existsSync(exe)) {
    console.error(`exe not found: ${exe}\nbuild it first: npm run build && cargo build --manifest-path src-tauri/Cargo.toml`)
    process.exit(2)
  }
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'phl-trial-cdp-'))
  const root = seedRoot(path.join(scratch, 'root'))
  // Candidate identity travels with the evidence: the exe under test, byte-exact.
  const exeSha256 = createHash('sha256').update(fs.readFileSync(exe)).digest('hex')
  process.stdout.write(`\nPHL trial-copy GUI evidence\n  exe  : ${exe}\n  root : ${root}\n  out  : ${outDir}\n\n`)

  let vite = null
  if (withVite) {
    vite = spawn('npm', ['run', 'dev'], { cwd: repoRoot, shell: true, stdio: 'ignore' })
    await waitForHttp('http://localhost:5180/', 60000, 'vite')
    process.stdout.write('  vite dev server up on http://localhost:5180/\n')
  }

  const child = spawn(exe, [], {
    windowsHide: true,
    env: {
      ...process.env,
      PHL_ROOT: root,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
    },
    stdio: 'ignore',
  })

  let cdp = null
  let exitCode = 1
  const evidence = {
    startedAt: new Date().toISOString(),
    exe,
    exeSha256,
    dataRoot: root,
    steps,
  }
  try {
    const page = await connectCdp(PORT)
    cdp = cdpClient(page.webSocketDebuggerUrl)
    await cdp.ready
    await cdp.send('Runtime.enable')
    step('G1', 'real window reachable over CDP', 'PASS', page.url)

    // Record the real IPC traffic: every command the page sends lands on
    // `POST http://ipc.localhost/<cmd>`, so the create request can be read
    // verbatim instead of reconstructed.
    await cdp.send('Network.enable')
    ipc = ipcRecorder(cdp)
    step('G2', 'IPC transport observed (http://ipc.localhost)', 'PASS')

    await waitForText(cdp, ['CDP 源实例'], 30000, 'G3')
    step('G3', 'source instance rendered on the instances page', 'PASS')

    await clickText(cdp, '更多操作')
    await waitForText(cdp, ['复制并试用新版'], 10000, 'G4')
    await clickText(cdp, '复制并试用新版')
    const dialogText = await waitForText(cdp, ['复制并试用新版', '目标 DSH 版本'], 10000, 'G5')
    evidence.dialogText = dialogText.slice(0, 2000)
    step('G4', 'copy-trial dialog opened from the instance menu', 'PASS')

    // The preview must have landed before the button can be pressed: the
    // dialog disables create until a plan for the current knobs exists. The
    // catalogue load is asynchronous, so the version options (and with them
    // the default target and the preview) may only appear after a while.
    await waitForVersionOptions(cdp, 90000, 'G6')
    await waitForText(cdp, [`dsh-${TARGET_VERSION}`], 30000, 'G6')
    await waitForEnabledButton(cdp, '创建副本', 60000, 'G6')
    evidence.previewCalls = ipc
      .filter((c) => c.command === 'preview_trial')
      .map((c) => ({ at: c.at, body: c.body }))
    if (!evidence.previewCalls.length) {
      throw new Error('G6: the dialog never sent preview_trial')
    }
    step('G5', 'preview completed and 创建副本 enabled', 'PASS')

    await clickText(cdp, '创建副本')
    const resultText = await waitForText(cdp, ['副本已创建'], 120000, 'G7')
    evidence.resultText = resultText.slice(0, 3000)
    step('G6', 'create finished and the result panel is on screen', 'PASS')

    evidence.ipcCalls = ipc
    const createCall = ipc.filter((c) => c.command === 'create_trial').pop()
    const create = createCall ? JSON.parse(createCall.body ?? '{}') : null
    const req = create?.req
    if (!req?.planId || !req.targetId || !req.sourceFingerprint) {
      throw new Error(`G8: the page's create request carries no plan: ${JSON.stringify(create)}`)
    }
    evidence.createRequest = req
    step(
      'G7',
      'the page sent the preview-issued plan',
      'PASS',
      `planId=${req.planId.slice(0, 12)}… targetId=${req.targetId}`,
    )

    // 查看副本: the record the app itself would show, then the detail page.
    const records = await cdp.evaluate('window.__TAURI_INTERNALS__.invoke("list_instances")', {
      awaitPromise: true,
    })
    const copy = (records ?? []).find((r) => r.id === req.targetId)
    evidence.copyRecord = copy
      ? {
          id: copy.id,
          name: copy.name,
          versionId: copy.versionId,
          runtimeId: copy.runtimeId,
          port: copy.port,
          readiness: copy.readiness ?? null,
          dshHome: copy.dshHome,
        }
      : null
    if (!copy) throw new Error(`G9: list_instances has no copy ${req.targetId}`)
    await clickText(cdp, '打开副本')
    await waitForText(cdp, [copy.name, 'Profile'], 30000, 'G9')
    evidence.pageTextAfterOpen = (await cdp.bodyText()).slice(0, 3000)
    step('G8', 'copy visible as an instance after 打开副本', 'PASS', `${copy.id} · ${copy.versionId} · :${copy.port}`)

    const shot = await cdp.send('Page.captureScreenshot', { format: 'png' })
    const shotPath = path.join(outDir, `trial-copy-${label}.png`)
    fs.mkdirSync(outDir, { recursive: true })
    fs.writeFileSync(shotPath, Buffer.from(shot.result.data, 'base64'))
    evidence.screenshot = shotPath
    step('G9', 'screenshot captured', 'PASS', shotPath)

    exitCode = 0
  } catch (err) {
    evidence.error = String(err?.message ?? err)
    step('GX', 'run failed', 'FAIL', evidence.error.split('\n')[0])
    if (cdp) {
      // The scene matters more than the message: keep what the page showed and
      // what it had already sent when a step failed.
      try {
        evidence.pageTextAtFailure = (await cdp.bodyText()).slice(0, 4000)
        evidence.ipcCallsAtFailure = ipc ?? []
      } catch {
        /* the page may already be gone */
      }
    }
  } finally {
    try {
      child.kill()
    } catch {
      /* already gone */
    }
    if (vite) {
      try {
        // npm spawns node as a child; taskkill takes the tree.
        spawn('taskkill', ['/pid', String(vite.pid), '/t', '/f'], { stdio: 'ignore' })
      } catch {
        /* best effort */
      }
    }
    cdp?.close()
    evidence.finishedAt = new Date().toISOString()
    fs.mkdirSync(outDir, { recursive: true })
    const evidencePath = path.join(outDir, `trial-copy-${label}-evidence.json`)
    fs.writeFileSync(evidencePath, JSON.stringify(evidence, null, 2))
    process.stdout.write(`\n  evidence: ${evidencePath}\n`)
    if (!keep) {
      // The app writes into the root until it dies; retry once after the kill.
      await sleep(500)
      try {
        fs.rmSync(scratch, { recursive: true, force: true })
      } catch {
        /* a stray handle keeps the temp dir; harmless */
      }
    } else {
      process.stdout.write(`  root kept: ${root}\n`)
    }
  }
  process.exit(exitCode)
}

main().catch((err) => {
  console.error(err)
  process.exit(1)
})
