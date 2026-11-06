import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  UPDATE_MANIFEST_URL,
  compareVersions,
  isNewerVersion,
  parseUpdateManifest,
  releaseUrl,
} from './updateManifest'

/**
 * The manual update path (non-Windows) reads the same manifest the Windows
 * updater plugin uses. Two things are worth pinning here: prerelease order
 * (`0.1.0-rc.1` < `0.1.1-alpha.2`, `alpha.1` < `alpha.2`) and the fact that the
 * frontend constant cannot drift from `tauri.conf.json`.
 */

describe('compareVersions', () => {
  it('orders releases by major/minor/patch', () => {
    expect(compareVersions('0.1.1', '0.1.0')).toBe(1)
    expect(compareVersions('0.1.0', '0.1.1')).toBe(-1)
    expect(compareVersions('0.1.1', '0.1.1')).toBe(0)
    expect(compareVersions('v0.1.1', '0.1.1')).toBe(0)
    expect(compareVersions('1.0.0', '0.9.9')).toBe(1)
  })

  it('orders prereleases below their release', () => {
    expect(compareVersions('0.1.0-rc.1', '0.1.1-alpha.2')).toBe(-1)
    expect(compareVersions('0.1.1-alpha.2', '0.1.1')).toBe(-1)
    expect(compareVersions('0.1.1', '0.1.1-alpha.2')).toBe(1)
  })

  it('orders prerelease identifiers numerically, then lexically', () => {
    expect(compareVersions('0.1.1-alpha.2', '0.1.1-alpha.1')).toBe(1)
    expect(compareVersions('0.1.1-alpha.10', '0.1.1-alpha.9')).toBe(1)
    expect(compareVersions('1.0.0-alpha', '1.0.0-beta')).toBe(-1)
    // Numeric identifiers have lower precedence than alphanumeric ones.
    expect(compareVersions('1.0.0-1', '1.0.0-alpha')).toBe(-1)
    // A shorter identifier list is lower when it is a prefix of the other.
    expect(compareVersions('1.0.0-alpha', '1.0.0-alpha.1')).toBe(-1)
  })

  it('rejects versions it cannot parse', () => {
    expect(() => compareVersions('latest', '0.1.0')).toThrow('无法识别的版本号')
  })
})

describe('isNewerVersion', () => {
  it('decides the "有新版本" question', () => {
    expect(isNewerVersion('0.1.1-alpha.2', '0.1.0-rc.1')).toBe(true)
    expect(isNewerVersion('0.1.1-alpha.2', '0.1.1-alpha.2')).toBe(false)
    expect(isNewerVersion('0.1.0', '0.1.1')).toBe(false)
  })
})

describe('parseUpdateManifest', () => {
  it('reads the fields PHL uses and ignores the rest', () => {
    const manifest = parseUpdateManifest({
      version: '0.1.1-alpha.2',
      notes: '# PHL 0.1.1-alpha.2',
      pub_date: '2026-10-23T13:58:19Z',
      platforms: { 'windows-x86_64': { url: 'https://example.invalid/x', signature: 'sig' } },
    })
    expect(manifest).toEqual({
      version: '0.1.1-alpha.2',
      notes: '# PHL 0.1.1-alpha.2',
      date: '2026-10-23T13:58:19Z',
    })
  })

  it('rejects a missing or unparseable version', () => {
    expect(() => parseUpdateManifest({})).toThrow('更新清单缺少版本号')
    expect(() => parseUpdateManifest({ version: 'soon' })).toThrow('无法识别的版本号')
    expect(() => parseUpdateManifest(null)).toThrow('更新清单格式无法识别')
  })
})

describe('releaseUrl', () => {
  it('points at the tagged release when a version is known', () => {
    expect(releaseUrl('0.1.1-alpha.2')).toBe(
      'https://github.com/A7m0spHere/dsh-phl/releases/tag/v0.1.1-alpha.2',
    )
    expect(releaseUrl()).toBe('https://github.com/A7m0spHere/dsh-phl/releases')
  })
})

describe('UPDATE_MANIFEST_URL', () => {
  it('matches the updater endpoint the Windows plugin reads', () => {
    const conf = JSON.parse(
      readFileSync(
        fileURLToPath(new URL('../../src-tauri/tauri.conf.json', import.meta.url)),
        'utf8',
      ),
    ) as { plugins?: { updater?: { endpoints?: string[] } } }
    expect(conf.plugins?.updater?.endpoints).toContain(UPDATE_MANIFEST_URL)
  })
})
