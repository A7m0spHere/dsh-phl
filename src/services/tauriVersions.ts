import * as desktopVersions from '@/lib/desktop'
import { useSettingsStore } from '@/stores/settingsStore'
import { mockRepository } from './mockRepository'
import { tauriPluginOverrides } from './tauriPlugins'
import { tauriInstanceOverrides } from './tauriInstances'
import { tauriRuntimeOverrides } from './tauriRuntimes'
import { tauriLaunchOverrides } from './tauriLaunches'
import type { PhlRepository, TransferProgress } from './repository'
import { Cancelled, newTransferId } from './repository'
import type { DshVersion } from '@/types'

/**
 * Desktop repository: the version / plugin / runtime / instance / process
 * modules are real (GitHub + npm + nodejs.org dist + on-disk instances +
 * spawned DSH processes, all through the Rust pipeline). What still delegates
 * to mock is whatever the override objects below do not cover — templates.
 * In the browser everything is mock so `npm run dev` keeps working for pure
 * UI work.
 */

/** Resolve the npm registry base from the user's download-source setting. */
export function registryBase(): string {
  const { source, customSource } = useSettingsStore.getState()
  if (source === 'mirror-cn') return 'https://registry.npmmirror.com'
  if (source === 'custom' && customSource.trim()) return customSource.trim()
  return 'https://registry.npmjs.org'
}

/** Version ids are `dsh-<name>`; the install directory uses the bare name. */
const versionName = (id: string) => id.replace(/^dsh-/, '')

async function listVersions(): Promise<DshVersion[]> {
  const [remote, installed] = await Promise.all([
    desktopVersions.listDshVersions(registryBase()).catch((err) => {
      console.warn('[phl] version catalog unavailable:', err)
      return null
    }),
    desktopVersions.listInstalledVersions().catch((err) => {
      console.warn('[phl] installed-version scan unavailable:', err)
      return []
    }),
  ])

  const installedInfo = new Map(installed.map((i) => [i.name, i]))
  const versions: DshVersion[] = []

  for (const meta of remote ?? []) {
    versions.push({
      ...meta,
      channel: meta.channel as DshVersion['channel'],
      state: (() => {
        const info = installedInfo.get(meta.name)
        return info
          ? {
              kind: 'installed' as const,
              installedAt: info.installedAt,
              installHealth:
                info.installHealth === 'degraded' ? ('degraded' as const) : ('healthy' as const),
              skippedDependencies: info.skippedDependencies,
            }
          : ({ kind: 'available' as const } as const)
      })(),
    })
    installedInfo.delete(meta.name)
  }

  // Locally installed but no longer in the remote catalog (or the catalog
  // request failed entirely) — still a first-class installed version.
  for (const [name, info] of installedInfo) {
    versions.push({
      id: `dsh-${name}`,
      name,
      channel: 'stable',
      releasedAt: '',
      size: 0,
      requiresNode: [], // no catalog entry to read `engines.node` from
      notes: [],
      state: {
        kind: 'installed',
        installedAt: info.installedAt,
        installHealth: info.installHealth === 'degraded' ? 'degraded' : 'healthy',
        skippedDependencies: info.skippedDependencies,
      },
    })
  }
  return versions
}

async function installVersion(
  version: DshVersion,
  onProgress: (p: TransferProgress) => void,
  signal: AbortSignal,
): Promise<void> {
  if (!version.source) {
    throw new Error('这个版本没有可用的下载源')
  }
  const transferId = newTransferId(`v:${version.id}`)
  // `addEventListener('abort')` never fires on an already-aborted signal, so
  // a cancel that lands before this call would otherwise be ignored outright
  // and the whole tarball would download behind a "已取消" toast.
  if (signal.aborted) throw new Cancelled()
  const onAbort = () => void desktopVersions.cancelTransfer(transferId)
  signal.addEventListener('abort', onAbort, { once: true })
  try {
    await desktopVersions.downloadDshVersion({
      transferId,
      tarballUrl: version.source.tarball,
      integrity: version.source.integrity,
      versionName: version.name,
      // Rust installs the package's own dependencies after extraction —
      // against the same registry the catalog is read from.
      registryBase: registryBase(),
      keepArchive: useSettingsStore.getState().keepArchives,
      totalBytes: version.size || undefined,
      onProgress,
    })
  } catch (err) {
    if (signal.aborted) throw new Cancelled()
    throw err instanceof Error ? err : new Error(String(err))
  } finally {
    signal.removeEventListener('abort', onAbort)
  }
}

/**
 * `{ ...mockRepository }` would silently drop every method: class methods
 * live on the prototype and spread only copies own properties. Walk the
 * prototype chain and bind instead.
 */
function bindRepositoryMethods(source: object): PhlRepository {
  const out: Record<string, unknown> = {}
  let proto: object | null = source
  while (proto && proto !== Object.prototype) {
    for (const name of Object.getOwnPropertyNames(proto)) {
      if (name === 'constructor' || name in out) continue
      const value = (source as Record<string, unknown>)[name]
      if (typeof value === 'function') out[name] = (value as (...args: unknown[]) => unknown).bind(source)
    }
    proto = Object.getPrototypeOf(proto)
  }
  return out as unknown as PhlRepository
}

const mock = bindRepositoryMethods(mockRepository)

export const tauriRepository: PhlRepository = {
  ...mock,
  enrichModelMetadata: desktopVersions.enrichModelMetadata,
  // The plugin module is real on desktop too (community registry + Rust
  // download pipeline); see tauriPlugins for what it overrides.
  ...tauriPluginOverrides,
  // Instances are real directories under <root>/instances; see tauriInstances.
  ...tauriInstanceOverrides,
  // Runtimes are the nodejs.org dist index + unpacked trees under
  // <root>/runtimes; see tauriRuntimes.
  ...tauriRuntimeOverrides,
  // Launch/stop spawn real DSH processes; see tauriLaunches.
  ...tauriLaunchOverrides,
  listVersions,
  installVersion,
  removeVersion: async (id) => {
    await desktopVersions.removeVersionDir(versionName(id))
  },
}
