import { beforeEach, describe, expect, it, vi } from 'vitest'
const mocks = vi.hoisted(() => ({ sampleProcesses: vi.fn(), root: 'root-a' }))
vi.mock('@/services', () => ({ repository: mocks }))
vi.mock('@/lib/desktop', () => ({ isDesktop: true }))
vi.mock('@/stores/settingsStore', () => ({ useSettingsStore: { getState: () => ({ root: mocks.root }) } }))
import { useProcessStore } from './processStore'

beforeEach(() => {
  vi.resetAllMocks()
  mocks.root = 'root-a'
  useProcessStore.setState({ samples: {}, error: null, loading: false, refreshedAt: null })
})

describe('process sampling store', () => {
  it('shares a slow request instead of stacking requests', async () => {
    let finish!: (samples: []) => void
    mocks.sampleProcesses.mockReturnValue(new Promise((resolve) => { finish = resolve }))
    const first = useProcessStore.getState().refresh()
    const second = useProcessStore.getState().refresh()
    expect(second).toBe(first)
    await Promise.resolve()
    finish([])
    await first
    expect(mocks.sampleProcesses).toHaveBeenCalledTimes(1)
  })

  it('can retry both synchronous and asynchronous sampling failures', async () => {
    mocks.sampleProcesses.mockImplementationOnce(() => { throw new Error('probe denied') })
    await useProcessStore.getState().refresh()
    expect(useProcessStore.getState().error).toBe('probe denied')
    mocks.sampleProcesses.mockRejectedValueOnce(new Error('IPC failed'))
    await useProcessStore.getState().refresh()
    expect(useProcessStore.getState().error).toBe('IPC failed')
    mocks.sampleProcesses.mockResolvedValueOnce([])
    await useProcessStore.getState().refresh()
    expect(useProcessStore.getState().error).toBeNull()
  })

  it('discards results from an old data root', async () => {
    mocks.sampleProcesses.mockImplementationOnce(async () => {
      mocks.root = 'root-b'
      return [{ instanceId: 'old', pid: 1 }]
    })
    await useProcessStore.getState().refresh()
    expect(useProcessStore.getState().samples).toEqual({})
    expect(useProcessStore.getState().loading).toBe(false)
  })
})
