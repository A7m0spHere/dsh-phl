import * as desktop from '@/lib/desktop'
import { NODE_DIST_MIRROR, NODE_DIST_OFFICIAL } from '@/lib/desktop'
import { useSettingsStore } from '@/stores/settingsStore'
import type { Runtime } from '@/types'
import { Cancelled, newTransferId } from './repository'
import type { PhlRepository, TransferProgress } from './repository'

/**
 * Desktop overrides for the **runtime module**: the catalog is the nodejs.org
 * dist index (the npmmirror copy for CN users), installs unpack into
 * `<root>/runtimes/node-<major>/` after SHASUMS256 verification, and the
 * 「系统 Node」 entry is whatever `node --version` finds on PATH.
 */

function phlRoot(): string {
  return useSettingsStore.getState().root
}

/** npm 镜像选择同样适用于 Node 的二进制分发；自定义 npm 源对 dist 无意义。 */
function distBase(): string {
  const { source } = useSettingsStore.getState()
  return source === 'mirror-cn' ? NODE_DIST_MIRROR : NODE_DIST_OFFICIAL
}

async function listRuntimes(): Promise<Runtime[]> {
  const [catalog, installed, usage, system] = await Promise.all([
    desktop.listNodeRuntimeCatalog(distBase()).catch((err) => {
      console.warn('[phl] runtime catalog unavailable:', err)
      return null
    }),
    desktop.listInstalledRuntimes(phlRoot()).catch((err) => {
      console.warn('[phl] installed-runtime scan unavailable:', err)
      return []
    }),
    desktop.runtimesDiskUsage(phlRoot()).catch(() => ({} as Record<string, number>)),
    desktop.systemNodeVersion().catch(() => null),
  ])

  const pending = new Map(installed.map((i) => [i.name, i]))
  const out: Runtime[] = []

  for (const meta of catalog ?? []) {
    const at = pending.get(meta.id)
    // dist 索引不发布体积，占用只能来自磁盘实测；未安装时无从得知。
    out.push({
      id: meta.id,
      name: `Node ${meta.major}`,
      major: meta.major,
      // 已安装的行显示磁盘标记里的实际版本，而不是目录里更新的最新版 ——
      // 否则重装前 UI 承诺的是一个并不存在的版本。
      version: at ? at.version : meta.version,
      codename: meta.codename ?? undefined,
      lts: meta.lts,
      size: at ? (usage[meta.id] ?? 0) : 0,
      state: at ? { kind: 'installed', installedAt: at.installedAt } : { kind: 'available' },
    })
    pending.delete(meta.id)
  }

  // 已安装但目录里查不到的（目录是官方离线包手放的、或目录请求失败）——
  // 与版本列表同一待遇：照常展示，不能因为目录挂了就消失。
  for (const [name, info] of pending) {
    const major = Number(name.replace(/^node-/, ''))
    if (!Number.isFinite(major)) continue
    out.push({
      id: name,
      name: `Node ${major}`,
      major,
      version: info.version,
      lts: false,
      size: usage[name] ?? 0,
      state: { kind: 'installed', installedAt: info.installedAt },
    })
  }
  out.sort((a, b) => b.major - a.major)

  if (system) {
    const major = Number(system.split('.')[0])
    out.push({
      id: 'node-system',
      name: '系统 Node',
      major: Number.isFinite(major) ? major : 0,
      version: system,
      lts: false,
      size: 0,
      system: true,
      state: { kind: 'installed', installedAt: new Date().toISOString() },
    })
  }
  return out
}

async function installRuntime(
  runtime: Runtime,
  onProgress: (p: TransferProgress) => void,
  signal: AbortSignal,
): Promise<void> {
  if (runtime.system) {
    throw new Error('系统 Node 由系统管理，PHL 不负责安装')
  }
  if (!runtime.version) {
    throw new Error('这个 Runtime 没有可用的下载源')
  }
  const transferId = newTransferId(`r:${runtime.id}`)
  if (signal.aborted) throw new Cancelled()
  const onAbort = () => void desktop.cancelTransfer(transferId)
  signal.addEventListener('abort', onAbort, { once: true })
  try {
    await desktop.downloadNodeRuntime({
      transferId,
      distBase: distBase(),
      versionName: runtime.id,
      version: runtime.version,
      root: phlRoot(),
      keepArchive: useSettingsStore.getState().keepArchives,
      onProgress,
    })
  } catch (err) {
    if (signal.aborted) throw new Cancelled()
    throw err instanceof Error ? err : new Error(String(err))
  } finally {
    signal.removeEventListener('abort', onAbort)
  }
}

/** The runtime module's overrides, spread into the desktop repository. */
export const tauriRuntimeOverrides: Pick<
  PhlRepository,
  'listRuntimes' | 'installRuntime' | 'removeRuntime'
> = {
  listRuntimes,
  installRuntime,
  removeRuntime: async (id) => {
    await desktop.removeRuntimeDir(phlRoot(), id)
  },
}
