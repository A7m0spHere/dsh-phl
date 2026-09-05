import type { DshVersion } from '@/types'

const day = (n: number) => {
  const d = new Date('2026-08-28T10:00:00Z')
  d.setDate(d.getDate() - n)
  return d.toISOString()
}

export const versionSeed: DshVersion[] = [
  {
    // Mirrors a real situation: GitHub cut the tag, npm hasn't published the
    // package yet. In the browser preview there is no agent to hand the build
    // task to, so the row still surfaces the guide + GitHub link.
    id: 'dsh-0.1.3-alpha.1',
    name: '0.1.3-alpha.1',
    channel: 'alpha',
    releasedAt: day(1),
    size: 0,
    requiresNode: [],
    pendingPublish: true,
    notes: ['Session 持久化 API 改为 lifecycle-scoped SessionHandle', 'Session 格式升级到 v2'],
    state: { kind: 'available' },
  },
  {
    id: 'dsh-0.1.0-rc.8',
    name: '0.1.0-rc.8',
    channel: 'rc',
    releasedAt: day(2),
    size: 48_210_000,
    requiresNode: [22, 24],
    latest: true,
    notes: [
      'Plugin host 重写，插件不再共享全局 require 缓存',
      '新增 DSH_HOME 覆盖优先级说明',
      '修复长会话下的内存增长',
    ],
    state: { kind: 'available' },
  },
  {
    id: 'dsh-0.1.0-rc.7',
    name: '0.1.0-rc.7',
    channel: 'rc',
    releasedAt: day(16),
    size: 47_640_000,
    requiresNode: [20, 22],
    notes: ['Profile 支持多份并存', 'WebUI 端口可通过 --port 指定'],
    state: { kind: 'available' },
  },
  {
    id: 'dsh-0.1.0-rc.6',
    name: '0.1.0-rc.6',
    channel: 'rc',
    releasedAt: day(38),
    size: 46_900_000,
    requiresNode: [20, 22],
    notes: ['插件依赖解析改为按实例隔离'],
    state: { kind: 'available' },
  },
  {
    id: 'dsh-0.1.0-rc.5',
    name: '0.1.0-rc.5',
    channel: 'rc',
    releasedAt: day(64),
    size: 45_120_000,
    requiresNode: [18, 20],
    legacy: true,
    notes: ['已停止维护，仅用于回归测试'],
    state: { kind: 'available' },
  },
  {
    id: 'dsh-0.1.0-rc.4',
    name: '0.1.0-rc.4',
    channel: 'rc',
    releasedAt: day(92),
    size: 44_030_000,
    requiresNode: [18, 20],
    legacy: true,
    notes: ['旧版 Profile 结构'],
    state: { kind: 'available' },
  },
  {
    id: 'dsh-0.2.0-nightly.0831',
    name: '0.2.0-nightly.0831',
    channel: 'nightly',
    releasedAt: day(0),
    size: 49_880_000,
    requiresNode: [24],
    notes: ['每日构建，接口可能随时变化'],
    state: { kind: 'available' },
  },
  {
    id: 'dsh-0.0.9',
    name: '0.0.9',
    channel: 'stable',
    releasedAt: day(140),
    size: 41_500_000,
    requiresNode: [18],
    legacy: true,
    notes: ['最后一个 0.0.x 稳定版'],
    state: { kind: 'available' },
  },
]
