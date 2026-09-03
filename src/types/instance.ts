import type { InstalledPlugin } from './plugin'
import type { ApiBinding, ApiInheritance } from './apiConfig'

/**
 * The Instance is PHL's first-class citizen. Everything an environment needs
 * to be reproducible hangs off it — version, runtime, plugins, paths, port.
 */
export type InstanceStatus = 'stopped' | 'starting' | 'running' | 'stopping' | 'error'

/** Ordered phases of a launch; the UI narrates them one by one. */
export type LaunchPhase =
  | 'resolve-version'
  | 'resolve-runtime'
  | 'install-deps'
  | 'prepare-home'
  | 'link-plugins'
  | 'allocate-port'
  | 'spawn'
  | 'await-ready'

export const LAUNCH_PHASES: LaunchPhase[] = [
  'resolve-version',
  'resolve-runtime',
  'install-deps',
  'prepare-home',
  'link-plugins',
  'allocate-port',
  'spawn',
  'await-ready',
]

export const LAUNCH_PHASE_LABEL: Record<LaunchPhase, string> = {
  'resolve-version': '解析 DSH 版本',
  'resolve-runtime': '准备 Node Runtime',
  'install-deps': '安装版本依赖',
  'prepare-home': '挂载 DSH_HOME',
  'link-plugins': '装载插件',
  'allocate-port': '分配端口',
  spawn: '启动 DSH 进程',
  'await-ready': '等待服务就绪',
}

/** Live state while an instance is starting, running or failing. */
export interface InstanceRuntimeState {
  status: InstanceStatus
  phase?: LaunchPhase
  /** 0..1 across the whole launch sequence. */
  progress?: number
  pid?: number
  startedAt?: number
  /** Authenticated `dsh web` URL captured at launch; WebUI links prefer it. */
  webUrl?: string
  error?: { title: string; detail: string; hint?: string }
}

export type InstanceKind = 'production' | 'development' | 'test' | 'sandbox'

export interface Instance {
  id: string
  name: string
  /** Short free-text note shown under the name. */
  note?: string
  kind: InstanceKind
  /** Accent hue index 0..5, gives each instance a stable identity colour. */
  hue: number
  versionId: string
  runtimeId: string
  port: number
  /** Auto-assign a free port at launch instead of pinning `port`. */
  autoPort: boolean
  dshHome: string
  workspace: string
  profile: string
  plugins: InstalledPlugin[]
  createdAt: string
  lastRunAt?: string
  /** Cumulative run time in seconds, across all sessions. */
  totalRuntime: number
  /** Bytes on disk for this instance's isolated tree. */
  diskUsage: number
  favorite?: boolean
  /** Environment variables layered on top of the resolved defaults. */
  env: Record<string, string>
  args: string[]
  snapshots: Snapshot[]
  /**
   * Binding to the global API library; null/undefined = manifests from before
   * the feature (treated as unmanaged until the user binds it explicitly).
   */
  api?: ApiBinding | null
}

export interface Snapshot {
  id: string
  label: string
  createdAt: string
  versionId: string
  runtimeId: string
  pluginCount: number
  size: number
}

export interface InstanceTemplate {
  id: string
  name: string
  description: string
  kind: InstanceKind
  /** Plugin ids seeded into the new instance. */
  plugins: string[]
  /** Hint for the wizard: preferred channel of DSH version. */
  prefer?: 'latest' | 'stable' | 'legacy'
  icon: string
}

/** Draft carried through the create-instance wizard. */
export interface InstanceDraft {
  name: string
  note: string
  kind: InstanceKind
  hue: number
  versionId: string | null
  runtimeId: string | null
  templateId: string | null
  autoPort: boolean
  port: number
  copyFromId?: string | null
  /** Create-time API takeover: 'default' inherits the global library,
   * 'none' leaves the instance to its own DSH config. */
  apiInheritance: ApiInheritance
}
