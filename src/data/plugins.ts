import type { Plugin } from '@/types'

const RC8 = 'dsh-0.1.0-rc.8'
const RC7 = 'dsh-0.1.0-rc.7'
const RC6 = 'dsh-0.1.0-rc.6'
const RC5 = 'dsh-0.1.0-rc.5'

export const pluginSeed: Plugin[] = [
  {
    id: 'routing-suite',
    name: 'Routing Suite',
    author: 'dsh-core',
    category: 'routing',
    official: true,
    downloads: 184_200,
    summary: '模型路由、回退链与成本预算的一站式配置。',
    releases: [
      { version: '1.4.0', publishedAt: '2026-08-24', compatible: [RC8, RC7], incompatible: [RC5] },
      { version: '1.3.2', publishedAt: '2026-07-30', compatible: [RC7, RC6] },
      { version: '1.2.3', publishedAt: '2026-06-11', compatible: [RC6, RC5] },
    ],
  },
  {
    id: 'session-inspector',
    name: 'Session Inspector',
    author: 'dsh-core',
    category: 'diagnostics',
    official: true,
    downloads: 96_400,
    summary: '实时查看会话树、token 消耗与工具调用轨迹。',
    releases: [
      { version: '0.9.1', publishedAt: '2026-08-20', compatible: [RC8, RC7, RC6] },
      { version: '0.8.4', publishedAt: '2026-07-02', compatible: [RC7, RC6, RC5] },
    ],
  },
  {
    id: 'prompt-lab',
    name: 'Prompt Lab',
    author: 'weiran',
    category: 'workflow',
    downloads: 51_900,
    summary: '在实例内做提示词 A/B，与 Profile 绑定保存。',
    releases: [
      { version: '2.1.0', publishedAt: '2026-08-12', compatible: [RC8, RC7] },
      { version: '2.0.0', publishedAt: '2026-06-28', compatible: [RC7] },
    ],
  },
  {
    id: 'local-provider',
    name: 'Local Provider',
    author: 'nightfold',
    category: 'provider',
    downloads: 73_100,
    summary: '把本地推理服务接入 DSH，支持 OpenAI 兼容端点。',
    releases: [
      { version: '0.6.2', publishedAt: '2026-08-18', compatible: [RC8, RC7, RC6] },
      { version: '0.5.9', publishedAt: '2026-05-30', compatible: [RC6, RC5] },
    ],
  },
  {
    id: 'theme-nocturne',
    name: 'Nocturne Theme',
    author: 'mizuki',
    category: 'ui',
    downloads: 22_800,
    summary: '低对比暗色主题与等宽排版方案。',
    releases: [{ version: '1.1.0', publishedAt: '2026-07-15', compatible: [RC8, RC7, RC6, RC5] }],
  },
  {
    id: 'toolbelt',
    name: 'Toolbelt',
    author: 'dsh-core',
    category: 'tooling',
    official: true,
    downloads: 141_600,
    summary: '文件、Shell 与 HTTP 工具集合，可按实例裁剪权限。',
    releases: [
      { version: '3.0.1', publishedAt: '2026-08-26', compatible: [RC8], incompatible: [RC6, RC5] },
      { version: '2.7.5', publishedAt: '2026-07-19', compatible: [RC7, RC6] },
      { version: '2.4.0', publishedAt: '2026-04-22', compatible: [RC6, RC5] },
    ],
  },
  {
    id: 'trace-export',
    name: 'Trace Export',
    author: 'lumen',
    category: 'diagnostics',
    downloads: 18_450,
    summary: '把运行轨迹导出为 OTLP / JSONL，便于离线复盘。',
    releases: [{ version: '0.4.0', publishedAt: '2026-08-05', compatible: [RC8, RC7] }],
  },
  {
    id: 'workspace-sync',
    name: 'Workspace Sync',
    author: 'kestrel',
    category: 'workflow',
    downloads: 34_700,
    summary: '在多个实例之间同步 workspace 快照，只读挂载。',
    releases: [
      { version: '1.0.4', publishedAt: '2026-08-09', compatible: [RC8, RC7, RC6] },
      { version: '0.9.0', publishedAt: '2026-06-01', compatible: [RC6] },
    ],
  },
  {
    id: 'guardrails',
    name: 'Guardrails',
    author: 'dsh-core',
    category: 'tooling',
    official: true,
    downloads: 88_300,
    summary: '为工具调用加入确认策略与目录白名单。',
    releases: [
      { version: '1.2.0', publishedAt: '2026-08-22', compatible: [RC8, RC7] },
      { version: '1.0.7', publishedAt: '2026-06-14', compatible: [RC7, RC6, RC5] },
    ],
  },
  {
    id: 'metrics-panel',
    name: 'Metrics Panel',
    author: 'orbit',
    category: 'diagnostics',
    downloads: 12_900,
    summary: '实例级延迟、失败率与并发指标面板。',
    releases: [{ version: '0.3.1', publishedAt: '2026-07-27', compatible: [RC7, RC6] }],
  },
]
