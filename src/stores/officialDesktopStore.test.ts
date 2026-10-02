import { beforeEach, expect, it, vi } from 'vitest'
import type { OfficialDesktopInfo } from '@/types'

/**
 * The official-desktop store's two hard rules: a probe it cannot trust never
 * reads as "not running" (the card must say 无法确认), and a quit that times
 * out never goes force without the user's explicit confirmation — the
 * official app may be running quit-inspection of its own that only the user
 * can answer.
 */

const mocks = vi.hoisted(() => ({
  inspect: vi.fn(),
  launch: vi.fn(),
  quit: vi.fn(),
  toast: vi.fn(),
  confirm: vi.fn(),
}))

vi.mock('@/lib/desktop', () => ({
  isDesktop: true,
  inspectOfficialDesktop: mocks.inspect,
  launchOfficialDesktop: mocks.launch,
  quitOfficialDesktop: mocks.quit,
}))

vi.mock('./uiStore', () => ({
  useUIStore: {
    getState: () => ({ toast: mocks.toast, confirm: mocks.confirm }),
  },
}))

const { useOfficialDesktopStore } = await import('./officialDesktopStore')

function reset(info: OfficialDesktopInfo | null = null) {
  useOfficialDesktopStore.setState({
    info,
    loaded: false,
    loading: false,
    launching: false,
    quitting: false,
    error: null,
  })
}

function installedInfo(over: Partial<OfficialDesktopInfo> = {}): OfficialDesktopInfo {
  return {
    status: 'installed',
    root: 'D:/dsh',
    mainExe: 'D:/dsh/DeepSeek Harness.exe',
    version: '0.2.0-rc.2',
    running: false,
    pid: undefined,
    ...over,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  reset()
})

it('refresh keeps the live snapshot and marks loaded even for a failed probe', async () => {
  mocks.inspect.mockResolvedValueOnce(installedInfo({ running: true, pid: 42 }))
  await useOfficialDesktopStore.getState().refresh()
  expect(useOfficialDesktopStore.getState().info?.running).toBe(true)
  expect(useOfficialDesktopStore.getState().loaded).toBe(true)
  expect(useOfficialDesktopStore.getState().error).toBeNull()

  mocks.inspect.mockRejectedValueOnce(new Error('boom'))
  await useOfficialDesktopStore.getState().refresh()
  // The failed probe must NOT fall back to the last observation's "running"
  // value: the card reads 无法确认 while error carries the reason.
  expect(useOfficialDesktopStore.getState().error).toBe('boom')
})

it('launch reports failure through a toast and refreshes anyway', async () => {
  mocks.launch.mockRejectedValueOnce(new Error('官方桌面端已在运行，无需重复启动'))
  await useOfficialDesktopStore.getState().launch()
  expect(mocks.toast).toHaveBeenCalledWith(
    expect.objectContaining({ kind: 'error', title: '无法启动官方桌面端' }),
  )
  expect(mocks.inspect).toHaveBeenCalled()
})

it('a graceful quit that exits stays graceful', async () => {
  mocks.quit.mockResolvedValueOnce('exited')
  await useOfficialDesktopStore.getState().requestQuit()
  expect(mocks.quit).toHaveBeenCalledWith(false)
  expect(mocks.confirm).not.toHaveBeenCalled()
  expect(mocks.toast).toHaveBeenCalledWith(
    expect.objectContaining({ kind: 'success', title: '官方桌面端已退出' }),
  )
})

it('still_running goes force only after the user confirms', async () => {
  mocks.quit.mockResolvedValueOnce('still_running').mockResolvedValueOnce('exited')
  mocks.confirm.mockResolvedValueOnce(true)
  await useOfficialDesktopStore.getState().requestQuit()
  expect(mocks.confirm).toHaveBeenCalledWith(
    expect.objectContaining({ tone: 'danger', title: '强制结束官方桌面端？' }),
  )
  expect(mocks.quit).toHaveBeenNthCalledWith(2, true)
  expect(mocks.toast).toHaveBeenCalledWith(
    expect.objectContaining({ kind: 'success', title: '官方桌面端已退出' }),
  )
})

it('still_running without confirmation never sends the force quit', async () => {
  mocks.quit.mockResolvedValueOnce('still_running')
  mocks.confirm.mockResolvedValueOnce(false)
  await useOfficialDesktopStore.getState().requestQuit()
  expect(mocks.quit).toHaveBeenCalledTimes(1)
  expect(mocks.quit).toHaveBeenCalledWith(false)
  expect(mocks.toast).not.toHaveBeenCalledWith(
    expect.objectContaining({ kind: 'success' }),
  )
})

it('a forced quit that still leaves the process reports it instead of success', async () => {
  mocks.quit.mockResolvedValueOnce('still_running').mockResolvedValueOnce('still_running')
  mocks.confirm.mockResolvedValueOnce(true)
  await useOfficialDesktopStore.getState().requestQuit()
  expect(mocks.toast).toHaveBeenCalledWith(
    expect.objectContaining({ kind: 'error', title: '强制结束未生效' }),
  )
})
