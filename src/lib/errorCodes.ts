/**
 * Parse the `[code] message` encoding the Rust side applies to common
 * failures (roadmap O-09). Messages without a recognised prefix pass
 * through untouched — adoption is per-site, so this must never be strict.
 *
 * The UI shows `message` verbatim (it is already Chinese and user-facing);
 * `retryable` / `hint` let callers add an action to the toast instead of
 * leaving "失败" as the whole story.
 */
export type PhlErrCode =
  | 'not-found'
  | 'busy'
  | 'port-conflict'
  | 'net-retryable'
  | 'net-fatal'
  | 'permission'
  | 'disk-full'
  | 'state'

const KNOWN: PhlErrCode[] = [
  'not-found',
  'busy',
  'port-conflict',
  'net-retryable',
  'net-fatal',
  'permission',
  'disk-full',
  'state',
]

/** Whether a retry can plausibly help — mirrors `ErrCode::retryable` in Rust. */
const RETRYABLE: PhlErrCode[] = ['net-retryable', 'busy', 'port-conflict']

const HINTS: Record<PhlErrCode, string> = {
  'not-found': '对象不存在，刷新列表或检查拼写。',
  busy: '等另一个操作结束后重试；可在任务中心查看进度。',
  'port-conflict': '端口被占用：改用自动端口，或释放该端口后重试。',
  'net-retryable': '网络波动，通常直接重试即可。',
  'net-fatal': '地址或来源有问题，检查下载源设置。',
  permission: '系统拒绝了文件操作：检查目录权限或占用后重试。',
  'disk-full': '磁盘空间不足，清理后重试。',
  state: '磁盘状态与操作前提不符，需要先处理遗留记录。',
}

export interface ParsedError {
  /** The stable code, when the message carried one. */
  code: PhlErrCode | null
  /** The message with the `[code] ` prefix removed — what to show users. */
  message: string
  retryable: boolean
  /** One-line next-action wording; `null` for un-coded messages. */
  hint: string | null
}

export function parsePhlError(raw: string): ParsedError {
  const match = /^\[([a-z-]+)\] (.*)$/s.exec(raw)
  if (match && (KNOWN as string[]).includes(match[1])) {
    const code = match[1] as PhlErrCode
    return {
      code,
      message: match[2],
      retryable: (RETRYABLE as string[]).includes(code),
      hint: HINTS[code],
    }
  }
  return { code: null, message: raw, retryable: false, hint: null }
}

/** Normalise whatever a Tauri rejection or Error hands back, then parse. */
export function parseThrownError(err: unknown): ParsedError {
  const text =
    typeof err === 'string'
      ? err
      : err instanceof Error && err.message
        ? err.message
        : String(err)
  return parsePhlError(text)
}
