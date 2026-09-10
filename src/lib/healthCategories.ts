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
  api: 'API 配置',
  launch: '启动条件',
}

export function categoryLabel(category: string): string {
  return CATEGORY_LABEL[category] ?? category
}
