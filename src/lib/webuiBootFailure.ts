/**
 * The WebUI boot-failure card, read.
 *
 * `dsh web`'s browser-side boot code renders its failure as a plain card
 * (wordmark, "Failed to load plugins", one line per entry that never
 * activated). The shell probe forwards that text verbatim — it is not ours to
 * own, its exact wording can change with any DSH release. So this parser must
 * only ever *degrade*, never fail: anything it cannot interpret stays visible
 * as the original line, and an entirely unknown format still yields a usable
 * report (headline + raw text), because "页面报错了，原文如下" beats a crash.
 */

export interface BootFailureReading {
  /** One-line headline for the toast / card chip. */
  headline: string
  /** One human-readable line per plugin that failed to activate. */
  causes: string[]
  /** What the user can do about it, phrased for a launcher's user. */
  hint: string
}

/** `<id> (<name>): pending (waiting for service(s): a, b)` — the incompatibility shape. */
const PENDING_LINE = /^(.+?)\s*(?:\(([^)]*)\))?:\s*pending\s*\(\s*waiting for service(?:s)?\s*:\s*([^)]*)\)/i
/** `<id> (<name>): <phase>: <error>` / `failed to import` — the throwing shape. */
const FAILED_LINE = /^(.+?)\s*(?:\(([^)]*)\))?:\s*(failed.*|error.*)$/i
/** The `web boot: 1 entry did not activate` count header. */
const COUNT_LINE = /(\d+)\s+entr(?:y|ies)\s+did not activate/i

/** Prefer the package id, fall back to the parenthesised name. */
const pluginName = (id: string, pkg: string | undefined): string =>
  id.trim() || (pkg ?? '').trim() || '未知插件'

export function interpretBootFailure(detail: string): BootFailureReading {
  const lines = detail.split(/\r?\n/).map((l) => l.trim()).filter(Boolean)
  const causes: string[] = []

  for (const line of lines) {
    const pending = line.match(PENDING_LINE)
    if (pending) {
      const name = pluginName(pending[1], pending[2])
      const services = (pending[3] || 'unknown').trim()
      causes.push(
        `插件「${name}」在等待服务 ${services} —— 当前 DSH 版本没有提供，` +
          `这通常意味着该插件是为旧版本写的，与实例现用版本不兼容。`,
      )
      continue
    }
    const failed = line.match(FAILED_LINE)
    if (failed) {
      const name = pluginName(failed[1], failed[2])
      causes.push(`插件「${name}」加载时抛错：${failed[3]}`)
    }
  }

  const counted = detail.match(COUNT_LINE)
  const headline =
    causes.length > 0
      ? `WebUI 启动失败：${causes.length} 个插件未能加载`
      : counted
        ? `WebUI 启动失败：${counted[1]} 个插件条目未能加载`
        : 'WebUI 页面报告启动失败'

  const hint =
    causes.length > 0
      ? '处理：在这个实例里停用/卸载上面点名的插件再启动；或把实例切到该插件兼容的 DSH 版本。' +
        '（实例进程本身是活的，坏的只是网页加载。）'
      : '处理：页面未能完成加载，但原文没有被识别——把下面的原文连同实例的报错日志一起反馈。'

  return { headline, causes, hint }
}
