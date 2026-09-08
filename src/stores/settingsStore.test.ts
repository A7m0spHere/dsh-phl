import { beforeEach, describe, expect, it, vi } from 'vitest'

/**
 * The data-root switch is the one settings action that can silently split the
 * UI from the backend: `setRoot` writes localStorage and fires the backend
 * call without waiting, so a path the backend refuses used to leave the field
 * showing one directory while every read and write resolved against another.
 */
const mocks = vi.hoisted(() => ({
  isDesktop: true,
  setPhlRoot: vi.fn(),
}))

vi.mock('@/lib/desktop', () => ({
  // A getter, not a copy: the browser case flips it after the factory ran.
  get isDesktop() {
    return mocks.isDesktop
  },
  setPhlRoot: mocks.setPhlRoot,
  defaultRoot: vi.fn().mockResolvedValue(null),
  initPhlRoot: vi.fn().mockResolvedValue(null),
  rootDataSummary: vi.fn().mockResolvedValue(null),
  chooseDirectory: vi.fn().mockResolvedValue(null),
}))
vi.mock('./uiStore', () => ({
  useUIStore: { getState: () => ({ toast: vi.fn(), confirm: vi.fn() }) },
}))

import { useSettingsStore } from './settingsStore'

describe('setRootVerified', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.isDesktop = true
    useSettingsStore.setState({ root: 'C:/PHL' })
  })

  it('adopts the root the backend accepted, normalised', async () => {
    mocks.setPhlRoot.mockResolvedValue('D:/PHL')
    await expect(useSettingsStore.getState().setRootVerified('D:/PHL/')).resolves.toBe(true)
    expect(mocks.setPhlRoot).toHaveBeenCalledWith('D:/PHL')
    expect(useSettingsStore.getState().root).toBe('D:/PHL')
  })

  it('keeps the current root when the backend refuses the path', async () => {
    // A relative path, an unwritable config dir — the backend answers null and
    // the app must not move on its own.
    mocks.setPhlRoot.mockResolvedValue(null)
    await expect(useSettingsStore.getState().setRootVerified('D:PHL')).resolves.toBe(false)
    expect(useSettingsStore.getState().root).toBe('C:/PHL')
  })

  it('adopts locally in the browser, where there is no backend', async () => {
    mocks.isDesktop = false
    await expect(useSettingsStore.getState().setRootVerified('E:/PHL')).resolves.toBe(true)
    expect(mocks.setPhlRoot).not.toHaveBeenCalled()
    expect(useSettingsStore.getState().root).toBe('E:/PHL')
  })
})
