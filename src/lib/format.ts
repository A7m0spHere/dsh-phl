export function formatBytes(bytes: number, digits = 1): string {
  if (!bytes) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  const value = bytes / Math.pow(1024, i)
  return `${value.toFixed(i === 0 ? 0 : value >= 100 ? 0 : digits)} ${units[i]}`
}

export function formatSpeed(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec, 1)}/s`
}

export function formatDuration(seconds: number): string {
  if (seconds < 60) return `${Math.floor(seconds)} 秒`
  const m = Math.floor(seconds / 60)
  if (m < 60) return `${m} 分钟`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h} 小时 ${m % 60} 分`
  const d = Math.floor(h / 24)
  return `${d} 天 ${h % 24} 小时`
}

/** Compact uptime for the status line: `01:24:09`. */
export function formatClock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds))
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`
}

const RELATIVE_STEPS: [number, string][] = [
  [60, '秒'],
  [3600, '分钟'],
  [86400, '小时'],
  [86400 * 30, '天'],
]

export function formatRelative(iso?: string | number): string {
  if (!iso) return '从未运行'
  const then = typeof iso === 'number' ? iso : new Date(iso).getTime()
  const diff = (Date.now() - then) / 1000
  if (diff < 45) return '刚刚'
  if (diff < 60) return '不到 1 分钟前'
  for (let i = 0; i < RELATIVE_STEPS.length; i++) {
    const [limit, unit] = RELATIVE_STEPS[i]
    const next = RELATIVE_STEPS[i + 1]
    if (!next || diff < next[0]) {
      return `${Math.floor(diff / limit)} ${unit}前`
    }
  }
  return new Date(then).toLocaleDateString('zh-CN')
}

export function formatDate(iso: string): string {
  return new Date(iso).toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  })
}

export function formatDateTime(iso: string | number): string {
  return new Date(iso).toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function formatCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`
  return String(n)
}

/** `C:\a\b\c` → `…\b\c` for tight columns. */
export function shortenPath(path: string, segments = 2): string {
  const parts = path.split(/[\\/]/)
  if (parts.length <= segments + 1) return path
  return `…\\${parts.slice(-segments).join('\\')}`
}

export function clamp(n: number, min: number, max: number) {
  return Math.min(max, Math.max(min, n))
}

export function slugify(name: string): string {
  const base = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9\u4e00-\u9fa5]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return base || 'instance'
}
