import { useEffect } from 'react'
import { AppWindow, FolderOpen, Power, RefreshCw, Rocket } from 'lucide-react'
import { Badge, Button } from '@/components/ui'
import { revealPath } from '@/lib/desktop'
import {
  startOfficialDesktopPolling,
  stopOfficialDesktopPolling,
  useOfficialDesktopStore,
} from '@/stores/officialDesktopStore'
import type { OfficialDesktopInfo } from '@/types'

/**
 * The 「官方桌面端」 singleton card on the instances page: one install per
 * machine, owned by its own installer/updater. PHL only ever *reads* it and
 * drives launch/quit — the card says so on every state, and a probe it
 * cannot trust reads 「无法确认」, never 「未运行」 (the difference between
 * an honest card and one that invites a second launch).
 */

/** The running halo, matching the instance list's ambient "this one is live". */
function RunningDot() {
  return (
    <span className="relative inline-flex h-1.5 w-1.5 items-center justify-center">
      <span className="absolute inset-0 animate-halo rounded-full bg-ok" aria-hidden />
      <span className="relative h-1.5 w-1.5 rounded-full bg-ok" />
    </span>
  )
}

function statusBadge(info: OfficialDesktopInfo | null) {
  if (info === null) {
    return <Badge tone="neutral">浏览器模式</Badge>
  }
  switch (info.status) {
    case 'installed':
      if (info.running === true) {
        return (
          <Badge tone="ok">
            <RunningDot />
            运行中
          </Badge>
        )
      }
      if (info.running === false) {
        return <Badge tone="neutral">未运行</Badge>
      }
      return <Badge tone="warn">状态未知</Badge>
    case 'not_installed':
      return <Badge tone="neutral">未安装</Badge>
    case 'unsupported':
      return <Badge tone="neutral">平台不支持</Badge>
    case 'unknown':
      return <Badge tone="warn">无法确认</Badge>
  }
}

function footnote(info: OfficialDesktopInfo | null, error: string | null, loaded: boolean) {
  if (!loaded) return '正在检测官方桌面端…'
  if (info === null) return '浏览器模式不提供官方桌面端管理，请在 PHL 桌面应用中使用。'
  switch (info.status) {
    case 'installed':
      return [info.root, '由官方安装与更新 · PHL 只读管理'].filter(Boolean).join(' · ')
    case 'not_installed':
      return '未在本机发现官方 DeepSeek Harness 桌面端；安装后 PHL 会自动识别。'
    case 'unsupported':
      return '官方桌面端仅提供 Windows 与 macOS 版本。'
    case 'unknown':
      return error ?? '无法确认官方桌面端状态，请稍后重试。'
  }
}

export function OfficialDesktopCard() {
  const info = useOfficialDesktopStore((s) => s.info)
  const loaded = useOfficialDesktopStore((s) => s.loaded)
  const loading = useOfficialDesktopStore((s) => s.loading)
  const launching = useOfficialDesktopStore((s) => s.launching)
  const quitting = useOfficialDesktopStore((s) => s.quitting)
  const error = useOfficialDesktopStore((s) => s.error)
  const refresh = useOfficialDesktopStore((s) => s.refresh)
  const launch = useOfficialDesktopStore((s) => s.launch)
  const requestQuit = useOfficialDesktopStore((s) => s.requestQuit)

  // Poll only while the card is on screen; the loop also stops itself once
  // there is nothing left to watch (not installed / unsupported).
  useEffect(() => {
    startOfficialDesktopPolling()
    return () => stopOfficialDesktopPolling()
  }, [])

  const installed = info?.status === 'installed'
  const canDrive =
    installed && (info.running === true || info.running === false)

  return (
    <div className="mb-2.5 rounded-lg bg-surface px-3 py-2.5 ring-1 ring-inset ring-line">
      <div className="flex min-h-[28px] flex-wrap items-center gap-2">
        <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-sm bg-surface-sunken text-ink-faint">
          <AppWindow size={13} />
        </span>
        <span className="text-md font-medium text-ink">官方桌面端</span>
        {installed && info.version && <Badge tone="outline">v{info.version}</Badge>}
        {statusBadge(info)}
        <div className="ml-auto flex items-center gap-1.5">
          {canDrive && info.running === false && (
            <Button variant="primary" size="sm" loading={launching} onClick={() => void launch()}>
              <Rocket size={13} />
              启动
            </Button>
          )}
          {canDrive && info.running === true && (
            <Button variant="secondary" size="sm" loading={quitting} onClick={() => void requestQuit()}>
              <Power size={13} />
              请求退出
            </Button>
          )}
          {installed && info.root && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                if (info.root) void revealPath(info.root)
              }}
            >
              <FolderOpen size={13} />
              打开安装目录
            </Button>
          )}
          {loaded && info !== null && info.status !== 'unsupported' && (
            <Button variant="secondary" size="sm" loading={loading} onClick={() => void refresh()}>
              <RefreshCw size={13} />
              重新检测
            </Button>
          )}
        </div>
      </div>
      <p className="mt-1 truncate text-2xs text-ink-faint">{footnote(info, error, loaded)}</p>
    </div>
  )
}
