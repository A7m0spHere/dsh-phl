import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

/**
 * Static guard for the official-desktop card's honesty rules (node-only test
 * infra, same source-level style as InstanceCard.test.ts): a state the probe
 * could not determine must read 无法确认/状态未知, never collapse into 「未运行」
 * — that difference is what stops the card from inviting a second launch of
 * an app that may well be running. The action set is also pinned: PHL only
 * ever offers launch / quit-request / reveal on this read-only singleton.
 */

function source(name: string): string {
  return readFileSync(fileURLToPath(new URL(name, import.meta.url)), 'utf8')
}

describe('official desktop card', () => {
  const card = source('./OfficialDesktopCard.tsx')

  it('distinguishes an untrusted probe from a confirmed not-running', () => {
    expect(card).toContain('tone="warn"')
    expect(card).toContain('状态未知')
    expect(card).toContain('无法确认')
    // The three-value running check: true / false / undefined each get their
    // own branch, in that order of specificity.
    expect(card.indexOf('info.running === true')).toBeGreaterThan(-1)
    expect(card.indexOf('info.running === false')).toBeGreaterThan(-1)
    expect(card).toContain('canDrive')
  })

  it('offers exactly the read-only singleton action set', () => {
    expect(card).toContain('启动')
    expect(card).toContain('请求退出')
    expect(card).toContain('打开安装目录')
    expect(card).toContain('重新检测')
    // The read-only promise is stated on the card itself.
    expect(card).toContain('PHL 只读管理')
  })

  it('keeps 重新检测 available on the installed state too (状态未知 must be actionable)', () => {
    // The refresh affordance is gated only on "there is something to probe",
    // never on "not installed" — an honest unknown must not strand the user
    // until the next poll tick.
    expect(card).not.toContain('!installed && loaded')
    expect(card).toContain("info.status !== 'unsupported'")
  })

  it('shows the browser-mode notice instead of fabricating an install', () => {
    expect(card).toContain('浏览器模式不提供官方桌面端管理')
  })

  it('scopes its poll loop to the card lifecycle', () => {
    expect(card).toContain('startOfficialDesktopPolling()')
    expect(card).toContain('stopOfficialDesktopPolling()')
  })
})
