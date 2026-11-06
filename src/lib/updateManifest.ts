/**
 * The published update manifest, plus the version comparison the manual
 * update path needs.
 *
 * Windows keeps using `tauri-plugin-updater` end to end: it verifies the
 * manifest's minisign signature and downloads the signed NSIS installer.
 * macOS has no in-app updater chain yet — the release pipeline deliberately
 * ships the DMG as a plain release asset and keeps the manifest Windows-only
 * until signing, notarization and a macOS updater artifact exist (see
 * `.github/workflows/release.yml`). The plugin treats a platform that is
 * missing from `platforms` as an error *before* it decides whether an update
 * exists, so on those platforms it fails even when the app is already
 * current. PHL therefore reads the same manifest itself and only *notifies*;
 * the install step stays a manual download.
 */

/**
 * Must stay in sync with `plugins.updater.endpoints` in
 * `src-tauri/tauri.conf.json` — the Windows path reads that copy through the
 * plugin. `updateManifest.test.ts` pins the two together.
 */
export const UPDATE_MANIFEST_URL =
  'https://raw.githubusercontent.com/A7m0spHere/dsh-phl/updates/latest.json'

/** Release list; the version-specific page is preferred when we have one. */
const RELEASES_PAGE = 'https://github.com/A7m0spHere/dsh-phl/releases'

export interface UpdateManifest {
  version: string
  notes?: string
  /** `pub_date` in the manifest, kept as the raw RFC 3339 string. */
  date?: string
}

interface ParsedVersion {
  major: number
  minor: number
  patch: number
  prerelease: string[]
}

const VERSION = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/

function parseVersion(input: string): ParsedVersion {
  const match = VERSION.exec(input.trim())
  if (!match) throw new Error('无法识别的版本号：' + input)
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] ? match[4].split('.') : [],
  }
}

/**
 * Semver precedence (no build metadata), returning -1 / 0 / 1. Prerelease
 * ordering is the part that matters here: `0.1.0-rc.1` is older than
 * `0.1.1-alpha.2`, and `0.1.1-alpha.2` is newer than `0.1.1-alpha.1`.
 */
export function compareVersions(a: string, b: string): number {
  const left = parseVersion(a)
  const right = parseVersion(b)
  for (const part of ['major', 'minor', 'patch'] as const) {
    if (left[part] !== right[part]) return left[part] < right[part] ? -1 : 1
  }
  const hasLeftPre = left.prerelease.length > 0
  const hasRightPre = right.prerelease.length > 0
  if (!hasLeftPre && !hasRightPre) return 0
  // A release outranks any prerelease of the same version.
  if (!hasLeftPre) return 1
  if (!hasRightPre) return -1
  const length = Math.max(left.prerelease.length, right.prerelease.length)
  for (let i = 0; i < length; i += 1) {
    const l = left.prerelease[i]
    const r = right.prerelease[i]
    if (l === undefined) return -1
    if (r === undefined) return 1
    if (l === r) continue
    const lNumeric = /^\d+$/.test(l)
    const rNumeric = /^\d+$/.test(r)
    if (lNumeric && rNumeric) {
      const lNum = Number(l)
      const rNum = Number(r)
      if (lNum !== rNum) return lNum < rNum ? -1 : 1
      continue
    }
    // Numeric identifiers have lower precedence than alphanumeric ones.
    if (lNumeric) return -1
    if (rNumeric) return 1
    return l < r ? -1 : 1
  }
  return 0
}

export function isNewerVersion(candidate: string, current: string): boolean {
  return compareVersions(candidate, current) > 0
}

/**
 * Validate the fields PHL actually uses. `platforms` is intentionally not
 * required: this path never downloads, and the macOS entry is exactly what
 * does not exist yet.
 */
export function parseUpdateManifest(value: unknown): UpdateManifest {
  if (typeof value !== 'object' || value === null) {
    throw new Error('更新清单格式无法识别')
  }
  const raw = value as Record<string, unknown>
  const version = typeof raw.version === 'string' ? raw.version.trim() : ''
  if (!version) throw new Error('更新清单缺少版本号')
  parseVersion(version)
  return {
    version,
    notes: typeof raw.notes === 'string' ? raw.notes : undefined,
    date: typeof raw.pub_date === 'string' ? raw.pub_date : undefined,
  }
}

/** Release page for a version (`0.1.1-alpha.2` → tag `v0.1.1-alpha.2`). */
export function releaseUrl(version?: string): string {
  return version ? `${RELEASES_PAGE}/tag/v${encodeURIComponent(version)}` : RELEASES_PAGE
}
