/**
 * The task centre's label layer (O-10 / audit P3-8, P3-9). Kept out of the
 * component so a node-environment test can import it — and so a kind added
 * in Rust (`guarded(..., "kind", ...)`) has exactly one map to extend,
 * enforced by `taskLabels.test.ts` enumerating every `guarded` call in the
 * Rust sources.
 */
import type { TaskInfo } from '@/lib/desktop'

type PhaseView = Pick<TaskInfo, 'phase' | 'cancelRequested' | 'state'>

/**
 * The directories a data-root migration moves, keyed by the token the
 * backend pastes into `set_phase` (forward: the bare kind; undo:
 * `撤销 <kind>`). ONE table for both spellings — the earlier per-string
 * enumeration was what this file exists to end.
 */
export const DIRECTORY_LABEL: Record<string, string> = {
  instances: '实例目录',
  versions: '版本目录',
  runtimes: 'Runtime 目录',
  config: '配置目录',
  cache: '缓存目录',
}

export const KIND_LABEL: Record<string, string> = {
  'plugin-install': '安装插件',
  'plugin-uninstall': '卸载插件',
  'plugin-toggle': '插件开关',
  'instance-create': '创建实例',
  'instance-save': '保存实例',
  'instance-delete': '删除实例',
  'instance-clone': '克隆实例',
  'instance-repair': '修复实例',
  'snapshot-create': '创建快照',
  'snapshot-restore': '恢复快照',
  'snapshot-delete': '删除快照',
  'bundle-import': '导入 Bundle',
  'version-install': '安装 DSH 版本',
  'version-remove': '删除 DSH 版本',
  'runtime-install': '安装 Runtime',
  'runtime-remove': '删除 Runtime',
  'root-migration': '迁移数据目录',
  'root-migration-finish': '完成目录迁移',
  'root-migration-undo': '撤销目录迁移',
  adopt: '接入本机 DSH',
  'pack-export': '导出整合包',
  'pack-install': '安装整合包',
  'session-copy': '迁移会话',
  'cache-clear': '清理下载缓存',
}

export const PHASE_LABEL: Record<string, string> = {
  started: '准备中',
  resolving: '解析来源',
  downloading: '下载中',
  verifying: '校验完整性',
  extracting: '解压中',
  'installing-deps': '安装依赖',
  checking: '健康检查',
  committing: '提交变更',
  removing: '删除目录',
  copying: '复制文件',
  moving: '搬迁',
  committed: '提交完成',
}

export function kindLabel(kind: string): string {
  return KIND_LABEL[kind] ?? kind
}

/**
 * Localise a phase token without enumerating every compound string the
 * backend can produce: fixed tokens first, then the migration directory
 * phases (forward = bare kind, undo = `撤销 <kind>`), then phases the
 * backend already emits in Chinese (write-list / unpack-packs steps) pass
 * through unchanged. A token with no mapping shows verbatim — the test
 * below fails first when Rust adds one without a label.
 */
export function phaseText(t: PhaseView): string {
  if (t.cancelRequested && t.state === 'running') return '正在取消…'
  const direct = PHASE_LABEL[t.phase]
  if (direct) return direct
  const directory = DIRECTORY_LABEL[t.phase]
  if (directory) return `搬运${directory}`
  const undo = /^撤销\s+(\S+)$/.exec(t.phase)
  if (undo) {
    const dir = DIRECTORY_LABEL[undo[1]]
    if (dir) return `撤销${dir}`
  }
  return t.phase
}
