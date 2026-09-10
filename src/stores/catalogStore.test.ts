import { beforeEach, describe, expect, it, vi } from 'vitest'

// Only the latest-version probe path is exercised here; the rest of the
// repository surface is irrelevant to these actions and stubbed empty.
const latest = vi.hoisted(() => vi.fn())
const toggled = vi.hoisted(() => vi.fn())
const toastSpy = vi.hoisted(() => vi.fn())
// Mutable so individual tests can run actions against a running instance.
const instanceStatus = vi.hoisted(() => ({ current: 'stopped' as string }))

vi.mock('@/services', () => ({
  repository: {
    latestPluginVersion: (...args: unknown[]) => latest(...args),
    setPluginEnabled: (...args: unknown[]) => toggled(...args),
  },
  Cancelled: class Cancelled extends Error {},
}))
vi.mock('./uiStore', () => ({
  useUIStore: { getState: () => ({ toast: toastSpy, push: vi.fn() }) },
}))
vi.mock('./settingsStore', () => ({
  useSettingsStore: { getState: () => ({ pendingReleaseAlerts: false, set: vi.fn() }) },
}))
// One instance `inst` with a single checkable plugin `p-a` (version 1.0.0).
vi.mock('./instanceStore', () => ({
  useInstanceStore: {
    getState: () => ({
      byId: (id: string) =>
        id === 'inst'
          ? { id: 'inst', name: 'Inst', plugins: [{ pluginId: 'p-a', version: '1.0.0', linked: false }] }
          : undefined,
      updateInstance: vi.fn(),
      stateOf: () => ({ status: instanceStatus.current }),
    }),
  },
}))

import { useCatalogStore } from './catalogStore'

beforeEach(() => {
  latest.mockReset()
  toggled.mockReset()
  toastSpy.mockReset()
  instanceStatus.current = 'stopped'
  useCatalogStore.setState({
    latestVersions: {},
    // pluginById reads `plugins`; seed a row so `p-a` is checkable.
    plugins: [{ id: 'p-a', name: 'A' } as never],
  })
})

describe('update-check state machine (§R5)', () => {
  it('a successful probe records a resolved state', async () => {
    latest.mockResolvedValue('2.0.0')
    await useCatalogStore.getState().refreshLatestVersions('inst')
    expect(useCatalogStore.getState().latestVersions['p-a']).toEqual({
      status: 'resolved',
      latest: '2.0.0',
    })
  })

  it('a first failure is an error; a later success is recoverable', async () => {
    latest.mockRejectedValueOnce(new Error('network down'))
    await useCatalogStore.getState().refreshLatestVersions('inst')
    expect(useCatalogStore.getState().latestVersions['p-a']).toEqual({
      status: 'error',
      message: 'network down',
    })
    // Auto refresh retries only never-checked / errored plugins — the errored
    // one is re-queried and now succeeds.
    latest.mockResolvedValueOnce('2.1.0')
    await useCatalogStore.getState().refreshLatestVersions('inst')
    expect(useCatalogStore.getState().latestVersions['p-a']).toEqual({
      status: 'resolved',
      latest: '2.1.0',
    })
  })

  it('a null from the repository is `unresolvable`, never auto-retried', async () => {
    latest.mockResolvedValue(null)
    await useCatalogStore.getState().refreshLatestVersions('inst')
    expect(useCatalogStore.getState().latestVersions['p-a']).toEqual({ status: 'unresolvable' })
    latest.mockClear()
    await useCatalogStore.getState().refreshLatestVersions('inst') // auto
    expect(latest).not.toHaveBeenCalled() // non-npm source must not loop (§R5)
    // A manual re-check, however, will poll it once more on demand.
    await useCatalogStore.getState().recheckLatestVersions('inst')
    expect(latest).toHaveBeenCalledTimes(1)
  })

  it('a resolved plugin is not re-polled by auto refresh, but is by manual re-check', async () => {
    latest.mockResolvedValue('1.5.0')
    await useCatalogStore.getState().refreshLatestVersions('inst')
    expect(useCatalogStore.getState().latestVersions['p-a'].status).toBe('resolved')
    latest.mockClear()
    await useCatalogStore.getState().refreshLatestVersions('inst') // auto: settled, skip
    expect(latest).not.toHaveBeenCalled()
    await useCatalogStore.getState().recheckLatestVersions('inst') // manual: force
    expect(latest).toHaveBeenCalledTimes(1)
  })

  it('concurrent refreshes never double-query an in-flight plugin', async () => {
    let release: (v: string) => void = () => {}
    latest.mockImplementation(
      () =>
        new Promise<string>((resolve) => {
          release = resolve
        }),
    )
    const a = useCatalogStore.getState().refreshLatestVersions('inst')
    const b = useCatalogStore.getState().refreshLatestVersions('inst')
    // The second call sees `checking` and skips, so only one probe is issued.
    expect(latest).toHaveBeenCalledTimes(1)
    release('3.0.0')
    await Promise.all([a, b])
    expect(useCatalogStore.getState().latestVersions['p-a']).toEqual({
      status: 'resolved',
      latest: '3.0.0',
    })
  })

  it('a superseded (stale) response must not overwrite a newer check', async () => {
    // Deterministic generation guard (no timing race): the first probe is an
    // error that resolves late (generation 1), the second resolves '2.0.0'
    // immediately (generation 2, after an explicit re-check cleared the
    // in-flight gate). The late error must NOT clobber the fresh result —
    // exactly the "换源 / 强制刷新时旧响应覆盖新查询" case §R5 calls out.
    let rejectFirst: (e: Error) => void = () => {}
    const firstProbe = new Promise<string>((_, rej) => {
      rejectFirst = rej
    })
    latest
      .mockImplementationOnce(() => firstProbe)
      .mockResolvedValueOnce('2.0.0')

    // Start the first (slow) check; do not await it.
    void useCatalogStore.getState().refreshLatestVersions('inst')
    // Force a second check: clear the in-flight gate, then re-query.
    useCatalogStore.setState((s) => ({
      latestVersions: { ...s.latestVersions, 'p-a': undefined as never },
    }))
    await useCatalogStore.getState().recheckLatestVersions('inst')
    expect(useCatalogStore.getState().latestVersions['p-a']).toEqual({
      status: 'resolved',
      latest: '2.0.0',
    })
    // Now the stale first probe fails late; it must be discarded by the
    // generation guard, leaving '2.0.0' intact.
    rejectFirst(new Error('late network drop'))
    await new Promise((r) => setTimeout(r, 0))
    expect(useCatalogStore.getState().latestVersions['p-a']).toEqual({
      status: 'resolved',
      latest: '2.0.0',
    })
  })
})

describe('plugin toggle serialization', () => {
  it('a double-click does not race two writes to the same patch file', async () => {
    // Two clicks used to fire two `cordis.patch.yml` writes; whichever request
    // landed last decided the file while whichever response landed last
    // decided the switch, so the UI could show the opposite of what DSH loads.
    let release: () => void = () => {}
    const gate = new Promise<void>((resolve) => {
      release = resolve
    })
    toggled.mockImplementation(async () => {
      await gate
    })

    const first = useCatalogStore.getState().setPluginEnabled('inst', 'p-a', true)
    const second = useCatalogStore.getState().setPluginEnabled('inst', 'p-a', false)
    release()
    await Promise.all([first, second])

    expect(toggled).toHaveBeenCalledTimes(1)
  })
})

describe('plugin toggle runtime semantics', () => {
  it('a toggle while the instance runs promises the NEXT launch, not the live one', async () => {
    // `cordis.patch.yml` is read at launch only; the old silent success made
    // the switch moving look like the running process had re-read it.
    instanceStatus.current = 'running'
    toggled.mockResolvedValue(undefined)
    await useCatalogStore.getState().setPluginEnabled('inst', 'p-a', false)
    expect(toastSpy).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: 'info',
        title: expect.stringContaining('重启实例后生效'),
      }),
    )
    expect(toastSpy).not.toHaveBeenCalledWith(expect.objectContaining({ kind: 'success' }))
  })

  it('a toggle while stopped reports plain success', async () => {
    toggled.mockResolvedValue(undefined)
    await useCatalogStore.getState().setPluginEnabled('inst', 'p-a', true)
    expect(toastSpy).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'success', title: expect.stringContaining('已启用') }),
    )
  })
})
