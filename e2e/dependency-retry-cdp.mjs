#!/usr/bin/env node
/**
 * R4-01 GUI evidence — the dependency-retry FAILURE path, driven through the
 * real Tauri window.
 *
 *   node e2e/dependency-retry-cdp.mjs [--exe=<phl.exe>] [--out=<dir>] [--keep] [--no-vite]
 *
 * Why this lane exists: the round-4 review reproduced a user-visible lie — a
 * failed dependency install (a normal `Ok(outcome)` with
 * `readiness: needsDependencies` and `dependencyFailures`) was announced as
 * 「依赖安装完成 / 实例已就绪」. Component tests pin the contract; this lane
 * proves the whole page path: the reasons stored in the import record are on
 * screen, the retry really runs the command, the failure is reported as a
 * failure (never as success), the instance stays pending, and the damaged-
 * record guidance leads somewhere real.
 *
 * The fixture keeps the failure SAFE and offline: the import record's planned
 * dependency set deliberately disagrees with the profile's package.json, so
 * `prepare_dependencies` refuses before npm is ever spawned
 * (「profile package.json 与计划依赖集不一致：拒绝执行 npm」).
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
  sleep,
  waitForHttp,
  waitForText,
  waitForTextWithout,
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
const withVite = !process.argv.includes('--no-vite')
/** Optional CSS-viewport override for the screenshots: the card lives in a
 * narrow detail column, and a review must be able to re-check the labels at a
 * cramped width without touching the OS DPI. */
const width = Number(argOf('width', '0'))
const height = Number(argOf('height', '0'))
const PORT = Number(process.env.PHL_RETRY_CDP_PORT ?? 9449)

const VERSION = '9.9.9-cdp'
const RUNTIME = 'node-24.0.0'
/** `pending` = a live record whose dependencies are not installed and whose
 * machine file drifted; `corrupt` = a record that cannot be parsed at all. */
const SCENARIO = argOf('scenario', 'pending')
const INSTANCE = SCENARIO === 'corrupt' ? 'corrupt-src' : 'dep-src'

/** The import record's plan: `left-pad`. The profile's machine file asks for
 * `right-pad` instead — a drift the retry must refuse (and NOT run npm for). */
const HISTORIC_FAILURE = '上次尝试：npm 不可用（历史记录，来自导入记录）'
const EXPECTED_FAILURE =
  SCENARIO === 'corrupt' ? '导入记录损坏' : 'profile package.json 与计划依赖集不一致'

function seedRoot(root, scenario) {
  const write = (p, body) => {
    fs.mkdirSync(path.dirname(p), { recursive: true })
    fs.writeFileSync(p, body)
  }
  const now = new Date().toISOString()
  write(
    path.join(root, 'versions', VERSION, 'phl-install.json'),
    JSON.stringify({ installedAt: now, version: VERSION }),
  )
  write(
    path.join(root, 'versions', VERSION, 'package.json'),
    JSON.stringify({ name: '@deepseek-ai/dsh', version: VERSION }),
  )
  write(path.join(root, 'versions', VERSION, 'lib', 'bin.js'), '// fake dsh entry\n')
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

  const dir = path.join(root, 'instances', INSTANCE)
  const home = path.join(dir, 'dsh-home')
  write(
    path.join(home, 'profiles', 'web', 'package.json'),
    JSON.stringify({
      name: 'dsh-profile-community-pack',
      private: true,
      dependencies: { 'right-pad': '1.0.0' },
    }),
  )
  write(path.join(home, 'profiles', 'web', 'cordis.patch.yml'), '[]\n')
  fs.mkdirSync(path.join(dir, 'workspace'), { recursive: true })
  write(
    path.join(dir, 'phl-import.json'),
    scenario === 'corrupt'
      ? '{broken'
      : JSON.stringify({
          format: 'dspack',
          packSha256: 'a'.repeat(64),
          containerVersion: 1,
          manifestVersion: 1,
          bundles: [],
          plannedDependencies: { 'left-pad': '1.3.0' },
          dependenciesFailed: [HISTORIC_FAILURE],
          stage: 'needsDependencies',
        }),
  )
  write(
    path.join(dir, 'instance.json'),
    JSON.stringify({
      schemaVersion: 2,
      id: INSTANCE,
      name: scenario === 'corrupt' ? '记录损坏实例' : '依赖待补实例',
      kind: 'sandbox',
      hue: 0,
      versionId: `dsh-${VERSION}`,
      runtimeId: RUNTIME,
      port: 8125,
      autoPort: false,
      profile: 'web',
      createdAt: now,
      managementMode: 'pack-installed',
      source: 'phlpack',
      adoptedFrom: {
        dshHome: 'dspack:demo@1',
        detectedVersion: VERSION,
        adoptedAt: now,
        mode: 'community-import',
      },
    }),
  )
  return root
}

const steps = []
function step(id, title, status, detail = '') {
  steps.push({ id, title, status, detail })
  process.stdout.write(`  [${status}] ${id} ${title}${detail ? ` — ${detail}` : ''}\n`)
}

async function main() {
  if (!fs.existsSync(exe)) {
    console.error(`exe not found: ${exe}\nbuild it first: npm run build && cargo build --manifest-path src-tauri/Cargo.toml`)
    process.exit(2)
  }
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'phl-retry-cdp-'))
  const root = seedRoot(path.join(scratch, 'root'), SCENARIO)
  const exeSha256 = createHash('sha256').update(fs.readFileSync(exe)).digest('hex')
  process.stdout.write(
    `\nPHL dependency-retry failure evidence\n  exe  : ${exe}\n  root : ${root}\n  out  : ${outDir}\n\n`,
  )

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
  let ipc = null
  let exitCode = 1
  const evidence = {
    startedAt: new Date().toISOString(),
    scenario: `R4-01 dependency retry (${SCENARIO})`,
    exe,
    exeSha256,
    dataRoot: root,
    viewport: width > 0 ? { width, height: height > 0 ? height : 800 } : 'window default',
    steps,
  }
  try {
    const page = await connectCdp(PORT)
    cdp = cdpClient(page.webSocketDebuggerUrl)
    await cdp.ready
    await cdp.send('Runtime.enable')
    await cdp.send('Network.enable')
    ipc = ipcRecorder(cdp)
    step('R1', 'real window reachable over CDP', 'PASS', page.url)

    await waitForText(cdp, [SCENARIO === 'corrupt' ? '记录损坏实例' : '依赖待补实例'], 30000, 'R2')
    step('R2', 'the pending-import instance is on the instances page', 'PASS')

    // Open its detail page through the real menu.
    await clickText(cdp, '更多操作')
    await waitForText(cdp, ['查看详情'], 10000, 'R3')
    await clickText(cdp, '查看详情')
    const cardState =
      SCENARIO === 'corrupt'
        ? ['整合包导入记录缺失或已损坏', '重试依赖安装']
        : ['整合包依赖尚未安装完成', '重试依赖安装']
    const detail = await waitForText(cdp, cardState, 20000, 'R3')
    evidence.detailText = detail.slice(0, 2500)
    step('R3', 'retry card is on the detail page', 'PASS')

    if (SCENARIO === 'corrupt') {
      // The card must say what is true of a damaged record — no local rebuild,
      // re-import is the way out — and offer that next step.
      if (!detail.includes('无法在本地重建') || !detail.includes('重新导入整合包')) {
        throw new Error('R4: the damaged-record card does not name the real next step')
      }
      step('R4', 'damaged record: card says no local rebuild, offers re-import', 'PASS')
    } else {
      // The reasons the RECORD holds are on screen before any click: the card
      // reads them from the instance record, not from a toast.
      if (!detail.includes(HISTORIC_FAILURE)) {
        throw new Error('R4: the card does not show the reason stored in the import record')
      }
      step('R4', 'record-held failure reason is rendered', 'PASS', HISTORIC_FAILURE.slice(0, 18) + '…')
    }

    await clickText(cdp, '重试依赖安装')
    const afterRetry = await waitForTextWithout(
      cdp,
      ['依赖安装未完成', EXPECTED_FAILURE],
      ['依赖安装完成', '实例已就绪'],
      90000,
      'R5',
    )
    evidence.afterRetryText = afterRetry.slice(0, 3000)
    step('R5', 'failure is reported as a failure, never as success', 'PASS', EXPECTED_FAILURE)

    if (width > 0) {
      await cdp.send('Emulation.setDeviceMetricsOverride', {
        width,
        height: height > 0 ? height : 800,
        deviceScaleFactor: 1,
        mobile: false,
      })
      await sleep(500)
    }
    const shotFail = await cdp.send('Page.captureScreenshot', { format: 'png' })
    fs.mkdirSync(outDir, { recursive: true })
    const failShot = path.join(outDir, `dep-retry-${SCENARIO}-${label}.png`)
    fs.writeFileSync(failShot, Buffer.from(shotFail.result.data, 'base64'))
    evidence.failureScreenshot = failShot
    step('R6', 'failure screenshot captured', 'PASS', failShot)

    // The command really ran, and the instance state is what it should be.
    evidence.ipcCalls = ipc.filter((c) => c.command === 'prepare_pack_dependencies')
    if (!evidence.ipcCalls.length) throw new Error('R7: the retry never reached the backend')
    const records = await cdp.evaluate('window.__TAURI_INTERNALS__.invoke("list_instances")', {
      awaitPromise: true,
    })
    const record = (records ?? []).find((r) => r.id === INSTANCE)
    evidence.instanceRecord = record
      ? {
          id: record.id,
          readiness: record.readiness ?? null,
          importFailures: record.importFailures ?? [],
        }
      : null
    const expectedReadiness = SCENARIO === 'corrupt' ? 'corruptImport' : 'needsDependencies'
    if (record?.readiness !== expectedReadiness) {
      throw new Error(`R7: readiness became ${record?.readiness}, expected ${expectedReadiness}`)
    }
    if (SCENARIO === 'pending') {
      if (!(record?.importFailures ?? []).some((f) => f.includes('不一致'))) {
        throw new Error('R7: the fresh failure reason was not persisted into the record')
      }
      if (SCENARIO === 'pending' && !(record?.importFailures ?? []).length) {
        throw new Error('R7: no failure reasons on the record')
      }
    }
    step(
      'R7',
      'backend ran; instance keeps its true state',
      'PASS',
      `readiness=${record.readiness} failures=${(record.importFailures ?? []).length}`,
    )

    if (SCENARIO === 'corrupt') {
      // The guidance leads somewhere real: the pack import entry.
      await clickText(cdp, '重新导入整合包')
      await waitForText(cdp, ['安装整合包'], 20000, 'R8')
      evidence.afterReimportText = (await cdp.bodyText()).slice(0, 2000)
      const shotNav = await cdp.send('Page.captureScreenshot', { format: 'png' })
      const navShot = path.join(outDir, `dep-reimport-${label}.png`)
      fs.writeFileSync(navShot, Buffer.from(shotNav.result.data, 'base64'))
      evidence.reimportScreenshot = navShot
      step('R8', 're-import guidance opens the pack install page', 'PASS', navShot)
    }

    exitCode = 0
  } catch (err) {
    evidence.error = String(err?.message ?? err)
    step('RX', 'run failed', 'FAIL', evidence.error.split('\n')[0])
    if (cdp) {
      try {
        evidence.pageTextAtFailure = (await cdp.bodyText()).slice(0, 4000)
        evidence.ipcCallsAtFailure = ipc ?? []
      } catch {
        /* page already gone */
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
        spawn('taskkill', ['/pid', String(vite.pid), '/t', '/f'], { stdio: 'ignore' })
      } catch {
        /* best effort */
      }
    }
    cdp?.close()
    evidence.finishedAt = new Date().toISOString()
    fs.mkdirSync(outDir, { recursive: true })
    const evidencePath = path.join(outDir, `dep-retry-${SCENARIO}-${label}-evidence.json`)
    fs.writeFileSync(evidencePath, JSON.stringify(evidence, null, 2))
    process.stdout.write(`\n  evidence: ${evidencePath}\n`)
    if (!keep) {
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
