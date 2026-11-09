#!/usr/bin/env node
// Windows process-panel acceptance against controlled Node HTTP processes.
// Build first: npm run build && cargo build --manifest-path src-tauri/Cargo.toml
// Run: node e2e/process-panel-cdp.mjs [--exe=...] [--out=...] [--keep]
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { execFileSync, spawn } from 'node:child_process'
import os from 'node:os'
import { fileURLToPath } from 'node:url'
import { cdpClient, connectCdp, ipcRecorder, sleep, waitForHttp } from './lib/tauri-cdp.mjs'

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const option = (name, fallback) => process.argv.find((arg) => arg.startsWith(`--${name}=`))?.slice(name.length + 3) ?? fallback
const exe = path.resolve(option('exe', path.join(repo, 'src-tauri/target/debug/dsh-phl.exe')))
const out = path.resolve(option('out', path.join(repo, 'internal/acceptance/2026-11-08-issue-22')))
const keep = process.argv.includes('--keep')
const port = 9471
const version = '99.0.0-process-fixture'
const runtimeId = `node-${process.versions.node}`
if (process.platform !== 'win32') throw new Error('This CDP lane requires Windows WebView2')
fs.mkdirSync(path.join(repo, '.tmp'), { recursive: true })
fs.mkdirSync(out, { recursive: true })
const scratch = fs.mkdtempSync(path.join(repo, '.tmp/process-panel-cdp-'))
const root = path.join(scratch, 'root')
const report = { startedAt: new Date().toISOString(), fixture: 'controlled Node HTTP processes', root, checks: [], errors: [] }
const record = (name, result = true) => { report.checks.push({ name, result }); console.log(name, JSON.stringify(result)) }
const write = (name, body) => {
  const target = path.join(root, name)
  fs.mkdirSync(path.dirname(target), { recursive: true })
  fs.writeFileSync(target, body)
}
const manifest = (id, name, env = {}, extra = {}) => ({
  schemaVersion: 2, id, name, kind: 'test', hue: 0, versionId: `dsh-${version}`,
  runtimeId, port: 3080, autoPort: true, profile: 'web', createdAt: new Date().toISOString(),
  totalRuntime: 0, favorite: false, env, args: [], api: { inheritance: 'none', providerIds: [] }, ...extra,
})
write(`versions/${version}/phl-install.json`, JSON.stringify({ version, installedAt: new Date().toISOString() }))
write(`versions/${version}/package.json`, JSON.stringify({ name: '@deepseek-ai/dsh', version }))
write(`versions/${version}/lib/bin.js`, `
const fs = require('fs'), path = require('path'), http = require('http');
const port = Number(process.argv[process.argv.indexOf('--port') + 1]);
const home = process.env.DSH_HOME;
fs.mkdirSync(home, { recursive: true });
fs.writeFileSync(path.join(home, 'root.pid'), String(process.pid));
const memory = Buffer.alloc(16 * 1024 * 1024, 1);
if (!process.env.FIXTURE_CRASH_MS) {
  const child = require('child_process').spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { windowsHide: true, stdio: 'ignore' });
  fs.writeFileSync(path.join(home, 'child.pid'), String(child.pid));
}
setInterval(() => { const end = Date.now() + 15; while (Date.now() < end) memory[0] ^= 1; }, 100);
setTimeout(() => {
  http.createServer((_req, res) => res.end('process fixture')).listen(port, '127.0.0.1', () => {
    console.log('dsh web: http://127.0.0.1:' + port + '/?token=process-fixture');
    if (process.env.FIXTURE_CRASH_MS) setTimeout(() => process.exit(7), Number(process.env.FIXTURE_CRASH_MS));
  });
}, Number(process.env.FIXTURE_START_DELAY_MS || 0));
`)
write(`runtimes/${runtimeId}/phl-runtime.json`, JSON.stringify({ version: process.versions.node, installedAt: new Date().toISOString(), platform: 'win-x64', arch: process.arch, bytes: 0, sha256: '' }))
fs.copyFileSync(process.execPath, path.join(root, 'runtimes', runtimeId, 'node.exe'))
const special = [
  manifest('proc-a', '进程验收 A'), manifest('proc-b', '进程验收 B'),
  manifest('proc-crash', '进程验收 异常退出', { FIXTURE_CRASH_MS: '1800' }),
  manifest('proc-port', '进程验收 端口冲突', {}, { autoPort: false, port: 3081 }),
  manifest('proc-slow', '进程验收 慢启动', { FIXTURE_START_DELAY_MS: '8000' }),
]
const all = [...special, ...Array.from({ length: 500 }, (_, index) => manifest(`idle-${String(index).padStart(4, '0')}`, `历史实例 ${index}`))]
for (const instance of all) write(`instances/${instance.id}/instance.json`, JSON.stringify(instance))

let preview, child, cdp
const pids = new Set()
async function waitFor(check, label, timeout = 30000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    const result = await check()
    if (result) return result
    await sleep(200)
  }
  throw new Error(`${label} timed out`)
}
async function button(label, scope = 'document') {
  await waitFor(() => cdp.evaluate(`(() => {
    const scope=${scope};
    if (!scope) return false;
    const target=[...scope.querySelectorAll('button')].find(button => button.getAttribute('aria-label') === ${JSON.stringify(label)} || button.textContent.trim() === ${JSON.stringify(label)});
    if (!target || target.disabled) return false;
    target.click(); return true;
  })()`), `button ${label} enabled`)
}
const samples = () => cdp.evaluate('window.__TAURI_INTERNALS__.invoke("plugin:process-monitor|sample_instance_processes")', { awaitPromise: true })
const rowText = (id) => cdp.evaluate(`document.querySelector('[data-instance-id="${id}"]')?.innerText ?? ''`)
async function filter(value) {
  await cdp.evaluate(`(() => {
    const input=document.querySelector('input[placeholder="搜索实例"]');
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,${JSON.stringify(value)});
    input.dispatchEvent(new Event('input',{bubbles:true}));
  })()`)
  await sleep(250)
}
async function shot(name) {
  const result = await cdp.send('Page.captureScreenshot', { format: 'png' })
  fs.writeFileSync(path.join(out, name + '.png'), Buffer.from(result.result.data, 'base64'))
}
function family(id) {
  const home = path.join(root, 'instances', id, 'dsh-home')
  const found = ['root.pid', 'child.pid'].filter((file) => fs.existsSync(path.join(home, file))).map((file) => Number(fs.readFileSync(path.join(home, file), 'utf8')))
  for (const pid of found) pids.add(pid)
  return found
}
function gone(pid) {
  try { process.kill(pid, 0); return false } catch { return true }
}
async function stopAll() {
  await button('全部停止')
  await waitFor(() => cdp.evaluate('document.getElementById("phl-dialog-title")?.textContent === "停止全部实例"'), 'confirmation')
  await button('全部停止', 'document.querySelector("[aria-labelledby=phl-dialog-title]")')
  await waitFor(async () => (await samples()).length === 0, 'all registrations removed')
}

try {
  const existing = await fetch('http://127.0.0.1:5180/').then((response) => response.text(), () => null)
  if (existing && !existing.includes('/assets/')) throw new Error('Port 5180 must serve built assets, not the HMR development page')
  if (!existing) {
    preview = spawn(process.execPath, [path.join(repo, 'node_modules/vite/bin/vite.js'), 'preview', '--host', '127.0.0.1', '--port', '5180', '--strictPort'], { cwd: repo, windowsHide: true, stdio: 'ignore' })
    await waitForHttp('http://127.0.0.1:5180/', 30000, 'preview')
  }
  child = spawn(exe, [], { windowsHide: true, stdio: 'ignore', env: { ...process.env, PHL_ROOT: root, WEBVIEW2_USER_DATA_FOLDER: path.join(scratch, 'webview'), WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` } })
  const page = await connectCdp(port)
  cdp = cdpClient(page.webSocketDebuggerUrl)
  await cdp.ready
  await cdp.send('Runtime.enable')
  await cdp.send('Network.enable')
  await cdp.send('Page.enable')
  const ipc = ipcRecorder(cdp)
  report.commands = ipc
  cdp.onEvent((message) => {
    if (message.method === 'Runtime.exceptionThrown') {
      const details = message.params.exceptionDetails
      report.errors.push(details.exception?.description ?? details.text)
    }
  })
  await waitFor(() => cdp.evaluate('document.getElementById("phl-guide-title") !== null'), 'first-run guide', 180000)
  await button('跳过', 'document.querySelector("[aria-labelledby=phl-guide-title]")')
  await waitFor(() => cdp.evaluate('document.getElementById("phl-guide-title") === null'), 'guide dismissed')
  await button('设置')
  await waitFor(() => cdp.evaluate('document.querySelector(\'[aria-label="启动实例后自动打开 WebUI"]\') !== null'), 'WebUI preference')
  if (await cdp.evaluate('document.querySelector(\'[aria-label="启动实例后自动打开 WebUI"]\').getAttribute("aria-checked") === "true"')) {
    await button('启动实例后自动打开 WebUI')
  }
  await button('实例')
  await waitFor(() => cdp.evaluate('document.querySelector("[aria-label=进程总览]") !== null'), 'instances loaded')
  const before = performance.now()
  await button('进程总览')
  await waitFor(() => cdp.evaluate('document.querySelectorAll("tbody [data-instance-id]").length === 50'), 'paged table')
  record('505-instances-render-50-rows', { elapsedMs: Math.round(performance.now() - before), count: 505, rendered: 50 })
  await filter('进程验收')
  await button('启动 进程验收 A')
  await waitFor(async () => (await rowText('proc-a')).includes('运行中') && (await rowText('proc-a')).includes('%'), 'first process sampled')
  const a = (await samples()).find((sample) => sample.instanceId === 'proc-a')
  assert(a.memoryBytes > 1024 * 1024 && a.cpuPercent !== null && a.startedAtMs > 0)
  const firstFamily = family('proc-a')
  record('native-cpu-memory-and-start-time', a)
  const cpuCounter = () => JSON.parse(execFileSync('powershell.exe', ['-NoProfile', '-Command',
    `$p=Get-Process -Id ${a.pid}; @{cpu=$p.CPU;time=[DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()} | ConvertTo-Json -Compress`,
  ], { encoding: 'utf8', windowsHide: true }))
  const cpuBefore = cpuCounter()
  await sleep(2000)
  const cpuAfter = cpuCounter()
  const cpuNative = (cpuAfter.cpu - cpuBefore.cpu) / ((cpuAfter.time - cpuBefore.time) / 1000) / os.availableParallelism() * 100
  const cpuSample = (await samples()).find((sample) => sample.instanceId === 'proc-a')
  record('cpu-native-counter-comparison', { nativePercent: cpuNative, sampledPercent: cpuSample.cpuPercent, logicalCpus: os.availableParallelism() })
  await button('启动 进程验收 B')
  await waitFor(async () => (await rowText('proc-b')).includes('运行中'), 'second process')
  await waitFor(async () => (await rowText('proc-a')).includes('%') && (await rowText('proc-b')).includes('%'), 'both CPU readings visible')
  record('two-live-processes', (await samples()).map((sample) => ({ instanceId: sample.instanceId, pid: sample.pid, port: sample.port })))
  await shot('windows-processes')
  await button('重启 进程验收 A')
  await waitFor(async () => (await samples()).some((sample) => sample.instanceId === 'proc-a' && (sample.pid !== a.pid || sample.startedAtMs !== a.startedAtMs) && sample.status === 'running'), 'restart')
  await waitFor(() => firstFamily.every(gone), 'old process tree stopped')
  record('restart-stops-old-parent-and-child', firstFamily)
  await waitFor(async () => {
    const text = await rowText('proc-a')
    return text.includes('运行中') && !text.includes('重启中')
  }, 'restarted UI ready')
  const restarted = family('proc-a')
  await button('停止 进程验收 A')
  await waitFor(() => restarted.every(gone), 'single stop tree')
  record('single-stop-removes-parent-and-child', restarted)
  await button('启动 进程验收 异常退出')
  await waitFor(async () => (await rowText('proc-crash')).includes('退出码 7'), 'abnormal exit')
  record('abnormal-exit-visible', await rowText('proc-crash'))
  await button('日志 进程验收 异常退出')
  await waitFor(() => ipc.some((call) => call.command === 'reveal_path'), 'log directory IPC')
  const logCall = ipc.filter((call) => call.command === 'reveal_path').at(-1)
  const logPath = JSON.parse(logCall.body).path
  assert(fs.statSync(logPath).isDirectory())
  record('log-directory-entry', logPath)
  await button('启动 进程验收 端口冲突')
  await waitFor(async () => (await rowText('proc-port')).includes('端口') && (await rowText('proc-port')).includes('启动失败'), 'port failure')
  record('port-conflict-visible', await rowText('proc-port'))
  await button('启动 进程验收 A')
  await waitFor(async () => (await rowText('proc-a')).includes('运行中'), 'restart for stop-all')
  const bothFamilies = [...family('proc-a'), ...family('proc-b')]
  await filter('进程验收 A')
  await stopAll()
  await waitFor(() => bothFamilies.every(gone), 'all trees stopped')
  record('stop-all-includes-filtered-out-instance', bothFamilies)
  await filter('进程验收')
  await button('启动 进程验收 慢启动')
  await waitFor(async () => (await rowText('proc-slow')).includes('启动中'), 'slow launch')
  const slowFamily = await waitFor(() => { const found = family('proc-slow'); return found.length === 2 && found }, 'slow process tree')
  await stopAll()
  await waitFor(() => slowFamily.every(gone), 'cancelled launch tree stopped')
  record('stop-all-cancels-starting-instance', slowFamily)
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 780, height: 520, deviceScaleFactor: 1, mobile: false })
  const narrow = await cdp.evaluate(`(() => {
    const area=document.querySelector('table').parentElement.getBoundingClientRect();
    const action=document.querySelector('tbody tr td:last-child').getBoundingClientRect();
    return {viewport:innerWidth,areaRight:area.right,actionRight:action.right};
  })()`)
  assert(narrow.actionRight <= narrow.areaRight + 1 && narrow.actionRight <= 780)
  record('minimum-viewport-actions-visible', narrow)
  await shot('windows-processes-780x520')
  await cdp.send('Emulation.clearDeviceMetricsOverride')
  assert.equal(report.errors.length, 0)
} catch (error) {
  report.error = String(error.stack ?? error)
  if (cdp) report.pageAtFailure = await cdp.bodyText().catch(() => '')
  if (cdp) report.samplesAtFailure = await samples().catch(() => [])
  console.error(report.error)
  if (cdp) await shot('windows-processes-failure').catch(() => {})
  process.exitCode = 1
} finally {
  if (cdp) {
    for (const instance of special) {
      await cdp.evaluate(`window.__TAURI_INTERNALS__.invoke('stop_instance',{instanceId:${JSON.stringify(instance.id)}})`, { awaitPromise: true }).catch(() => {})
    }
    await cdp.evaluate('void window.__TAURI_INTERNALS__.invoke("exit_app")').catch(() => {})
    cdp.close()
  }
  if (child) await Promise.race([new Promise((resolve) => child.once('exit', resolve)), sleep(5000)])
  child?.kill()
  preview?.kill()
  try {
    // Close only the Explorer window this lane opened for its own log folder.
    execFileSync('powershell.exe', ['-NoProfile', '-Command',
      '$target=$env:PHL_TEST_LOG_FOLDER; $shell=New-Object -ComObject Shell.Application; foreach($window in $shell.Windows()) { try { if(([Uri]$window.LocationURL).LocalPath.TrimEnd("\\") -eq $target.TrimEnd("\\")) { $window.Quit() } } catch {} }',
    ], { windowsHide: true, env: { ...process.env, PHL_TEST_LOG_FOLDER: path.join(root, 'instances/proc-crash/logs') } })
  } catch (error) { report.logWindowCleanupError = error.message }
  report.remainingPids = [...pids].filter((pid) => !gone(pid))
  report.finishedAt = new Date().toISOString()
  fs.writeFileSync(path.join(out, 'windows-process-report.json'), JSON.stringify(report, null, 2))
  if (!keep && report.remainingPids.length === 0) {
    const resolved = path.resolve(scratch)
    if (!resolved.startsWith(path.resolve(repo, '.tmp') + path.sep)) throw new Error('Scratch cleanup escaped the workspace')
    try { fs.rmSync(resolved, { recursive: true, force: true, maxRetries: 5, retryDelay: 250 }) }
    catch (error) { console.error('Scratch cleanup failed', resolved, error.message) }
  }
}
