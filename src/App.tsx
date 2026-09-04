import { useEffect } from 'react'
import { MotionConfig } from 'motion/react'
import { desktop } from '@/lib/desktop'
import { useHotkeys } from '@/lib/hooks'
import {
  useApiConfigStore,
  useCatalogStore,
  useInstanceStore,
  useSettingsStore,
  useUIStore,
  useWizardStore,
  initDesktopRoot,
  maybeOfferRootChoice,
} from '@/stores'
import { DialogHost, Toaster } from '@/components/ui'
import { TitleBar } from '@/components/layout/TitleBar'
import { Router } from '@/components/layout/Router'
import { QuickSwitcher } from '@/components/layout/QuickSwitcher'
import { GuideOverlay } from '@/components/layout/GuideOverlay'

export default function App() {
  const loadInstances = useInstanceStore((s) => s.load)
  const loadCatalog = useCatalogStore((s) => s.load)
  const loadApiConfig = useApiConfigStore((s) => s.load)
  const navigate = useUIStore((s) => s.navigate)
  const back = useUIStore((s) => s.back)
  const setPaletteOpen = useUIStore((s) => s.setPaletteOpen)
  const paletteOpen = useUIStore((s) => s.paletteOpen)
  const dialogOpen = useUIStore((s) => s.dialog !== null)
  const syncSystemTheme = useUIStore((s) => s.syncSystemTheme)
  const motionLevel = useUIStore((s) => s.motion)

  useEffect(() => {
    // The real data root must be known before the catalog queries the
    // filesystem for installed versions. Both loaders can now genuinely fail
    // (they reach the disk), so the chain needs a handler — an unhandled
    // rejection here left the pages on their skeletons with no explanation.
    void initDesktopRoot()
      .then(() => Promise.all([loadCatalog(), loadInstances(), loadApiConfig()]))
      .catch((err) => {
        console.error('[phl] startup load failed:', err)
        useUIStore.getState().toast({
          kind: 'error',
          title: '加载失败',
          message: err instanceof Error && err.message ? err.message : String(err),
          action: {
            label: '重试',
            run: () => {
              void loadCatalog()
              void loadInstances()
              void loadApiConfig()
            },
          },
        })
      })
  }, [loadCatalog, loadInstances, loadApiConfig])

  // Version catalog auto-sync. Deliberately cheap: one coarse timer that only
  // compares clocks on each tick, and no-ops while the window is hidden — so
  // a tray-minimized PHL does zero background work. The interval is user-
  // configurable (0 = off) precisely because of the overhead concern.
  const versionRefreshMinutes = useSettingsStore((s) => s.versionRefreshMinutes)
  useEffect(() => {
    if (!versionRefreshMinutes) return
    const id = setInterval(() => {
      if (document.hidden) return
      const cat = useCatalogStore.getState()
      if (Date.now() - cat.versionsSyncAttemptedAt >= versionRefreshMinutes * 60_000) {
        void cat.refreshVersions({ silent: true })
      }
    }, 15_000)
    return () => clearInterval(id)
  }, [versionRefreshMinutes])

  // The Tauri window starts hidden so the user never sees an unpainted frame.
  // Reveal it after the first paint, once the theme class is already applied.
  useEffect(() => {
    const raf = requestAnimationFrame(() => void desktop.ready())
    return () => cancelAnimationFrame(raf)
  }, [])

  // Closing the window is the app's decision, not the OS's: Rust hands the
  // request back so we can warn about instances that are still running.
  useEffect(() => {
    let unlisten: (() => void) | undefined
    let disposed = false

    void desktop
      .onCloseRequested(async () => {
        const ui = useUIStore.getState()
        const instances = useInstanceStore.getState()
        const stopOnExit = useSettingsStore.getState().closeStopsInstances
        const running = instances.instances.filter(
          (i) => instances.states[i.id]?.status === 'running',
        )

        const ok = await ui.confirm({
          title: '退出 PHL',
          message: !running.length
            ? '确定要退出 PHL 吗？'
            : stopOnExit
              ? `有 ${running.length} 个实例正在运行，退出会一并停止它们的 DSH 进程。`
              : `有 ${running.length} 个实例正在运行。按当前设置，退出后它们的进程会被保留。`,
          detail: running.length ? running.map((i) => `${i.name}  :${i.port}`).join('\n') : undefined,
          confirmLabel: '退出',
          tone: running.length && stopOnExit ? 'danger' : 'default',
        })

        if (!ok) return

        // The dialog above promises this; without it the setting was inert and
        // exiting left orphaned node.exe processes holding their ports, so the
        // next launch of the same instance failed to bind.
        if (stopOnExit && running.length) {
          await Promise.all(
            running.map((i) =>
              instances.stop(i.id).catch((err) => {
                console.warn(`[phl] stop ${i.name} on exit failed:`, err)
              }),
            ),
          )
        }
        await desktop.exit()
      })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  // A desktop app should not offer the browser's page context menu. Text
  // selection stays available inside fields and monospace values.
  useEffect(() => {
    if (!desktop.isDesktop) return
    const onContextMenu = (e: MouseEvent) => {
      const el = e.target as HTMLElement | null
      const selectable =
        el?.closest('input, textarea, [contenteditable], code, pre, .font-mono') !== null
      if (!selectable) e.preventDefault()
    }
    document.addEventListener('contextmenu', onContextMenu)
    return () => document.removeEventListener('contextmenu', onContextMenu)
  }, [])

  // Follow the OS while the theme is set to "system".
  useEffect(() => {
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    const onChange = () => syncSystemTheme()
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [syncSystemTheme])

  // First run opens the guide. It is the answer to "what is this thing" —
  // an instance manager is not self-evident from a list of cards.
  const guideSeen = useSettingsStore((s) => s.guideSeen)
  useEffect(() => {
    if (useSettingsStore.getState().guideSeen) return
    const id = window.setTimeout(() => useUIStore.getState().setGuideOpen(true), 450)
    return () => window.clearTimeout(id)
  }, [])

  // The one-time storage-location prompt waits until the guide is out of the
  // way — two full-screen first-run surfaces at once would fight over focus.
  useEffect(() => {
    if (guideSeen) void maybeOfferRootChoice()
  }, [guideSeen])

  useHotkeys([
    { key: 'k', ctrl: true, global: true, run: () => setPaletteOpen(!paletteOpen) },
    {
      key: 'n',
      ctrl: true,
      run: () => {
        useWizardStore.getState().reset()
        navigate({ name: 'create' })
      },
    },
    {
      key: 'escape',
      run: () => {
        if (paletteOpen || dialogOpen) return
        back()
      },
    },
    { key: ',', ctrl: true, run: () => navigate({ name: 'settings' }) },
  ])

  return (
    <MotionConfig reducedMotion={motionLevel === 'off' ? 'always' : 'never'}>
      <div className="flex h-full flex-col overflow-hidden bg-canvas text-ink">
        <TitleBar />
        <Router />
      </div>
      <Toaster />
      <DialogHost />
      <QuickSwitcher />
      <GuideOverlay />
    </MotionConfig>
  )
}
