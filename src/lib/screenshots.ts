/**
 * Screenshot discovery from a plugin's GitHub README.
 *
 * The registry's `screenshots` field only exists when an author filled it
 * in, but virtually every plugin that *has* demo images embeds them in its
 * README. `raw.githubusercontent.com/{repo}/HEAD/...` resolves the default
 * branch without knowing it, sends `Access-Control-Allow-Origin: *`, and
 * needs no API token — so one fetch per detail view is all it takes.
 * Results are memoized per repo for the session.
 */

const cache = new Map<string, string[]>()

const BADGE_RE =
  /shields\.io|badge|travis|circleci|codecov|coveralls|opencollective|snyk|discord|npmjs\.com|img\.githubusercontent\.com.*\/[0-9a-f]{40}[?)]/i

/** Real screenshots: raster images, or URLs that name themselves as demos. */
const SHOT_RE = /\.(png|jpe?g|gif|webp|avif)(\?|$)|screenshot|demo|preview|banner|cover/i

function toAbsolute(repo: string, url: string): string | null {
  const trimmed = url.trim().replace(/^\.\//, '')
  if (!trimmed || trimmed.startsWith('data:')) return null
  if (/^https?:\/\//i.test(trimmed)) return trimmed
  if (/^https?:$|^[a-z]+:/i.test(trimmed)) return null // mailto:, other schemes
  const path = trimmed.replace(/^\//, '')
  return `https://raw.githubusercontent.com/${repo}/HEAD/${path}`
}

/** Pulls the demo images embedded in the repo's README (default branch). */
export async function fetchRepoScreenshots(repo: string, limit = 6): Promise<string[]> {
  const key = repo.trim()
  if (!/^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/.test(key)) return []
  const cached = cache.get(key)
  if (cached) return cached

  let urls: string[] = []
  try {
    const response = await fetch(
      `https://raw.githubusercontent.com/${key}/HEAD/README.md`,
      { signal: AbortSignal.timeout(12_000) },
    )
    if (response.ok) {
      const readme = await response.text()
      const found = new Set<string>()
      for (const match of readme.matchAll(/!\[[^\]]*\]\(\s*([^\s)]+)/g)) {
        const abs = toAbsolute(key, match[1])
        if (abs) found.add(abs)
      }
      for (const match of readme.matchAll(/<img[^>]+src=["']([^"']+)["']/gi)) {
        const abs = toAbsolute(key, match[1])
        if (abs) found.add(abs)
      }
      urls = [...found]
        .filter((u) => !BADGE_RE.test(u) && SHOT_RE.test(u))
        .slice(0, limit)
    }
  } catch {
    // Offline / blocked CDN — the caller just shows no gallery.
  }
  cache.set(key, urls)
  return urls
}
