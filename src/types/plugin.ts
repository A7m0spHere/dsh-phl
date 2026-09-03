/**
 * Plugins belong to an Instance, never to the machine. The catalog below is
 * the *registry* view; per-instance installs are recorded on the Instance.
 *
 * The registry view mirrors the real DSH community catalog
 * (awesome-dsh-plugin `plugins.json`): entries carry an npm package or a
 * GitHub repo, bilingual summaries, and popularity counters. Version data
 * only exists once an install has been resolved against npm.
 */

/**
 * Category taxonomy follows the real registry's 23 keys. Seeds only use a
 * subset; the adapter passes unknown keys through as `dev` fallback.
 */
export type PluginCategory =
  | 'agi'
  | 'ui'
  | 'usage'
  | 'theme'
  | 'model'
  | 'identity'
  | 'session'
  | 'memory'
  | 'tools'
  | 'wsl'
  | 'browser'
  | 'vision'
  | 'voice'
  | 'docs'
  | 'skill'
  | 'workflow'
  | 'git'
  | 'notify'
  | 'dev'
  | 'security'
  | 'remote'
  | 'market'
  | 'fun'

export const PLUGIN_CATEGORIES: PluginCategory[] = [
  'agi',
  'ui',
  'usage',
  'theme',
  'model',
  'identity',
  'session',
  'memory',
  'tools',
  'wsl',
  'browser',
  'vision',
  'voice',
  'docs',
  'skill',
  'workflow',
  'git',
  'notify',
  'dev',
  'security',
  'remote',
  'market',
  'fun',
]

export const PLUGIN_CATEGORY_LABELS: Record<PluginCategory, string> = {
  agi: '智能体',
  ui: '界面',
  usage: '用量',
  theme: '主题',
  model: '模型接入',
  identity: '身份',
  session: '会话',
  memory: '记忆',
  tools: '工具',
  wsl: 'WSL',
  browser: '浏览器',
  vision: '视觉',
  voice: '语音',
  docs: '文档',
  skill: '技能',
  workflow: '工作流',
  git: 'Git',
  notify: '通知',
  dev: '开发',
  security: '安全',
  remote: '远程',
  market: '市场',
  fun: '趣味',
}

/**
 * Where the bits come from, in the real ecosystem's priority order:
 * a repo-verified npm package beats an author-built tarball, which beats
 * pulling the GitHub repo source.
 */
export type PluginSource =
  | { kind: 'npm'; pkg: string }
  | { kind: 'tarball'; url: string; integrity?: string }
  | { kind: 'github'; repo: string; path?: string }

export interface PluginRelease {
  version: string
  publishedAt: string
  /** Overrides `Plugin.source` when a release ships from somewhere else. */
  source?: PluginSource
  /** Tarball size in bytes when known (npm HEAD / registry metadata). */
  size?: number
  /** DSH version ids this release is verified against. */
  compatible: string[]
  /** DSH version ids known to break. */
  incompatible?: string[]
}

export interface Plugin {
  id: string
  name: string
  author: string
  category: PluginCategory
  summary: string
  /** English summary from the registry's bilingual description. */
  summaryEn?: string
  repoUrl?: string
  screenshots?: string[]
  /** Newest release, first element of `releases`. Real registry entries
   *  carry a single rolling release; the version resolves at install time. */
  releases: PluginRelease[]
  /** How this plugin is fetched when installed. */
  source: PluginSource
  official?: boolean
  downloads: number
  stars?: number
  addedAt?: string
}

/** A plugin as installed inside one Instance. */
export interface InstalledPlugin {
  pluginId: string
  version: string
  enabled: boolean
  /**
   * The id this plugin is registered under in `cordis.patch.yml` — the npm
   * package name, or the repo name for GitHub sources.
   *
   * Recorded at install time and read back from disk, rather than re-derived
   * from the catalog entry: enabling and uninstalling must keep working when
   * the registry is unreachable, and deriving it separately in TypeScript and
   * Rust gave two implementations that could disagree.
   */
  registryId?: string
  /** Local, unpublished plugin being developed against this instance. */
  linked?: boolean
}

/**
 * Install lifecycle for one (instance, plugin) pair. Kept in the store's
 * transfer map rather than on `Plugin` because the same plugin can be
 * installing into several instances at once.
 */
export type PluginInstallState =
  | { kind: 'available' }
  | { kind: 'queued' }
  | { kind: 'preparing' }
  | { kind: 'downloading'; progress: number; bytesDone: number; bytesPerSec: number }
  | { kind: 'verifying' }
  | { kind: 'installing'; progress: number }
  | { kind: 'failed'; reason: string }

export const latestRelease = (p: Plugin) => p.releases[0]
