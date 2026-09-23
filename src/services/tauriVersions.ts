import * as desktopVersions from '@/lib/desktop'
import { useSettingsStore } from '@/stores/settingsStore'
import type { PhlRepository, TransferProgress } from './repository'
import { Cancelled, InstalledScanError, newTransferId } from './repository'
import type { DshVersion } from '@/types'

/**
 * Desktop overrides for the **version module**: the catalog is npm (+ GitHub
 * releases), installs land under `<root>/versions` through the real Rust
 * pipeline. The assembled desktop repository lives in `tauriRepository.ts` —
 * it is an explicit, type-complete implementation; nothing here (or in the
 * sibling override modules) silently inherits mock behaviour beyond the one
 * shared static surface (templates) that file names.
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
  // Remote catalog: failure degrades to null so locally installed items are
  // still listed (offline mode). Local scan: a throw inside its catch rejects
  // the `Promise.all`, so the refresh fails loudly and the store keeps the
  // last trusted state — silently returning `[]` here made every installed
  // version look installable, inviting a reinstall over a live directory.
  const [remote, installed] = await Promise.all([
    desktopVersions.listDshVersions(registryBase()).catch((err) => {
      console.warn('[phl] version catalog unavailable:', err)
      return null
    }),
    desktopVersions.listInstalledVersions().catch((err) => {
      throw new InstalledScanError(err)
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

/** The version module's overrides, spread into the assembled desktop repository. */
export const tauriVersionOverrides: Pick<
  PhlRepository,
  'enrichModelMetadata' | 'listVersions' | 'installVersion' | 'removeVersion'
> = {
  enrichModelMetadata: desktopVersions.enrichModelMetadata,
  listVersions,
  installVersion,
  removeVersion: async (id) => {
    await desktopVersions.removeVersionDir(versionName(id))
  },
}
