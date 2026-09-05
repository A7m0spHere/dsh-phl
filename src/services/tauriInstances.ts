import * as desktop from '@/lib/desktop'
import { slugify } from '@/lib/format'
import type { Instance, InstanceDraft, InstanceKind, InstanceTemplate } from '@/types'
import { Cancelled, newTransferId } from './repository'
import type { CreateProgress, PhlRepository } from './repository'

/**
 * Desktop overrides for the **instance module**: instances are directories
 * under `<root>/instances/<id>`, and `instance.json` is their identity.
 *
 * Every call passes only stable ids — the data root is resolved Rust-side
 * from `PhlState`, so the WebView never aims a destructive operation at a
 * path of its own choosing.
 *
 * The plugin list is deliberately not passed in either direction — Rust reads
 * it back from `node_modules` and `cordis.patch.yml`, which is what DSH itself
 * consults. A record kept alongside those files could only ever drift from
 * them.
 */

/**
 * The directory name. Two instances may share a display name, so the slug
 * alone cannot be the identity — a short random suffix keeps them apart, and
 * the whole thing stays inside the filesystem whitelist Rust enforces.
 */
export function newInstanceId(name: string): string {
  const slug = slugify(name).replace(/[^a-zA-Z0-9-_]/g, '') || 'instance'
  return `${slug.slice(0, 40)}-${Math.random().toString(36).slice(2, 6)}`
}

/** `Instance` → the manifest fields Rust persists. */
function toManifest(instance: Instance): desktop.RemoteInstanceManifest {
  return {
    id: instance.id,
    name: instance.name,
    note: instance.note ?? null,
    kind: instance.kind,
    hue: instance.hue,
    versionId: instance.versionId,
    runtimeId: instance.runtimeId,
    port: instance.port,
    autoPort: instance.autoPort,
    profile: instance.profile,
    createdAt: instance.createdAt,
    lastRunAt: instance.lastRunAt ?? null,
    totalRuntime: instance.totalRuntime,
    favorite: !!instance.favorite,
    env: instance.env,
    args: instance.args,
    api: instance.api ?? null,
  }
}

/**
 * `diskUsage` is filled in separately: walking the tree is far too slow to do
 * while listing, and nothing on the instances page needs it. The storage
 * section asks for it on demand.
 */
function fromRecord(record: desktop.RemoteInstanceRecord): Instance {
  return {
    id: record.id,
    name: record.name,
    note: record.note ?? undefined,
    kind: record.kind as InstanceKind,
    hue: record.hue,
    versionId: record.versionId,
    runtimeId: record.runtimeId,
    port: record.port,
    autoPort: record.autoPort,
    dshHome: record.dshHome,
    workspace: record.workspace,
    profile: record.profile,
    plugins: record.plugins,
    createdAt: record.createdAt,
    lastRunAt: record.lastRunAt ?? undefined,
    totalRuntime: record.totalRuntime,
    diskUsage: 0,
    favorite: record.favorite,
    env: record.env,
    args: record.args,
    // Read back from `<instance>/snapshots/*/snapshot.json`. This used to be
    // hardcoded to `[]` from when snapshots were not implemented, which meant
    // every reload dropped them from the UI while the (potentially multi-GB)
    // trees stayed on disk — unrestorable and undeletable.
    snapshots: record.snapshots,
    api: record.api ?? null,
  }
}

/** Used by flows that receive a raw Rust record (e.g. bundle import). */
export const instanceFromRecord = fromRecord

async function listInstances(): Promise<Instance[]> {
  const records = await desktop.listInstanceRecords()
  return records.map(fromRecord)
}

async function createInstance(
  draft: InstanceDraft,
  template: InstanceTemplate | undefined,
  onProgress: (p: CreateProgress) => void,
  signal: AbortSignal,
): Promise<Instance> {
  if (signal.aborted) throw new Cancelled()

  onProgress({ step: 'create', progress: 0.1, detail: '写入 instance.json' })
  const id = newInstanceId(draft.name)
  // Browser-mode fallback for a draft built outside the wizard (apiInheritance
  // became required after the feature; old callers may still omit it).
  const apiInheritance = draft.apiInheritance ?? 'default'
  const manifest: desktop.RemoteInstanceManifest = {
    id,
    name: draft.name.trim(),
    note: draft.note.trim() || null,
    kind: draft.kind,
    hue: draft.hue,
    versionId: draft.versionId!,
    runtimeId: draft.runtimeId!,
    port: draft.port,
    autoPort: draft.autoPort,
    // `dsh web` boots `$DSH_HOME/profiles/web` and is the only entry that
    // parses --port; plugins must live in that same profile to be loaded.
    profile: 'web',
    createdAt: new Date().toISOString(),
    lastRunAt: null,
    totalRuntime: 0,
    favorite: false,
    env: {},
    args: [],
    // 'default' inherits every enabled provider; anything else leaves the
    // instance's config entirely to DSH. create_instance applies the
    // materialization inside the same Rust call.
    api: apiInheritance === 'none' ? { inheritance: 'none', providerIds: [] } : { inheritance: 'default', providerIds: [] },
  }

  const record = await desktop.createInstanceDir(manifest)

  // Directory creation is a handful of syscalls with no cancellable stage, so
  // there is nothing to abort mid-flight. What must not happen is resolving
  // normally after the wizard was cancelled: the store would then add an
  // instance the user cancelled. Delete what we just made and report it.
  if (signal.aborted) {
    await desktop.deleteInstanceDir(id).catch(() => {})
    throw new Cancelled()
  }

  // DSH and Node are *not* copied in: they stay shared under <root>/versions
  // and <root>/runtimes, and the instance only references them by id. That is
  // what lets several instances pin different versions without paying for
  // each one twice.
  onProgress({ step: 'environment', progress: 0.55, detail: '关联 DSH 版本与 Runtime' })
  onProgress({ step: 'configure', progress: 0.8, detail: '准备 DSH_HOME 与 profile' })
  // No template seeds plugins today — the old ones named demo packages that
  // do not exist in the live registry. Reporting a `plugins` step here would
  // claim work that never happens, so it stays out until templates carry ids
  // verified against the real catalog and this actually drives an install.
  void template
  onProgress({ step: 'done', progress: 1 })
  return fromRecord(record)
}

async function cloneInstance(source: Instance, name: string, port: number): Promise<Instance> {
  const id = newInstanceId(name)
  const record = await desktop.cloneInstanceDir({
    transferId: `i:${id}`,
    sourceId: source.id,
    manifest: {
      ...toManifest(source),
      id,
      name,
      note: `从 ${source.name} 克隆`,
      port,
      createdAt: new Date().toISOString(),
      lastRunAt: null,
      totalRuntime: 0,
      favorite: false,
    },
    onProgress: () => {},
  })
  return fromRecord(record)
}

/** The instance module's overrides, spread into the desktop repository. */
export const tauriInstanceOverrides: Pick<
  PhlRepository,
  | 'listInstances'
  | 'createInstance'
  | 'cloneInstance'
  | 'deleteInstance'
  | 'saveInstance'
  | 'measureDiskUsage'
  | 'listOrphanInstanceDirs'
  | 'removeOrphanInstanceDir'
  | 'createSnapshot'
  | 'restoreSnapshot'
  | 'deleteSnapshot'
> = {
  listInstances,
  createInstance,
  cloneInstance,
  deleteInstance: async (id) => {
    await desktop.deleteInstanceDir(id)
  },
  saveInstance: async (instance) => {
    await desktop.saveInstanceManifest(toManifest(instance))
  },
  measureDiskUsage: async (instance) => desktop.instanceDiskUsage(instance.id),
  listOrphanInstanceDirs: async () => desktop.scanOrphanInstances(),
  removeOrphanInstanceDir: async (name) => {
    await desktop.deleteOrphanInstance(name)
  },
  createSnapshot: async (instance, onProgress, signal) => {
    const transferId = newTransferId(`s:${instance.id}`)
    if (signal.aborted) throw new Cancelled()
    const onAbort = () => void desktop.cancelTransfer(transferId)
    signal.addEventListener('abort', onAbort, { once: true })
    try {
      return await desktop.createInstanceSnapshot(instance.id, transferId, onProgress)
    } catch (err) {
      if (signal.aborted) throw new Cancelled()
      throw err instanceof Error ? err : new Error(String(err))
    } finally {
      signal.removeEventListener('abort', onAbort)
    }
  },
  restoreSnapshot: async (instance, snapshotId) => {
    const record = await desktop.restoreInstanceSnapshot(instance.id, snapshotId)
    return instanceFromRecord(record)
  },
  deleteSnapshot: async (instance, snapshotId) => {
    await desktop.deleteInstanceSnapshot(instance.id, snapshotId)
  },
}
