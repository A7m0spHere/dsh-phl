import { AnimatePresence, motion } from 'motion/react'
import { useMotion } from '@/lib/motion'
import { routeKey, routeTab, useUIStore, type Route } from '@/stores'
import { InstancesPage, InstancesPanel } from '@/pages/InstancesPage'
import { InstanceDetailPage, InstanceDetailPanel } from '@/pages/InstanceDetailPage'
import { CreateInstancePage, CreateInstancePanel } from '@/pages/CreateInstancePage'
import { VersionsPage, VersionsPanel } from '@/pages/VersionsPage'
import { PluginsPage, PluginsPanel } from '@/pages/PluginsPage'
import { RuntimesPage, RuntimesPanel } from '@/pages/RuntimesPage'
import { SettingsPage, SettingsPanel } from '@/pages/SettingsPage'

function renderPage(route: Route) {
  switch (route.name) {
    case 'instances':
      return <InstancesPage />
    case 'instance':
      return <InstanceDetailPage id={route.id} />
    case 'create':
      return <CreateInstancePage cloneFrom={route.cloneFrom} />
    case 'versions':
      return <VersionsPage />
    case 'plugins':
      return <PluginsPage />
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
    case 'versions':
      return <VersionsPanel />
    case 'plugins':
      return <PluginsPanel />
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
            {renderPanel(route)}
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
            {renderPage(route)}
          </motion.div>
        </AnimatePresence>
      </main>
    </div>
  )
}
