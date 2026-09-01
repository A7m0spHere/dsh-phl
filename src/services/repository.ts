import type {
  DshVersion,
  Instance,
  InstanceDraft,
  InstanceTemplate,
  LaunchPhase,
  Plugin,
  Runtime,
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

export interface TransferProgress {
  /** 0..1 */
  progress: number
  bytesDone: number
  bytesPerSec: number
  stage: 'downloading' | 'extracting' | 'verifying'
}

export interface CreateProgress {
  step: 'create' | 'environment' | 'configure' | 'plugins' | 'done'
  progress: number
  detail?: string
}

export interface PhlRepository {
  listInstances(): Promise<Instance[]>
  listVersions(): Promise<DshVersion[]>
  listRuntimes(): Promise<Runtime[]>
  listPlugins(): Promise<Plugin[]>
  listTemplates(): Promise<InstanceTemplate[]>

  createInstance(
    draft: InstanceDraft,
    template: InstanceTemplate | undefined,
    onProgress: (p: CreateProgress) => void,
    signal: AbortSignal,
  ): Promise<Instance>

  cloneInstance(source: Instance, name: string, port: number): Promise<Instance>
  deleteInstance(id: string): Promise<void>

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
}
