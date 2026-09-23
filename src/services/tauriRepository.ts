import { mockRepository } from './mockRepository'
import { tauriVersionOverrides } from './tauriVersions'
import { tauriPluginOverrides } from './tauriPlugins'
import { tauriInstanceOverrides } from './tauriInstances'
import { tauriRuntimeOverrides } from './tauriRuntimes'
import { tauriLaunchOverrides } from './tauriLaunches'
import type { PhlRepository } from './repository'

/**
 * The desktop repository, assembled explicitly.
 *
 * Every method of `PhlRepository` is named here on purpose. The old shape
 * bound the whole mock and spread "overrides" over it, so any method nobody
 * overrode silently kept mock behavior — and a method *added* to the
 * interface was invisible to this file until someone noticed the hole. The
 * `PhlRepository` annotation below turns that into a compile error: a new
 * interface member forces a decision about where the desktop implementation
 * comes from.
 *
 * The modules are real: versions come from npm/GitHub into `<root>/versions`,
 * runtimes from the nodejs.org dist index into `<root>/runtimes`, plugins
 * from the community registry through the Rust pipeline, instances are
 * directories under `<root>/instances`, and launch spawns real DSH processes.
 * The one shared static surface still delegated to the mock is the
 * **template** list (`src/data/instances` seed) — templates are demo data
 * that the create flow explicitly does not apply on desktop yet.
 */
export const tauriRepository: PhlRepository = {
  ...tauriVersionOverrides,
  ...tauriPluginOverrides,
  ...tauriInstanceOverrides,
  ...tauriRuntimeOverrides,
  ...tauriLaunchOverrides,
  // Shared static demo data (roadmap: retire with a template backend).
  listTemplates: () => mockRepository.listTemplates(),
}
