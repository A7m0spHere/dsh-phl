import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const runner = fileURLToPath(new URL('./alpha-desktop.mjs', import.meta.url))

test('desktop runner preserves supplied data and reports, and cleans only its own root', () => {
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'phl-runner-test-'))
  try {
    const bin = path.join(fixture, 'bin')
    const temp = path.join(fixture, 'temp')
    const supplied = path.join(fixture, 'supplied')
    for (const dir of [bin, temp, supplied]) fs.mkdirSync(dir)
    // Exercise the real runner without launching Tauri or accessing user data.
    const npm = path.join(bin, process.platform === 'win32' ? 'npm.cmd' : 'npm')
    fs.writeFileSync(npm, process.platform === 'win32' ? '@echo off\r\nexit /b 0\r\n' : '#!/bin/sh\nexit 0\n', { mode: 0o755 })
    fs.writeFileSync(path.join(supplied, 'sentinel'), 'keep me')
    const env = { ...process.env }
    for (const key of Object.keys(env)) {
      if (key.toLowerCase() === 'path' || key.startsWith('PHL_ALPHA_')) delete env[key]
    }
    Object.assign(env, { PATH: `${bin}${path.delimiter}${process.env.PATH}`, TEMP: temp, TMP: temp, TMPDIR: temp })
    const run = (extra = {}) => {
      const result = spawnSync(process.execPath, [runner], { env: { ...env, ...extra }, encoding: 'utf8', timeout: 60000 })
      assert.equal(result.status, 0, `${result.error ?? ''}\n${result.stdout}\n${result.stderr}`)
    }
    run({ PHL_ALPHA_ROOT: supplied })
    assert.equal(fs.readFileSync(path.join(supplied, 'sentinel'), 'utf8'), 'keep me')
    const suppliedReports = fs.readdirSync(fixture).filter((name) => name.endsWith('-report.json'))
    assert.equal(suppliedReports.length, 1)
    run()
    const reports = fs.readdirSync(temp)
    assert.equal(reports.length, 1, 'only the report survives automatic cleanup')
    assert.ok(reports[0].endsWith('-report.json'))
    const report = JSON.parse(fs.readFileSync(path.join(temp, reports[0]), 'utf8'))
    assert.ok(!fs.existsSync(report.root), 'automatically allocated root was removed')
  } finally {
    fs.rmSync(fixture, { recursive: true, force: true })
  }
})
