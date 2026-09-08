#!/usr/bin/env node
/**
 * One version, four files — checked before anything is published.
 *
 *   node scripts/check-versions.mjs [--tag v0.1.0-alpha.1]
 *
 * PHL's version lives in four places that must never disagree:
 * package.json (frontend + the version the About panel shows),
 * src-tauri/tauri.conf.json (the installer's file name and product version),
 * src-tauri/Cargo.toml (the crate) and src-tauri/Cargo.lock (what the build
 * actually resolves). A half-applied bump ships an installer whose filename,
 * about dialog and crate version tell three different stories — the release
 * workflow catches that, but only at tag time, after the build is paid for.
 * This runs in the gate instead, and the release workflow calls it too, so
 * both use one definition of "consistent".
 *
 * The tag, when given (via --tag or PHL_TAG), must be "v" + the version: a
 * mismatched tag would publish an installer that disagrees with its own name.
 * Alpha candidates are marked by a semver prerelease suffix ("-alpha.1"),
 * which the release workflow turns into a GitHub prerelease.
 */
import { readFileSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

const readJson = (rel) => JSON.parse(readFileSync(path.join(repoRoot, rel), 'utf8'))

/** The `version` of the `[package]` table — not a dependency's. */
function packageVersionFromCargoToml(rel) {
  const text = readFileSync(path.join(repoRoot, rel), 'utf8')
  const lines = text.split(/\r?\n/)
  let inPackage = false
  for (const line of lines) {
    const section = /^\s*\[([^\]]+)\]\s*$/.exec(line)
    if (section) {
      inPackage = section[1].trim() === 'package'
      continue
    }
    if (!inPackage) continue
    const match = /^\s*version\s*=\s*"([^"]+)"\s*$/.exec(line)
    if (match) return match[1]
  }
  return null
}

/** The lock entry for one workspace member, by exact name. */
function packageVersionFromCargoLock(rel, name) {
  const text = readFileSync(path.join(repoRoot, rel), 'utf8')
  for (const block of text.split(/\r?\n\[\[package\]\]\r?\n/)) {
    const nameMatch = /^name = "([^"]+)"/m.exec(block)
    if (!nameMatch || nameMatch[1] !== name) continue
    const versionMatch = /^version = "([^"]+)"/m.exec(block)
    if (versionMatch) return versionMatch[1]
  }
  return null
}

const tagArgIndex = process.argv.indexOf('--tag')
const tag =
  (tagArgIndex > -1 ? process.argv[tagArgIndex + 1] : undefined) ||
  process.env.PHL_TAG ||
  ''

const sources = [
  { file: 'package.json', version: readJson('package.json').version },
  { file: 'src-tauri/tauri.conf.json', version: readJson('src-tauri/tauri.conf.json').version },
  { file: 'src-tauri/Cargo.toml', version: packageVersionFromCargoToml('src-tauri/Cargo.toml') },
  {
    file: 'src-tauri/Cargo.lock',
    version: packageVersionFromCargoLock('src-tauri/Cargo.lock', 'dsh-phl'),
  },
]

const missing = sources.filter((s) => !s.version)
const version = sources[0].version
const disagree = sources.filter((s) => s.version && s.version !== version)
const problems = []
if (missing.length) {
  problems.push(`could not read a version from: ${missing.map((s) => s.file).join(', ')}`)
}
if (disagree.length) {
  problems.push(
    `version mismatch — ${sources.map((s) => `${s.file}=${s.version ?? '?'}`).join(' ')}`,
  )
}
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version ?? '')) {
  problems.push(`"${version}" is not a semver version (x.y.z or x.y.z-prerelease)`)
}
if (tag && tag !== `v${version}`) {
  problems.push(`tag ${tag} does not match app version v${version}`)
}

if (problems.length) {
  for (const p of problems) process.stderr.write(`VERSION CHECK FAILED: ${p}\n`)
  process.exit(1)
}
const channel = version.includes('-') ? 'prerelease' : 'release'
process.stdout.write(
  `VERSION OK: ${version} (${channel}) agreed by ${sources.map((s) => s.file).join(', ')}` +
    (tag ? ` — tag ${tag}` : '') +
    '\n',
)
