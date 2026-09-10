import { formatBytes } from '@/lib/format'
import { isDesktop } from '@/lib/desktop'
import { appVersion } from '@/lib/desktopUpdate'
import { useSettingsStore, useUpdateStore } from '@/stores'
import { Button, ProgressBar, SettingRow, Switch } from '@/components/ui'
import { PageSection } from '@/components/layout/Page'

/**
 * 设置 → 关于 的应用更新面板.
 *
 * The manifest is signed, so a mirror or proxy cannot substitute a different
 * build - but nothing is fetched until the user asks, and nothing is installed
 * until they click again. The startup check (settings.autoUpdateCheck) only
 * reads the manifest and toasts when a newer version exists.
 */
export function UpdateSection() {
  const autoCheck = useSettingsStore((s) => s.autoUpdateCheck)
  const setSetting = useSettingsStore((s) => s.set)
  const status = useUpdateStore((s) => s.status)
  const info = useUpdateStore((s) => s.info)
  const error = useUpdateStore((s) => s.error)
  const downloaded = useUpdateStore((s) => s.downloaded)
  const total = useUpdateStore((s) => s.total)
  const checkedAt = useUpdateStore((s) => s.checkedAt)
  const check = useUpdateStore((s) => s.check)
  const install = useUpdateStore((s) => s.install)

  const current = appVersion()
  const busy = status === 'checking' || status === 'downloading' || status === 'installing'

  const statusText = () => {
    if (!isDesktop) return '浏览器模式下没有更新器，桌面端才会检查。'
    if (status === 'checking') return '正在检查…'
    if (status === 'available' && info)
      return '发现新版本 ' + info.version + '（当前 ' + info.currentVersion + '）'
    if (status === 'downloading')
      return total
        ? '正在下载 ' + formatBytes(downloaded) + ' / ' + formatBytes(total)
        : '正在下载 ' + formatBytes(downloaded)
    if (status === 'installing') return '正在安装，应用会自动重启。'
    if (status === 'error') return '检查失败：' + (error ?? '未知错误')
    if (status === 'current')
      return '已是最新版本' + (checkedAt ? '（' + timeOf(checkedAt) + ' 检查）' : '')
    return checkedAt ? '上次检查：' + timeOf(checkedAt) : '还没有检查过。'
  }

  return (
    <PageSection
      title="应用更新"
      description="从 GitHub 上的签名清单检查新版本；清单经过签名校验，只有你点了「下载并安装」才会下载。"
    >
      <div className="rounded-lg bg-surface p-4 ring-1 ring-inset ring-line">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="text-base text-ink">
              当前版本 <span className="num">{current || '未知'}</span>
            </div>
            <div className="mt-1 text-sm leading-relaxed text-ink-muted">{statusText()}</div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {status === 'available' ? (
              <Button size="sm" variant="primary" onClick={() => void install()}>
                下载并安装
              </Button>
            ) : (
              <Button size="sm" variant="secondary" disabled={busy || !isDesktop} onClick={() => void check()}>
                {status === 'checking' ? '检查中…' : '检查更新'}
              </Button>
            )}
          </div>
        </div>

        {(status === 'downloading' || status === 'installing') && (
          <div className="mt-3">
            <ProgressBar
              value={total ? downloaded / total : 0}
              active={!total || status === 'installing'}
              height={4}
            />
          </div>
        )}

        {status === 'available' && info?.notes && (
          <details className="mt-3 rounded-lg bg-surface-sunken p-3 text-sm text-ink-muted ring-1 ring-inset ring-line">
            <summary className="cursor-pointer select-none text-ink">这一版更新了什么</summary>
            <div className="mt-2 whitespace-pre-wrap leading-relaxed">{info.notes}</div>
          </details>
        )}
      </div>

      <div className="mt-2 rounded-lg bg-surface px-4 ring-1 ring-inset ring-line">
        <SettingRow
          title="启动后自动检查更新"
          description="只读取签名清单，不会自动下载或安装"
          control={
            <Switch
              checked={autoCheck}
              onChange={(v) => setSetting('autoUpdateCheck', v)}
              label="启动后自动检查更新"
            />
          }
        />
      </div>
    </PageSection>
  )
}

function timeOf(ms: number) {
  return new Date(ms).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
}
