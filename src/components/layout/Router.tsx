import { lazy, Suspense } from 'react'
import { AnimatePresence, motion } from 'motion/react'
import { useMotion } from '@/lib/motion'
import { Skeleton } from '@/components/ui'
import { routeKey, routeTab, useUIStore, type Route } from '@/stores'
import { InstancesPage, InstancesPanel } from '@/pages/InstancesPage'
// Keep the landing page eager; fetch other page modules only when visited.
const InstanceDetailPage = lazy(() => import('@/pages/InstanceDetailPage').then((m) => ({ default: m.InstanceDetailPage })))
const InstanceDetailPanel = lazy(() => import('@/pages/InstanceDetailPage').then((m) => ({ default: m.InstanceDetailPanel })))
const CreateInstancePage = lazy(() => import('@/pages/CreateInstancePage').then((m) => ({ default: m.CreateInstancePage })))
const CreateInstancePanel = lazy(() => import('@/pages/CreateInstancePage').then((m) => ({ default: m.CreateInstancePanel })))
const AdoptDshPage = lazy(() => import('@/pages/AdoptDshPage').then((m) => ({ default: m.AdoptDshPage })))
const AdoptDshPanel = lazy(() => import('@/pages/AdoptDshPage').then((m) => ({ default: m.AdoptDshPanel })))
const PackInstallPage = lazy(() => import('@/pages/PackInstallPage').then((m) => ({ default: m.PackInstallPage })))
const PackInstallPanel = lazy(() => import('@/pages/PackInstallPage').then((m) => ({ default: m.PackInstallPanel })))
const PackExportPage = lazy(() => import('@/pages/PackExportPage').then((m) => ({ default: m.PackExportPage })))
const VersionsPage = lazy(() => import('@/pages/VersionsPage').then((m) => ({ default: m.VersionsPage })))
const VersionsPanel = lazy(() => import('@/pages/VersionsPage').then((m) => ({ default: m.VersionsPanel })))
const PluginsPage = lazy(() => import('@/pages/PluginsPage').then((m) => ({ default: m.PluginsPage })))
const PluginsPanel = lazy(() => import('@/pages/PluginsPage').then((m) => ({ default: m.PluginsPanel })))
const ApiConfigPage = lazy(() => import('@/pages/ApiConfigPage').then((m) => ({ default: m.ApiConfigPage })))
const ApiConfigPanel = lazy(() => import('@/pages/ApiConfigPage').then((m) => ({ default: m.ApiConfigPanel })))
const RuntimesPage = lazy(() => import('@/pages/RuntimesPage').then((m) => ({ default: m.RuntimesPage })))
const RuntimesPanel = lazy(() => import('@/pages/RuntimesPage').then((m) => ({ default: m.RuntimesPanel })))
const SettingsPage = lazy(() => import('@/pages/SettingsPage').then((m) => ({ default: m.SettingsPage })))
const SettingsPanel = lazy(() => import('@/pages/SettingsPage').then((m) => ({ default: m.SettingsPanel })))

function renderPage(route: Route) {
  switch (route.name) {
    case 'instances':
      return <InstancesPage />
    case 'instance':
      return <InstanceDetailPage id={route.id} />
    case 'create':
      return <CreateInstancePage cloneFrom={route.cloneFrom} />
    case 'adopt':
      return <AdoptDshPage />
    case 'installPack':
      return <PackInstallPage />
    case 'exportPack':
      return <PackExportPage id={route.id} />
    case 'versions':
      return <VersionsPage />
    case 'plugins':
      return <PluginsPage />
    case 'apiConfig':
      return <ApiConfigPage />
    case 'runtimes':
      return <RuntimesPage />
    case 'settings':
      return <SettingsPage />
  }
}

function renderPanel(route: Route) {
  switch (route.name) {
    case 'instances':
      return <InstancesPanel />
    case 'instance':
      return <InstanceDetailPanel id={route.id} />
    case 'create':
      return <CreateInstancePanel />
    case 'adopt':
      return <AdoptDshPanel />
    case 'installPack':
      return <PackInstallPanel />
    case 'exportPack':
      return <InstanceDetailPanel id={route.id} />
    case 'versions':
      return <VersionsPanel />
    case 'plugins':
      return <PluginsPanel />
    case 'apiConfig':
      return <ApiConfigPanel />
    case 'runtimes':
      return <RuntimesPanel />
    case 'settings':
      return <SettingsPanel />
  }
}

/**
 * Page transitions carry direction: going deeper slides in from the right,
 * going back slides in from the left. The left panel only cross-fades — it is
 * context, and context should not appear to travel.
 */
export function Router() {
  const route = useUIStore((s) => s.route)
  const direction = useUIStore((s) => s.direction)
  const { page, t, scale } = useMotion()

  const panelKey = route.name === 'instance' ? 'instance-detail' : routeTab(route) + route.name

  return (
    <div className="flex min-h-0 flex-1">
      <div className="relative shrink-0" style={{ width: 'var(--panel-w)' }}>
        <AnimatePresence mode="wait" initial={false}>
          <motion.div
            key={panelKey}
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={t(0.14)}
            className="absolute inset-0 flex"
          >
            <Suspense fallback={<LoadingPage />}>{renderPanel(route)}</Suspense>
          </motion.div>
        </AnimatePresence>
      </div>

      <main className="canvas-field relative min-w-0 flex-1 overflow-hidden">
        <AnimatePresence mode="wait" custom={direction} initial={false}>
          <motion.div
            key={routeKey(route)}
            custom={direction}
            variants={page}
            initial="enter"
            animate="center"
            exit="exit"
            className="absolute inset-0"
            style={scale === 0 ? undefined : { willChange: 'transform, opacity' }}
          >
            <Suspense fallback={<LoadingPage />}>{renderPage(route)}</Suspense>
          </motion.div>
        </AnimatePresence>
      </main>
    </div>
  )
}

function LoadingPage() {
  // A skeleton shaped like a page, not a line of text: the module chunk lands
  // in a few frames, and a bare 「加载页面…」 flashed as a layout jump on every
  // lazy navigation. Same header + body rhythm as PageShell so the real page
  // settles into the placeholder's footprint.
  return (
    <div role="status" aria-live="polite" className="flex h-full min-h-0 flex-col">
      <span className="sr-only">加载页面…</span>
      <div className="shrink-0 px-[var(--page-pad)] pb-2 pt-3">
        <div className="mx-auto w-full" style={{ maxWidth: 'min(900px, 100%)' }}>
          <Skeleton className="h-[22px] w-40" />
          <Skeleton className="mt-2 h-[15px] w-72" />
        </div>
      </div>
      <div className="min-h-0 flex-1 px-[var(--page-pad)] pb-6">
        <div className="mx-auto w-full space-y-2.5" style={{ maxWidth: 'min(900px, 100%)' }}>
          {[0, 1, 2].map((i) => (
            <Skeleton key={i} className="h-[76px]" />
          ))}
        </div>
      </div>
    </div>
  )
}
