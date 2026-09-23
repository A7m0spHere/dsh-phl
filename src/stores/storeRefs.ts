import type { CatalogState } from './catalogStore'
import type { InstanceState } from './instanceStore'

/**
 * The seam that keeps the catalog ↔ instance module graph acyclic.
 *
 * Plugin actions read instance records; instance actions read plugin
 * transfers, versions, and runtimes. A static import in either direction is
 * a real cycle (and a dynamic `import()` in a hot path changes the
 * microtask timing that the in-flight dedupe gates of `latestPluginVersion`
 * and snapshot guards depend on). So both stores publish their `getState`
 * here when their module is first evaluated, and consumers reach each
 * other through these accessors — still one module instance, still
 * synchronous, but neither module imports the other at load time.
 */
let catalogGetState: (() => CatalogState) | null = null
let instanceGetState: (() => InstanceState) | null = null

export function registerCatalogStore(getState: () => CatalogState): void {
  catalogGetState = getState
}

export function registerInstanceStore(getState: () => InstanceState): void {
  instanceGetState = getState
}

/** The catalog store's live state. Stores load early; a null here is a bug. */
export function catalogState(): CatalogState {
  if (!catalogGetState) throw new Error('catalog store not yet loaded')
  return catalogGetState()
}

/** The instance store's live state. */
export function instanceState(): InstanceState {
  if (!instanceGetState) throw new Error('instance store not yet loaded')
  return instanceGetState()
}
