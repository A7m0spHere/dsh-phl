import type { DshVersion } from '@/types'
import { detectPendingChanges, loadPendingSet, savePendingSet } from '@/lib/pendingReleases'
import { useSettingsStore } from './settingsStore'
import { useUIStore } from './uiStore'

/**
 * Observe GitHub-only releases that have become installable. The caller
 * supplies the action so this policy module does not import the store it helps
 * compose (and therefore cannot create a circular dependency).
 */
export function observePendingReleases(
  versions: DshVersion[],
  installVersion: (id: string) => Promise<void>,
): void {
  try {
    const { published, next } = detectPendingChanges(loadPendingSet(), versions)
    savePendingSet(next)
    if (!published.length || !useSettingsStore.getState().pendingReleaseAlerts) return
    const first = published[0]
    useUIStore.getState().toast({
      kind: 'success',
      title: published.length === 1 ? `新版本已可安装：${first}` : `${published.length} 个版本已可安装`,
      message: '此前仅在 GitHub 发布，现已上架安装包源。',
      action: { label: '安装', run: () => void installVersion(`dsh-${first}`) },
    })
  } catch (err) {
    console.warn('[phl] pending-release check failed:', err)
  }
}

const OFFICIAL_HINT_MIN_AVERAGE = 256 * 1024
let mirrorHintShown = false

/** Show the official-registry slowdown hint at most once per app session. */
export function maybeHintMirror(slow: () => boolean): void {
  if (mirrorHintShown || useSettingsStore.getState().source !== 'official' || !slow()) return
  mirrorHintShown = true
  useUIStore.getState().toast({
    kind: 'info',
    title: '官方源速度较慢，试试镜像？',
    message:
      '检测到从 npm 官方源下载较慢。切换到 npmmirror 镜像通常能显著提速；切换后取消当前安装、重新安装即可生效。',
    duration: 12000,
    action: {
      label: '切换到镜像',
      run: () => {
        useSettingsStore.getState().set('source', 'mirror-cn')
        useUIStore
          .getState()
          .toast({ kind: 'success', title: '已切换到 npmmirror 镜像', message: '取消当前安装后重新安装即可生效。' })
      },
    },
  })
}

export function officialRegistryLooksSlow(startedAt: number, bytesDone: number): boolean {
  const elapsed = Date.now() - startedAt
  return elapsed > 15_000 && bytesDone / Math.max(elapsed, 1) < OFFICIAL_HINT_MIN_AVERAGE
}
