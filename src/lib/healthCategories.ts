/**
 * Display labels for the verify report's check categories (audit C-14).
 * The backend tokens (`dsh`, `manifest`, …) used to render verbatim — the
 * card mixed English identifiers into an otherwise fully Chinese surface.
 * `healthCategories.test.ts` enumerates every `VerifyCheck::` construction
 * in `src-tauri/src/verify.rs` and fails when a token lacks a label here,
 * so this map cannot silently fall behind the backend.
 */

export const CATEGORY_LABEL: Record<string, string> = {
  manifest: '实例清单',
  dsh: 'DSH 程序',
  runtime: 'Runtime',
  config: '配置文件',
  plugins: '插件配置',
  api: 'API 配置',
  launch: '启动条件',
}

export function categoryLabel(category: string): string {
  return CATEGORY_LABEL[category] ?? category
}

/**
 * Display labels for repair actions, on the same principle as the categories
 * above: the report's `repairAction` tokens used to render verbatim ("已修复：
 * recreate-workspace"), which reads as a code identifier in the middle of a
 * sentence. `healthCategories.test.ts` enumerates every action literal the
 * backend can emit, so this map cannot fall behind either.
 */
export const REPAIR_ACTION_LABEL: Record<string, string> = {
  'recreate-workspace': '重建 workspace 目录',
  'recreate-skeleton': '重建实例目录骨架',
  'cleanup-txn': '清理中断的安装残留',
  'disable-conflicting-plugins': '关闭已知会让 DSH 出问题的插件',
  'install-version': '安装 DSH 版本',
  'reinstall-version': '重装 DSH 版本',
  'install-runtime': '安装 Runtime',
  'reinstall-runtime': '重装 Runtime',
}

export function repairActionLabel(action: string): string {
  return REPAIR_ACTION_LABEL[action] ?? action
}
