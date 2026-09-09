import { useId } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import {
  ArrowLeft,
  ArrowRight,
  Blocks,
  Boxes,
  CircleHelp,
  Copy,
  Cpu,
  KeyRound,
  Minus,
  Moon,
  Package,
  Search,
  Settings,
  Square,
  Sun,
  X,
} from 'lucide-react'
import { cn } from '@/lib/cn'
import { desktop } from '@/lib/desktop'
import { useMaximized } from '@/lib/useMaximized'
import { useMotion } from '@/lib/motion'
import {
  routeTab,
  useInstanceStore,
  useUIStore,
  type Tab,
} from '@/stores'
import { IconButton, Kbd, Tooltip } from '@/components/ui'
import { Logo } from './Logo'
import { TaskCenter } from '@/features/tasks/TaskCenter'

const TABS: { id: Tab; label: string; icon: typeof Boxes }[] = [
  { id: 'instances', label: '实例', icon: Boxes },
  { id: 'versions', label: '版本', icon: Package },
  { id: 'plugins', label: '插件', icon: Blocks },
  { id: 'apiConfig', label: 'API', icon: KeyRound },
  { id: 'runtimes', label: '运行时', icon: Cpu },
  { id: 'settings', label: '设置', icon: Settings },
]

export function TitleBar() {
  const route = useUIStore((s) => s.route)
  const navigate = useUIStore((s) => s.navigate)
  const back = useUIStore((s) => s.back)
  const forward = useUIStore((s) => s.forward)
  const canGoBack = useUIStore((s) => s.navBack.length > 0)
  const canGoForward = useUIStore((s) => s.navForward.length > 0)
  const theme = useUIStore((s) => s.theme)
  const setTheme = useUIStore((s) => s.setTheme)
  const isDark = useUIStore((s) => s.isDark)
  const setPaletteOpen = useUIStore((s) => s.setPaletteOpen)
  const setGuideOpen = useUIStore((s) => s.setGuideOpen)
  const toast = useUIStore((s) => s.toast)
  const runningCount = useInstanceStore((s) =>
    s.instances.filter((i) => s.states[i.id]?.status === 'running').length,
  )
  const { t, spring, scale } = useMotion()
  const indicatorId = useId()
  const maximized = useMaximized()

  const activeTab = routeTab(route)

  // In the browser these controls have nothing to act on; say so plainly
  // rather than leaving a dead button.
  const unavailable = () =>
    toast({
      kind: 'info',
      title: '窗口控制仅在桌面端可用',
      message: '用 npm run app:dev 启动 Tauri 窗口。',
      duration: 2800,
    })

  return (
    // The tab labels drop away under 860px (see index.css): at a narrow window
    // or a 110/125% UI scale the six labels pushed the window controls past the
    // app's own overflow-hidden, which clipped 最小化/最大化/关闭 off the screen.
    <header
      data-tauri-drag-region
      className="drag relative z-30 flex shrink-0 select-none items-center gap-1 border-b border-line bg-chrome/90 px-2 backdrop-blur-xl"
      style={{ height: 'var(--titlebar-h)' }}
    >
      <div data-tauri-drag-region className="flex shrink-0 items-center gap-1.5 pl-1 pr-1.5">
        <Logo size={16} />
        <span className="select-none text-sm font-semibold tracking-tight text-ink">PHL</span>
        {/* Browser/mock mode looks identical to a real run (install progress,
            link-plugins, "running"), so the only honest signal is a persistent
            badge here — it's visible on every page, including the plugin detail
            overlay and the launch dock, not just on the pages that disable. */}
        {!desktop.isDesktop && (
          <Tooltip
            side="bottom"
            content={
              <span className="flex flex-col gap-1">
                <span>浏览器模式（<Kbd>npm run dev</Kbd>）：演示数据，不写盘、不拉起真实进程。</span>
                <span>安装/卸载插件、启动实例只会播放动画。桌面版请用 <Kbd>npm run app:dev</Kbd>。</span>
              </span>
            }
          >
            <span className="no-drag rounded-xs bg-warn/[0.12] px-1.5 py-0.5 text-2xs font-semibold text-warn ring-1 ring-inset ring-warn/25">
              浏览器模拟模式
            </span>
          </Tooltip>
        )}
      </div>

      {/* Back/forward sit between identity and navigation: they belong to the
          page history, not to the app. Fixed placeholders — greyed when there
          is nowhere to go — so the chevrons never jump and the hit target is
          always in the same spot. */}
      <div className="no-drag flex shrink-0 items-center">
        <Tooltip content={<span className="flex items-center gap-1">返回 <Kbd>Alt</Kbd><Kbd>←</Kbd></span>} side="bottom">
          <IconButton label="返回" size="sm" variant="ghost" disabled={!canGoBack} onClick={back}>
            <ArrowLeft size={14} />
          </IconButton>
        </Tooltip>
        <Tooltip content={<span className="flex items-center gap-1">前进 <Kbd>Alt</Kbd><Kbd>→</Kbd></span>} side="bottom">
          <IconButton label="前进" size="sm" variant="ghost" disabled={!canGoForward} onClick={forward}>
            <ArrowRight size={14} />
          </IconButton>
        </Tooltip>
      </div>

      <nav className="no-drag flex min-w-0 items-center gap-0.5">
        {TABS.map((tab) => {
          const active = tab.id === activeTab
          const Icon = tab.icon
          return (
            <button
              key={tab.id}
              onClick={() => navigate({ name: tab.id } as never)}
              className={cn(
                'relative flex h-[26px] shrink-0 items-center gap-1.5 rounded-sm px-2 text-sm font-medium transition-colors duration-150',
                active ? 'text-ink' : 'text-ink-faint hover:text-ink-muted',
              )}
            >
              {active && (
                <motion.span
                  layoutId={indicatorId}
                  transition={scale === 0 ? { duration: 0 } : spring}
                  className="absolute inset-0 rounded-sm bg-surface-hover"
                />
              )}
              <span className="relative z-10 flex items-center gap-1.5">
                <Icon size={13} strokeWidth={active ? 2.2 : 1.9} />
                {/* Icon-only when the row is too narrow: the label is a nicety,
                    the window controls are not. */}
                <span data-titlebar-label>{tab.label}</span>
              </span>
              {tab.id === 'instances' && runningCount > 0 && (
                <span className="relative z-10 ml-0.5 flex h-[15px] min-w-[15px] items-center justify-center rounded-full bg-ok/15 px-1 text-2xs font-semibold text-ok">
                  {runningCount}
                </span>
              )}
            </button>
          )
        })}
      </nav>

      <div data-tauri-drag-region className="ml-auto flex shrink-0 items-center gap-1 pr-1">
        <TaskCenter />

        <Tooltip
          content={<span className="flex items-center gap-1">快速跳转 <Kbd>Ctrl</Kbd><Kbd>K</Kbd></span>}
          side="bottom"
        >
          <IconButton
            label="快速跳转"
            size="sm"
            variant="ghost"
            className="no-drag"
            onClick={() => setPaletteOpen(true)}
          >
            <Search size={14} />
          </IconButton>
        </Tooltip>

        <Tooltip content="入门指引" side="bottom">
          <IconButton
            label="入门指引"
            size="sm"
            variant="ghost"
            className="no-drag"
            onClick={() => setGuideOpen(true)}
          >
            <CircleHelp size={13} />
          </IconButton>
        </Tooltip>

        <Tooltip content={isDark ? '切换到浅色' : '切换到深色'} side="bottom">
          <IconButton
            label="切换主题"
            size="sm"
            variant="ghost"
            className="no-drag"
            onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}
          >
            <AnimatePresence mode="wait" initial={false}>
              <motion.span
                key={isDark ? 'moon' : 'sun'}
                initial={{ opacity: 0, rotate: -60, scale: 0.7 }}
                animate={{ opacity: 1, rotate: 0, scale: 1 }}
                exit={{ opacity: 0, rotate: 60, scale: 0.7 }}
                transition={t(0.2)}
                className="flex"
              >
                {isDark ? <Moon size={14} /> : <Sun size={14} />}
              </motion.span>
            </AnimatePresence>
          </IconButton>
        </Tooltip>

        <div className="no-drag ml-1 flex shrink-0 items-center self-stretch">
          <button
            aria-label="最小化"
            onClick={() => (desktop.isDesktop ? void desktop.minimize() : unavailable())}
            className="flex h-full w-10 items-center justify-center text-ink-faint transition-colors hover:bg-surface-hover hover:text-ink"
          >
            <Minus size={13} />
          </button>
          <button
            aria-label={maximized ? '向下还原' : '最大化'}
            onClick={() => (desktop.isDesktop ? void desktop.toggleMaximize() : unavailable())}
            className="flex h-full w-10 items-center justify-center text-ink-faint transition-colors hover:bg-surface-hover hover:text-ink"
          >
            {/* Two offset squares read as "restore" the way the OS draws it. */}
            {maximized ? <Copy size={11} className="-scale-x-100" /> : <Square size={10.5} />}
          </button>
          <button
            aria-label="关闭"
            onClick={() => (desktop.isDesktop ? void desktop.requestClose() : unavailable())}
            className="flex h-full w-10 items-center justify-center text-ink-faint transition-colors hover:bg-danger hover:text-white"
          >
            <X size={14} />
          </button>
        </div>
      </div>
    </header>
  )
}
