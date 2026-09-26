import { beforeEach, describe, expect, it, vi } from 'vitest'

/**
 * R4-01 regression guard for the dependency-retry card's outcome contract.
 *
 * The round-4 counterexample: the backend reports a FAILED dependency install
 * as a normal `Ok(outcome)` — `readiness: 'needsDependencies'` with
 * `dependencyFailures` filled in — and the card announced
 * 「依赖安装完成 / 实例已就绪」 anyway, because it never looked at the result.
 * Only an explicitly-ready outcome with no failures may be announced as
 * success; everything else reports the real reasons and leaves the state
 * alone. The card is exercised as a function (no DOM in this test infra), the
 * same shape the reviewer's own probe uses.
 */

const probe = vi.hoisted(() => ({
  toast: vi.fn(),
  reload: vi.fn(),
  push: vi.fn(),
  prepare: vi.fn(),
}))

vi.mock('react', () => ({ useState: () => [false, vi.fn()] }))
vi.mock('@/components/ui', () => ({ Button: 'button', Notice: 'section' }))
vi.mock('@/stores', () => ({
  useInstanceStore: (select: (s: unknown) => unknown) => select({ reload: probe.reload }),
  useUIStore: { getState: () => ({ toast: probe.toast, push: probe.push }) },
}))
vi.mock('@/services', () => ({
  Cancelled: class Cancelled extends Error {},
  repository: { preparePackDependencies: probe.prepare },
}))

import { Cancelled, repository } from '@/services'
import { DependencyRetryCard } from './DependencyRetryCard'

type Node = { props?: Record<string, unknown> }

function clickHandler(node: unknown): (() => unknown) | undefined {
  if (!node || typeof node !== 'object') return undefined
  const el = node as Node
  if (typeof el.props?.onClick === 'function') return el.props.onClick as () => unknown
  for (const child of [el.props?.children].flat()) {
    const found = clickHandler(child)
    if (found) return found
  }
}

/** All text rendered inside the element tree (host elements included). */
function textOf(node: unknown): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (!node || typeof node !== 'object') return ''
  const el = node as Node
  return [el.props?.children].flat().map(textOf).join(' ')
}

/**
 * The card's buttons are `() => void onRetry()`, so the returned promise says
 * nothing about when the async work finished: click, then let the queue drain.
 */
async function clickThrough(node: unknown): Promise<void> {
  const click = clickHandler(node)
  expect(click).toBeTypeOf('function')
  await click!()
  await new Promise((resolve) => setTimeout(resolve, 0))
}

function card(props: { variant?: 'needsDependencies' | 'corruptImport'; failures?: string[] } = {}) {
  return DependencyRetryCard({ instanceId: 'inst-1', ...props })
}

const notices = () => probe.toast.mock.calls.map(([n]) => n as { kind: string; message?: string })

beforeEach(() => {
  vi.clearAllMocks()
})

describe('dependency retry outcome contract (R4-01)', () => {
  it('announces success only for an explicitly ready outcome', async () => {
    probe.prepare.mockResolvedValue({
      readiness: 'readyToLaunch',
      dependenciesInstalled: 3,
      dependencyFailures: [],
    })
    await clickThrough(card())
    expect(notices()).toHaveLength(1)
    expect(notices()[0].kind).toBe('success')
    expect(probe.reload).toHaveBeenCalledOnce()
  })

  it('reports the real reasons when the install failed, and never claims ready', async () => {
    probe.prepare.mockResolvedValue({
      readiness: 'needsDependencies',
      dependenciesInstalled: 0,
      dependencyFailures: ['依赖 a 下载失败', '依赖 b 与计划不一致'],
    })
    await clickThrough(card())
    const shown = notices()
    expect(shown.some((n) => n.kind === 'success')).toBe(false)
    const failure = shown.find((n) => n.kind === 'error')
    expect(failure?.message).toContain('依赖 a 下载失败')
    expect(failure?.message).toContain('依赖 b 与计划不一致')
    expect(probe.reload).toHaveBeenCalledOnce()
  })

  it('treats a damaged import record the same way — failure in, failure out', async () => {
    probe.prepare.mockResolvedValue({
      readiness: 'needsDependencies',
      dependenciesInstalled: 0,
      dependencyFailures: ['导入记录损坏，无法确认计划依赖集'],
    })
    await clickThrough(card({ variant: 'corruptImport' }))
    const shown = notices()
    expect(shown.some((n) => n.kind === 'success')).toBe(false)
    expect(shown.some((n) => JSON.stringify(n).includes('损坏'))).toBe(true)
  })

  it('does not treat "ready with failures attached" as success either', async () => {
    probe.prepare.mockResolvedValue({
      readiness: 'readyToLaunch',
      dependenciesInstalled: 1,
      dependencyFailures: ['一个可选依赖未装上'],
    })
    await clickThrough(card())
    expect(notices().some((n) => n.kind === 'success')).toBe(false)
  })

  it('says the desktop bridge is missing instead of faking a run', async () => {
    probe.prepare.mockResolvedValue(null)
    await clickThrough(card())
    const shown = notices()
    expect(shown.some((n) => n.kind === 'success')).toBe(false)
    expect(shown.some((n) => n.kind === 'warn')).toBe(true)
    expect(probe.reload).toHaveBeenCalledOnce()
  })

  it('reports a thrown error with its message', async () => {
    probe.prepare.mockRejectedValue(new Error('[busy] 实例正在运行'))
    await clickThrough(card())
    const shown = notices()
    expect(shown.some((n) => n.kind === 'success')).toBe(false)
    expect(shown.some((n) => n.kind === 'error' && n.message?.includes('实例正在运行'))).toBe(true)
  })

  it('reports a cancel without touching the instance state', async () => {
    probe.prepare.mockRejectedValue(new Cancelled())
    await clickThrough(card())
    const shown = notices()
    expect(shown.some((n) => n.kind === 'success')).toBe(false)
    expect(shown.some((n) => n.kind === 'info')).toBe(true)
    expect(probe.reload).not.toHaveBeenCalled()
  })
})

describe('the card tells the truth about what a retry can do (R4-01)', () => {
  it('shows the reasons the import record already holds', () => {
    const text = textOf(
      card({ variant: 'needsDependencies', failures: ['依赖 x 安装失败', 'npm 不可用'] }),
    )
    expect(text).toContain('依赖 x 安装失败')
    expect(text).toContain('npm 不可用')
  })

  it('offers a re-import for a damaged record instead of promising a rebuild', () => {
    const tree = card({ variant: 'corruptImport' })
    const text = textOf(tree)
    expect(text).toContain('重新导入整合包')
    expect(text).toContain('无法在本地重建')
    // The retry button stays reachable (it reports the honest refusal), and
    // the second button navigates to the pack import entry.
    const click = clickHandler(tree)
    expect(click).toBeTypeOf('function')
  })

  it('navigates to the pack import entry from the damaged-record path', () => {
    const tree = card({ variant: 'corruptImport' })
    const buttons: Array<() => unknown> = []
    const walk = (node: unknown) => {
      if (!node || typeof node !== 'object') return
      const el = node as Node & { type?: unknown }
      if (typeof el.props?.onClick === 'function') buttons.push(el.props.onClick as () => unknown)
      for (const child of [el.props?.children].flat()) walk(child)
    }
    walk(tree)
    // buttons[0] is 重试依赖安装, buttons[1] is 重新导入整合包
    expect(buttons.length).toBeGreaterThanOrEqual(2)
    buttons[1]()
    expect(probe.push).toHaveBeenCalledWith({ name: 'installPack' })
  })
})

describe('the retry still calls the backend exactly once (sanity)', () => {
  it('does not silently swallow the outcome by re-running', async () => {
    probe.prepare.mockResolvedValue({
      readiness: 'needsDependencies',
      dependenciesInstalled: 0,
      dependencyFailures: ['x'],
    })
    await clickThrough(card())
    expect(repository.preparePackDependencies).toHaveBeenCalledTimes(1)
  })
})

describe('action labels stay readable in a narrow card (round-5 P2)', () => {
  it('lets the action row wrap and never shrinks a button below its label', () => {
    const tree = card({ variant: 'corruptImport' })
    const classNames: string[] = []
    const buttonClassNames: string[] = []
    const walk = (node: unknown) => {
      if (!node || typeof node !== 'object') return
      const el = node as { type?: unknown; props?: Record<string, unknown> }
      const className = typeof el.props?.className === 'string' ? el.props.className : ''
      if (className) classNames.push(className)
      if (el.type === 'button' && className) buttonClassNames.push(className)
      for (const child of [el.props?.children].flat()) walk(child)
    }
    walk(tree)
    expect(classNames.some((c) => c.includes('flex-wrap'))).toBe(true)
    expect(buttonClassNames.length).toBeGreaterThanOrEqual(2)
    for (const c of buttonClassNames) {
      expect(c).toContain('shrink-0')
      expect(c).toContain('whitespace-nowrap')
    }
  })

  it('keeps the explanation on its own line, not squeezed between buttons', () => {
    const tree = card({ variant: 'corruptImport' })
    let actionsRow: unknown
    const walk = (node: unknown) => {
      if (!node || typeof node !== 'object') return
      const el = node as { props?: Record<string, unknown> }
      if (typeof el.props?.className === 'string' && el.props.className.includes('flex-wrap')) {
        actionsRow = node
      }
      for (const child of [el.props?.children].flat()) walk(child)
    }
    walk(tree)
    expect(actionsRow).toBeDefined()
    // Only buttons live in the action row; prose does not.
    expect(textOf(actionsRow)).not.toContain('要恢复可用实例')
    expect(textOf(tree)).toContain('要恢复可用实例')
  })
})
