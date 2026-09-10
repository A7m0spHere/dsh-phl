import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Instance, Plugin } from '@/types'
import { Cancelled } from './repository'

const mocks = vi.hoisted(() => ({ installPlugin: vi.fn(), cancelTransfer: vi.fn() }))
vi.mock('@/lib/desktop', () => mocks)
vi.mock('@/stores/settingsStore', () => ({
  useSettingsStore: { getState: () => ({ source: 'official', customSource: '' }) },
}))

import { tauriPluginOverrides } from './tauriPlugins'

const plugin = { id: 'plugin', source: { type: 'npm', package: 'plugin' } } as unknown as Plugin
const instance = { id: 'instance' } as Instance

describe('plugin cancellation at commit boundaries', () => {
  beforeEach(() => vi.resetAllMocks())

  it('keeps a committed success even when cancellation arrives before the reply', async () => {
    const controller = new AbortController()
    mocks.installPlugin.mockImplementation(async () => {
      controller.abort()
      return { version: '2.0.0', registryId: 'plugin', trust: 'pinned' }
    })
    await expect(tauriPluginOverrides.installPlugin(plugin, instance, vi.fn(), controller.signal))
      .resolves.toEqual({ version: '2.0.0', registryId: 'plugin', trust: 'pinned' })
  })

  it('carries the committed trust through, defaulting a missing wire value to unknown', async () => {
    mocks.installPlugin.mockResolvedValue({ version: '1.0.0', registryId: 'plugin' })
    await expect(
      tauriPluginOverrides.installPlugin(plugin, instance, vi.fn(), new AbortController().signal),
    ).resolves.toEqual({ version: '1.0.0', registryId: 'plugin', trust: 'unknown' })
    mocks.installPlugin.mockResolvedValue({ version: '1.0.0', registryId: 'plugin', trust: 'verified' })
    await expect(
      tauriPluginOverrides.installPlugin(plugin, instance, vi.fn(), new AbortController().signal),
    ).resolves.toEqual({ version: '1.0.0', registryId: 'plugin', trust: 'verified' })
  })

  it('does not hide a failed recovery behind a late cancel request', async () => {
    const controller = new AbortController()
    const failure = new Error('恢复未完成，旧版本保留在事务目录')
    mocks.installPlugin.mockImplementation(async () => {
      controller.abort()
      throw failure
    })
    await expect(tauriPluginOverrides.installPlugin(plugin, instance, vi.fn(), controller.signal))
      .rejects.toBe(failure)
  })

  it('maps a backend-confirmed cancellation to Cancelled', async () => {
    mocks.installPlugin.mockRejectedValue('cancelled')
    await expect(tauriPluginOverrides.installPlugin(plugin, instance, vi.fn(), new AbortController().signal))
      .rejects.toBeInstanceOf(Cancelled)
  })

  it('never invokes the backend for an already aborted request', async () => {
    const controller = new AbortController()
    controller.abort()
    await expect(tauriPluginOverrides.installPlugin(plugin, instance, vi.fn(), controller.signal))
      .rejects.toBeInstanceOf(Cancelled)
    expect(mocks.installPlugin).not.toHaveBeenCalled()
  })
})
