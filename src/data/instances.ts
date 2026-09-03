import type { Instance, InstanceTemplate } from '@/types'

/**
 * Default location for PHL's own data. Not the install directory — see the
 * note on `DEFAULT_ROOT` in `stores/settingsStore.ts`.
 */
export const PHL_ROOT = 'C:\\Users\\dev\\AppData\\Local\\PHL'

/** Fresh install: no instances exist yet. */
export const instanceSeed: Instance[] = []

/**
 * Templates describe the *shape* of an instance — version channel and kind —
 * and no longer seed a plugin list.
 *
 * They used to name plugins like `dsh-core/routing-suite`, which were demo
 * entries that do not exist in the live community registry. Once instance
 * creation became real those ids would have been written into a new instance
 * as "installed" while nothing was ever fetched or placed on disk. Seeding
 * plugins again needs ids verified against the real catalog.
 */
export const templateSeed: InstanceTemplate[] = [
  {
    id: 'blank',
    name: '空白实例',
    description: '只安装 DSH 与 Runtime，不预置任何插件。',
    kind: 'sandbox',
    plugins: [],
    icon: 'square',
  },
  {
    id: 'plugin-dev',
    name: '插件开发',
    description: '面向插件开发：优先使用最新版本，创建后自行装入调试插件。',
    kind: 'development',
    plugins: [],
    prefer: 'latest',
    icon: 'wrench',
  },
  {
    id: 'daily',
    name: '日常使用',
    description: '锁定稳定版本，适合作为主力环境。',
    kind: 'production',
    plugins: [],
    prefer: 'stable',
    icon: 'shield',
  },
  {
    id: 'regression',
    name: '回归测试',
    description: '锁定旧版本，用于复现历史问题。',
    kind: 'test',
    plugins: [],
    prefer: 'legacy',
    icon: 'flask',
  },
]

/** Locally linked plugins that have no registry entry. */
export const linkedPluginNames: Record<string, string> = {
  'routing-lab-local': 'routing-lab (本地链接)',
  'my-tool-bridge': 'my-tool-bridge (本地链接)',
}
