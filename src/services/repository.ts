import type {
  DshVersion,
  InstalledPlugin,
  Instance,
  InstanceDraft,
  InstanceTemplate,
  LaunchPhase,
  Plugin,
  Runtime,
  Snapshot,
} from '@/types'
/**
 * Everything the UI is allowed to know about where data comes from.
 *
 * Phase 1 binds this to `mockRepository`. Phase 2+ binds the exact same
 * surface to PHL Core over Tauri IPC; no page or store should need to change.
 */

export interface LaunchContext {
  version: DshVersion | undefined
  runtime: Runtime | undefined
  /** Ports currently held by other running instances. */
  portsInUse: Map<number, string>
}

export interface LaunchProgress {
  phase: LaunchPhase
  /** 0..1 across the whole launch. */
  progress: number
  /** Human-readable detail for the current phase, e.g. a plugin name. */
  detail?: string
}

export interface LaunchOutcome {
  pid: number
  port: number
  /** Authenticated `dsh web` URL; the bare host:port fails DSH's auth. */
  webUrl?: string
}

export class LaunchError extends Error {
  constructor(
    public title: string,
    public detail: string,
    public hint?: string,
  ) {
    super(title)
    this.name = 'LaunchError'
  }
}

export class Cancelled extends Error {
  constructor() {
    super('cancelled')
    this.name = 'Cancelled'
  }
}

let transferSeq = 0

/**
 * A transfer id that is unique to one attempt.
 *
 * The id only has to correlate a cancel with the transfer it belongs to, and
 * making it unique closes a nasty failure mode: a cancel that loses the race
 * and arrives just after the transfer finished used to register a cancel flag
 * under a *stable* id, which then aborted the next install of the same
 * plugin or version instantly — repeatably, until the app was restarted.
 */
export function newTransferId(prefix: string): string {
  transferSeq += 1
  return `${prefix}#${transferSeq}`
}

export interface TransferProgress {
  /**
   * 0..1, and **optional**: `preparing` and `verifying` are unit variants on
   * the Rust side and carry no numbers at all. They are typed optional so a
   * consumer has to decide what to show instead of silently rendering
   * `undefined` into a progress bar.
   */
  progress?: number
  bytesDone?: number
  bytesPerSec?: number
  /**
   * `extracting` and `installing-deps` are the version pipeline (tar unpack,
   * then npm materialising the version's own dependencies); plugins use
   * `preparing` (resolve tarball) and `installing` (unpack + register) so the
   * UI can label the PCL-style stages precisely.
   */
  stage:
    | 'preparing'
    | 'downloading'
    | 'extracting'
    | 'verifying'
    | 'installing-deps'
    | 'installing'
}

/** Progress of a local tree copy (snapshot create / clone). */
export interface CopyProgress {
  /** 0..1 */
  progress: number
  bytesDone: number
  bytesTotal: number
}

export interface CreateProgress {
  step: 'create' | 'environment' | 'configure' | 'plugins' | 'done'
  progress: number
  detail?: string
}

export interface PluginCatalog {
  plugins: Plugin[]
  /** The live registry was unreachable; `plugins` comes from cache/bundle. */
  offline?: boolean
  error?: string
}

export interface PhlRepository {
  listInstances(): Promise<Instance[]>
  listVersions(): Promise<DshVersion[]>
  listRuntimes(): Promise<Runtime[]>
  /**
   * The plugin catalog degrades gracefully instead of failing hard: when the
   * live registry is unreachable the repository serves a cached or bundled
   * copy and says so via `offline`, so the market is never a dead page.
   */
  listPlugins(): Promise<PluginCatalog>
  listTemplates(): Promise<InstanceTemplate[]>

  createInstance(
    draft: InstanceDraft,
    template: InstanceTemplate | undefined,
    onProgress: (p: CreateProgress) => void,
    signal: AbortSignal,
  ): Promise<Instance>

  cloneInstance(source: Instance, name: string, port: number): Promise<Instance>
  deleteInstance(id: string): Promise<void>

  /**
   * Copies the instance's `dsh-home` (profile, plugins, cordis config) into a
   * restorable snapshot. Workspace and logs are deliberately excluded — a
   * snapshot makes the environment reproducible, it does not back up data.
   */
  createSnapshot(
    instance: Instance,
    onProgress: (p: CopyProgress) => void,
    signal: AbortSignal,
  ): Promise<Snapshot>

  /**
   * Copies the snapshot's `dsh-home` back over the live one. The snapshot
   * itself survives, so the same point can be restored repeatedly.
   */
  restoreSnapshot(instance: Instance, snapshotId: string): Promise<Instance>

  deleteSnapshot(instance: Instance, snapshotId: string): Promise<void>

  /**
   * Persists edits to an instance's configuration — name, port, env, args,
   * favourite. Separate from `createInstance` because the store mutates
   * instances in place and every one of those edits has to reach disk; before
   * this existed they lived only in memory and vanished on restart.
   */
  saveInstance(instance: Instance): Promise<void>

  /**
   * Bytes on disk for one instance's tree. Measured on demand rather than
   * returned by `listInstances`: walking a tree with `node_modules` in it is
   * far too slow to do on every list, and only the storage view needs it.
   */
  measureDiskUsage(instance: Instance): Promise<number>

  /**
   * Directories under `instances/` that carry no manifest — interrupted
   * creates, and plugin trees written before instances were real. They are
   * invisible to the rest of the app, so the storage view is the only place
   * the space can be reclaimed.
   */
  listOrphanInstanceDirs(): Promise<{ name: string; size: number }[]>
  removeOrphanInstanceDir(name: string): Promise<void>

  launch(
    instance: Instance,
    ctx: LaunchContext,
    onProgress: (p: LaunchProgress) => void,
    signal: AbortSignal,
  ): Promise<LaunchOutcome>

  stop(instance: Instance, signal: AbortSignal): Promise<void>

  installVersion(
    version: DshVersion,
    onProgress: (p: TransferProgress) => void,
    signal: AbortSignal,
  ): Promise<void>

  removeVersion(id: string): Promise<void>

  installRuntime(
    runtime: Runtime,
    onProgress: (p: TransferProgress) => void,
    signal: AbortSignal,
  ): Promise<void>

  removeRuntime(id: string): Promise<void>

  /**
   * Downloads and installs a plugin into the instance's profile: resolve the
   * tarball (npm packument / direct / GitHub), verify its integrity, unpack
   * into `node_modules` and register it in `cordis.patch.yml`.
   * Resolves with the concrete version that landed on disk, and the id it was
   * registered under — the caller records that so later enable/uninstall
   * calls need neither the catalog nor a second derivation of the same id.
   */
  installPlugin(
    plugin: Plugin,
    instance: Instance,
    onProgress: (p: TransferProgress) => void,
    signal: AbortSignal,
  ): Promise<{ version: string; registryId?: string }>

  /**
   * Flags the plugin disabled/enabled in the profile's `cordis.patch.yml`.
   *
   * Takes the *installed* record rather than the catalog entry: the catalog
   * is empty whenever the registry is unreachable, and gating a disk write on
   * it turned enable/disable into a silent no-op that still reported success.
   */
  setPluginEnabled(
    instance: Instance,
    installed: InstalledPlugin,
    enabled: boolean,
  ): Promise<void>

  /** Removes the package from `node_modules` and its registry entry. */
  uninstallPlugin(instance: Instance, installed: InstalledPlugin): Promise<void>

  /**
   * Newest published version for an installed plugin, or `null` when it
   * cannot be determined (e.g. a GitHub-source plugin). Drives the
   * updates tab without hammering npm for the whole catalog.
   */
  latestPluginVersion(plugin: Plugin): Promise<string | null>
}
